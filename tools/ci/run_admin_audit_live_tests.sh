#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Own a socket-only PostgreSQL instance. Never migrate or clear a caller's DB.
for executable in initdb pg_ctl createdb cargo jq; do
  command -v "$executable" >/dev/null || { printf 'Missing executable: %s\n' "$executable" >&2; exit 2; }
done
fixture_dir="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/aether-admin-audit-ci.XXXXXX")"
fixture_dir="$(cd "$fixture_dir" && pwd -P)"
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -f "$fixture_dir/postgres/postmaster.pid" ]]; then
    pg_ctl -D "$fixture_dir/postgres" stop -m fast -w -t 10 >>"$fixture_dir/cleanup.log" 2>&1 || result=1
  fi
  printf 'Administrator audit evidence retained: %s\n' "$fixture_dir"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

initdb -D "$fixture_dir/postgres" -U audit_drill --auth-local=trust --auth-host=reject \
  --encoding=UTF8 --no-locale >"$fixture_dir/initdb.log" 2>&1
[[ "$fixture_dir" != *"'"* && "$fixture_dir" != *$'\n'* ]] || { printf 'Unsupported temporary path\n' >&2; exit 2; }
pg_ctl -D "$fixture_dir/postgres" -l "$fixture_dir/postgres.log" \
  -o "-k '$fixture_dir' -p 5432 -c listen_addresses=''" -w -t 10 start
encoded_socket="$(jq -rn --arg socket "$fixture_dir" '$socket | @uri')"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

new_database() {
  local name="$1"
  createdb -h "$fixture_dir" -p 5432 -U audit_drill "$name"
  export AETHER_TEST_AUDIT_DATABASE_URL="postgresql://audit_drill@localhost/$name?host=$encoded_socket&port=5432&sslmode=disable"
}

run_exact() {
  local package="$1" target="$2" log_file="$3"
  local -a audit_command=(cargo test --locked -p "$package") command_status
  if [[ "$package" != aether-gateway ]]; then audit_command+=(--all-features); fi
  audit_command+=(--lib "$target" -- --exact --include-ignored --nocapture --test-threads=1 --color never)
  printf '>>> Administrator audit live test: %s (%s)\n' "$target" "$package"
  if "${audit_command[@]}" 2>&1 | tee "$log_file"; then
    command_status=("${PIPESTATUS[@]}")
  else
    command_status=("${PIPESTATUS[@]}")
  fi
  if [[ "${command_status[0]}" -ne 0 ]]; then return "${command_status[0]}"; fi
  if [[ "${command_status[1]}" -ne 0 ]]; then return "${command_status[1]}"; fi
  if ! awk '
    /^test result:/ {
      summaries++
      if ($0 ~ /^test result: ok\. 1 passed; 0 failed; 0 ignored; /) passed++
    }
    END { exit !(summaries == 1 && passed == 1) }
  ' "$log_file"; then
    printf 'Expected exactly one executed administrator audit test; inspect %s\n' "$log_file" >&2
    return 1
  fi
}

gateway_database_targets=(
  tests::audit::admin_persistence::live_admin_mutations_persist_before_response_and_protected_readback
  tests::audit::admin_delivery_crash::live_admin_audit_process_kill_restart
  tests::audit::admin_sessions::authenticated_session_revocations_commit_audit_and_recover_without_business_replay
  tests::audit::group_members::authenticated_group_members_mutation_commits_intent_and_retries_without_business_replay
  tests::audit::wallet_balances::authenticated_wallet_mutations_enqueue_atomically_and_delivery_never_repeats_money_writes
)

gateway_memory_targets=(
  tests::audit::admin_sessions::authenticated_session_revocations_keep_memory_fallback
  tests::audit::group_members::authenticated_group_members_keep_non_postgres_fallback
  tests::audit::wallet_balances::wallet_audit_unsupported_adapter_is_non_mutating_and_http_fallback_is_preserved
)

postgres_targets=(
  users::admin_session_audit_tests::admin_session_revocation_is_atomic_idempotent_and_redelivers_audit_only
  users::admin_session_existence_tests::revoke_all_sessions_locks_user_and_rejects_a_concurrently_deleted_target
  users::admin_session_existence_tests::revoke_all_sessions_orders_after_an_inflight_password_login
  users::group_audit_tests::group_members_audit_enqueue_rolls_back_and_delivery_retry_never_rewrites_members
  users::group_concurrency_tests::concurrent_empty_group_replaces_commit_complete_sets_and_relock_late_members
  users::group_concurrency_tests::deleted_group_rejects_empty_replace_without_phantom_success_intent
  users::group_concurrency_tests::group_replace_allows_later_per_user_cas_without_a_lock_cycle
  users::group_concurrency_tests::group_replace_contention_exhaustion_leaves_members_and_intents_unchanged
  wallet::admin_audit_tests::wallet_adjust_and_manual_recharge_audit_are_atomic_and_preserve_replay_contracts
  audit::delivery_tests::durable_delivery_retries_fences_and_converges_to_one_row
  audit::delivery_tests::expired_lease_after_blocked_insert_rolls_back_audit_and_ack
  audit::delivery_tests::claim_bounds_and_strict_row_decode_fail_closed
  audit::delivery_tests::concurrent_claims_are_disjoint_and_failures_dead_letter_at_the_bound
  audit::delivery_tests::operator_redrive_is_single_winner_payload_preserving_and_fenced
  audit::delivery_tests::delivery_keyset_is_stable_for_ties_and_concurrent_new_rows
  audit::retention_tests::audit_retention_deletes_only_eligible_rows_with_exact_cutoff_and_batches
  audit::retention_tests::audit_retention_preserves_unresolved_orphan_and_malformed_delivery_rows
  audit::retention_tests::audit_retention_rolls_back_canonical_and_delivery_when_delete_fails
  audit::retention_tests::audit_retention_concurrent_cleanups_are_disjoint
  audit::retention_tests::audit_retention_skips_locked_delivered_pair_then_reclaims_it
  audit::retention_tests::audit_retention_skips_live_delivery_lock_then_reclaims_after_delivery
)

unset AETHER_TEST_AUDIT_DATABASE_URL AETHER_TEST_DATABASE_URL AETHER_TEST_POSTGRES_URL
index=0
for target in "${gateway_memory_targets[@]}"; do
  index=$((index + 1))
  run_exact aether-gateway "$target" "$fixture_dir/gateway-memory-$index.log"
done

index=0
for target in "${gateway_database_targets[@]}"; do
  index=$((index + 1))
  new_database "aether_admin_audit_gateway_$index"
  run_exact aether-gateway "$target" "$fixture_dir/gateway-database-$index.log"
done

index=0
for target in "${postgres_targets[@]}"; do
  index=$((index + 1))
  new_database "aether_admin_audit_delivery_$index"
  export AETHER_TEST_DATABASE_URL="$AETHER_TEST_AUDIT_DATABASE_URL"
  export AETHER_TEST_POSTGRES_URL="$AETHER_TEST_AUDIT_DATABASE_URL"
  run_exact aether-data \
    lifecycle::migrate::tests::postgres_migrations_create_core_config_tables_when_url_is_set \
    "$fixture_dir/migrate-$index.log"
  run_exact aether-data-postgres "$target" "$fixture_dir/postgres-$index.log"
done
printf 'PASS: administrator HTTP mutation, durable audit delivery and PostgreSQL readback\n'
