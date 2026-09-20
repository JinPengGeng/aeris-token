#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runner="$repo_root/tools/ci/run_admin_audit_live_tests.sh"
fixture="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/admin-audit-live-gate-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/logs"

cat > "$fixture/bin/initdb" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_INITDB_CALLS"
while [[ $# -gt 0 ]]; do
  if [[ "$1" == -D ]]; then mkdir -p "$2"; exit 0; fi
  shift
done
exit 2
SH

cat > "$fixture/bin/pg_ctl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_PG_CTL_CALLS"
data_dir=
action=
while [[ $# -gt 0 ]]; do
  case "$1" in
    -D) data_dir="$2"; shift 2 ;;
    start|stop) action="$1"; shift ;;
    *) shift ;;
  esac
done
[[ -n "$data_dir" && -n "$action" ]]
if [[ "$action" == start ]]; then
  mkdir -p "$data_dir"
  printf 'synthetic-postmaster\n' > "$data_dir/postmaster.pid"
else
  rm -f -- "$data_dir/postmaster.pid"
fi
SH

cat > "$fixture/bin/createdb" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CREATEDB_CALLS"
SH

cat > "$fixture/bin/jq" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
socket=
while [[ $# -gt 0 ]]; do
  if [[ "$1" == --arg && "${2:-}" == socket ]]; then socket="$3"; shift 3; else shift; fi
done
printf '%s\n' "$socket"
SH

cat > "$fixture/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CARGO_CALLS"
printf '%s|%s|%s\n' \
  "${AETHER_TEST_AUDIT_DATABASE_URL-}" \
  "${AETHER_TEST_DATABASE_URL-}" \
  "${AETHER_TEST_POSTGRES_URL-}" >> "$MOCK_CARGO_ENVS"
printf 'synthetic administrator audit cargo stderr evidence\n' >&2
case "$MOCK_RESULT" in
  zero) printf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n' ;;
  ignored) printf 'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n' ;;
  multiple) printf 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n' ;;
  missing) printf 'Finished test compilation without executing a test\n' ;;
  duplicate)
    printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    printf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n' ;;
  failed_summary) printf 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n' ;;
  *) printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out; finished in 0.01s\n' ;;
esac
if [[ "$MOCK_RESULT" == failure ]]; then exit 42; fi
SH
chmod +x "$fixture/bin/"*

run_fixture() {
  local result="$1"
  PATH="$fixture/bin:$PATH" AGENT_TMP_DIR="$fixture/logs" \
    MOCK_RESULT="$result" \
    MOCK_CARGO_CALLS="$fixture/$result.cargo.calls" \
    MOCK_CARGO_ENVS="$fixture/$result.cargo.envs" \
    MOCK_INITDB_CALLS="$fixture/$result.initdb.calls" \
    MOCK_PG_CTL_CALLS="$fixture/$result.pg-ctl.calls" \
    MOCK_CREATEDB_CALLS="$fixture/$result.createdb.calls" \
    bash "$runner" > "$fixture/$result.output" 2>&1
}

assert_shutdown_and_evidence() {
  local result="$1" evidence
  grep -Fq ' stop -m fast -w -t 10' "$fixture/$result.pg-ctl.calls"
  evidence="$(sed -n 's/^Administrator audit evidence retained: //p' "$fixture/$result.output")"
  [[ -d "$evidence" ]]
  [[ -n "$(find "$evidence" -name '*.log' -print -quit)" ]]
  grep -Fq 'synthetic administrator audit cargo stderr evidence' "$evidence"/*.log
}

extract_array() {
  local array_name="$1" output="$2"
  awk -v header="${array_name}=(" '
  $0 == header { inside=1; next }
  inside && /^\)/ { exit }
  inside {
    sub(/^[[:space:]]*/, "")
    sub(/[[:space:]]*$/, "")
    if (length) print
  }
' "$runner" > "$output"
}

extract_array gateway_database_targets "$fixture/gateway-database-targets"
extract_array gateway_memory_targets "$fixture/gateway-memory-targets"
extract_array postgres_targets "$fixture/postgres-targets"

gateway_strong=tests::audit::admin_persistence::live_admin_mutations_persist_before_response_and_protected_readback
for target in \
  users::admin_session_audit_tests::admin_session_revocation_is_atomic_idempotent_and_redelivers_audit_only \
  users::admin_session_existence_tests::revoke_all_sessions_locks_user_and_rejects_a_concurrently_deleted_target \
  users::admin_session_existence_tests::revoke_all_sessions_orders_after_an_inflight_password_login \
  users::group_audit_tests::group_members_audit_enqueue_rolls_back_and_delivery_retry_never_rewrites_members \
  users::group_concurrency_tests::concurrent_empty_group_replaces_commit_complete_sets_and_relock_late_members \
  users::group_concurrency_tests::deleted_group_rejects_empty_replace_without_phantom_success_intent \
  users::group_concurrency_tests::group_replace_allows_later_per_user_cas_without_a_lock_cycle \
  users::group_concurrency_tests::group_replace_contention_exhaustion_leaves_members_and_intents_unchanged \
  wallet::admin_audit_tests::wallet_adjust_and_manual_recharge_audit_are_atomic_and_preserve_replay_contracts \
  audit::delivery_tests::durable_delivery_retries_fences_and_converges_to_one_row \
  audit::delivery_tests::expired_lease_after_blocked_insert_rolls_back_audit_and_ack \
  audit::delivery_tests::claim_bounds_and_strict_row_decode_fail_closed \
  audit::delivery_tests::concurrent_claims_are_disjoint_and_failures_dead_letter_at_the_bound \
  audit::delivery_tests::operator_redrive_is_single_winner_payload_preserving_and_fenced \
  audit::delivery_tests::delivery_keyset_is_stable_for_ties_and_concurrent_new_rows \
  audit::retention_tests::audit_retention_deletes_only_eligible_rows_with_exact_cutoff_and_batches \
  audit::retention_tests::audit_retention_preserves_unresolved_orphan_and_malformed_delivery_rows \
  audit::retention_tests::audit_retention_rolls_back_canonical_and_delivery_when_delete_fails \
  audit::retention_tests::audit_retention_concurrent_cleanups_are_disjoint \
  audit::retention_tests::audit_retention_skips_locked_delivered_pair_then_reclaims_it \
  audit::retention_tests::audit_retention_skips_live_delivery_lock_then_reclaims_after_delivery; do
  grep -Fxq "$target" "$fixture/postgres-targets"
done

[[ "$(wc -l < "$fixture/postgres-targets" | tr -d ' ')" -eq 21 ]]
[[ "$(wc -l < "$fixture/gateway-database-targets" | tr -d ' ')" -eq 5 ]]
[[ "$(wc -l < "$fixture/gateway-memory-targets" | tr -d ' ')" -eq 3 ]]
for target in \
  "$gateway_strong" \
  tests::audit::admin_delivery_crash::live_admin_audit_process_kill_restart \
  tests::audit::admin_sessions::authenticated_session_revocations_commit_audit_and_recover_without_business_replay \
  tests::audit::group_members::authenticated_group_members_mutation_commits_intent_and_retries_without_business_replay \
  tests::audit::wallet_balances::authenticated_wallet_mutations_enqueue_atomically_and_delivery_never_repeats_money_writes; do
  grep -Fxq "$target" "$fixture/gateway-database-targets"
done
for target in \
  tests::audit::admin_sessions::authenticated_session_revocations_keep_memory_fallback \
  tests::audit::group_members::authenticated_group_members_keep_non_postgres_fallback \
  tests::audit::wallet_balances::wallet_audit_unsupported_adapter_is_non_mutating_and_http_fallback_is_preserved; do
  grep -Fxq "$target" "$fixture/gateway-memory-targets"
done

while IFS= read -r target; do
  printf 'test --locked -p aether-gateway --lib %s -- --exact --include-ignored --nocapture --test-threads=1 --color never\n' "$target"
done < "$fixture/gateway-memory-targets" > "$fixture/expected.calls"
while IFS= read -r target; do
  printf 'test --locked -p aether-gateway --lib %s -- --exact --include-ignored --nocapture --test-threads=1 --color never\n' "$target"
done < "$fixture/gateway-database-targets" >> "$fixture/expected.calls"
while IFS= read -r target; do
  printf '%s\n' 'test --locked -p aether-data --all-features --lib lifecycle::migrate::tests::postgres_migrations_create_core_config_tables_when_url_is_set -- --exact --include-ignored --nocapture --test-threads=1 --color never'
  printf 'test --locked -p aether-data-postgres --all-features --lib %s -- --exact --include-ignored --nocapture --test-threads=1 --color never\n' "$target"
done < "$fixture/postgres-targets" >> "$fixture/expected.calls"

status=0
run_fixture success || status=$?
if [[ "$status" -ne 0 ]]; then
  cat "$fixture/success.output" >&2
  exit "$status"
fi
grep -Fq 'PASS: administrator HTTP mutation, durable audit delivery and PostgreSQL readback' "$fixture/success.output"
sort "$fixture/expected.calls" > "$fixture/expected.sorted"
sort "$fixture/success.cargo.calls" > "$fixture/success.sorted"
cmp "$fixture/expected.sorted" "$fixture/success.sorted"
[[ "$(wc -l < "$fixture/success.cargo.calls" | tr -d ' ')" -eq "$(wc -l < "$fixture/expected.calls" | tr -d ' ')" ]]
[[ "$(wc -l < "$fixture/success.cargo.calls" | tr -d ' ')" -eq 50 ]]
[[ "$(grep -Fxc '||' "$fixture/success.cargo.envs")" -eq "$(wc -l < "$fixture/gateway-memory-targets" | tr -d ' ')" ]]
[[ "$(wc -l < "$fixture/success.createdb.calls" | tr -d ' ')" -eq "$((
  $(wc -l < "$fixture/gateway-database-targets") + $(wc -l < "$fixture/postgres-targets")
))" ]]
[[ "$(wc -l < "$fixture/success.createdb.calls" | tr -d ' ')" -eq 26 ]]
[[ "$(sort -u "$fixture/success.createdb.calls" | wc -l | tr -d ' ')" -eq "$(wc -l < "$fixture/success.createdb.calls" | tr -d ' ')" ]]
assert_shutdown_and_evidence success

for result in zero ignored multiple missing duplicate failed_summary failure; do
  status=0
  run_fixture "$result" || status=$?
  if [[ "$status" -eq 0 ]]; then
    printf 'FAIL: administrator audit gate accepted %s\n' "$result" >&2
    exit 1
  fi
  if [[ "$result" == failure && "$status" -ne 42 ]]; then
    printf 'FAIL: cargo failure status was not preserved: %s\n' "$status" >&2
    exit 1
  fi
  [[ "$(wc -l < "$fixture/$result.cargo.calls" | tr -d ' ')" -eq 1 ]]
  assert_shutdown_and_evidence "$result"
done

cat > "$fixture/bin/tee" <<'SH'
#!/usr/bin/env bash
cat > "$1"
cat "$1"
exit 31
SH
chmod +x "$fixture/bin/tee"
status=0
run_fixture tee_failure || status=$?
[[ "$status" -eq 31 ]]
[[ "$(wc -l < "$fixture/tee_failure.cargo.calls" | tr -d ' ')" -eq 1 ]]
assert_shutdown_and_evidence tee_failure

printf 'PASS: administrator audit live gate executes its inventory, rejects false summaries and preserves failures\n'
