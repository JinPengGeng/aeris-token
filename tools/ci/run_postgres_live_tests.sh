#!/usr/bin/env bash
set -Eeuo pipefail

# This job owns a disposable PostgreSQL service. Keep the URL explicit so a
# local invocation cannot accidentally point at a developer's default DB.
: "${AETHER_TEST_DATABASE_URL:?AETHER_TEST_DATABASE_URL must point at a disposable PostgreSQL test database}"

if [[ -n "${AETHER_TEST_POSTGRES_URL:-}" && "${AETHER_TEST_POSTGRES_URL}" != "${AETHER_TEST_DATABASE_URL}" ]]; then
  printf 'AETHER_TEST_POSTGRES_URL must match AETHER_TEST_DATABASE_URL in the live-DB harness\n' >&2
  exit 2
fi
export AETHER_TEST_POSTGRES_URL="${AETHER_TEST_DATABASE_URL}"

log_root="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/aether-postgres-live.XXXXXX")"
# Bash with nounset treats an empty array expansion as unbound on some macOS
# runners. The sentinel keeps cleanup safe before either Gateway fixture runs.
provider_cost_databases=("")
cleanup_logs() {
  local status=$?
  local provider_cost_database
  trap - EXIT
  for provider_cost_database in "${provider_cost_databases[@]}"; do
    [[ -n "$provider_cost_database" ]] || continue
    if ! dropdb --if-exists --maintenance-db="$AETHER_TEST_DATABASE_URL" "$provider_cost_database" >>"$log_root/cleanup.log" 2>&1; then
      printf 'Failed to drop provider-cost database %s\n' "$provider_cost_database" >&2
      status=1
    fi
  done
  if [[ "$status" -eq 0 ]]; then
    rm -rf -- "$log_root"
  else
    printf 'PostgreSQL live-test evidence retained at %s\n' "$log_root" >&2
  fi
  exit "$status"
}
trap cleanup_logs EXIT

if command -v pg_isready >/dev/null 2>&1; then
  pg_isready --dbname="${AETHER_TEST_DATABASE_URL}"
fi

run_test() {
  local package="$1"
  local test_name="$2"
  local log_file="${log_root}/${package}-${test_name//:/_}.log"
  local -a command_status
  printf '>>> live PostgreSQL test: %s (%s)\n' "${test_name}" "${package}"
  if cargo test --locked -p "${package}" --all-features "${test_name}" --lib -- \
    --exact --include-ignored --nocapture --test-threads=1 --color never 2>&1 | tee "$log_file"; then
    command_status=("${PIPESTATUS[@]}")
  else
    command_status=("${PIPESTATUS[@]}")
  fi
  if [[ "${command_status[0]}" -ne 0 ]]; then
    return "${command_status[0]}"
  fi
  if [[ "${command_status[1]}" -ne 0 ]]; then
    return "${command_status[1]}"
  fi
  # libtest returns success even when an exact name no longer matches a test.
  # Require one executed target rather than accepting a compile-only green job.
  if ! awk '
    /^test result:/ {
      summaries++
      if ($0 ~ /^test result: ok\. 1 passed; 0 failed; 0 ignored; /) passed++
    }
    END { exit !(summaries == 1 && passed == 1) }
  ' "$log_file"; then
    printf 'Expected exactly 1 passed, 0 failed, 0 ignored for %s; see %s\n' "$test_name" "$log_file" >&2
    return 1
  fi
}

# The migration smoke test creates the public schema used by the settlement
# fixture. Every test below is then run as an exact, serially logged target;
# no other ignored test is silently enabled by this harness.
run_test aether-data lifecycle::migrate::tests::postgres_migrations_create_core_config_tables_when_url_is_set
run_test aether-data-postgres provider_catalog::tests::live_endpoint_health_score_mapping_preserves_null_legacy_and_decode_failures
run_test aether-data-postgres candidate_selection::tests::live_declared_global_models_include_unavailable_model_aliases
run_test aether-data-postgres candidates::tests::live_postgres_candidate_nul_is_sanitized_and_legacy_json_is_discarded
run_test aether-data-postgres provider_cost::tests::provider_cost_imports_are_idempotent_and_unknown_stays_null
run_test aether-data-postgres video_tasks::tests::live_video_task_capture_claim_and_completion_preserve_business_fields
run_test aether-data-postgres usage::tests::live_first_byte_reads_provider_contribution_after_waiting_for_canonical_lock
run_test aether-data-postgres usage::tests::live_capture_states_agree_across_request_id_id_and_batch_reads
run_test aether-data-postgres settlement::tests::live_usage_policy_window_aggregates_preserve_exact_admission_and_idempotency
run_test aether-data-postgres settlement::tests::live_daily_quota_serializes_each_entitlement_without_locking_shared_plan
run_test aether-data-postgres usage::tests::live_stale_terminal_event_is_a_full_transaction_noop
run_test aether-data-postgres pool::tests::live_session_deadlines_rollback_transactions_and_isolate_migration_overrides
run_test aether-data-postgres emergency_chain::tests::live_emergency_chain_grants_commit_audit_with_issue_and_revoke_or_roll_back_together
run_test aether-data-postgres emergency_chain::tests::live_emergency_chain_consume_is_single_use_and_checks_time_and_revocation
run_test aether-gateway tests::control::admin::emergency_chain::live_admin_emergency_chain_uses_declared_order_and_stops_after_success
run_test aether-data-postgres settlement::funding::tests::live_request_funds_reserve_settle_release_protect_shared_wallet_and_rollback
run_test aether-data-postgres settlement::funding::tests::live_request_funds_recovery_collects_only_unreserved_funds_once
run_test aether-data-postgres settlement::funding::tests::live_request_funds_freeze_entitlement_day_and_recover_legacy_partial_debit
run_test aether-data-postgres settlement::funding::tests::live_request_funds_admission_time_controls_grant_eligibility_and_frozen_day
run_test aether-data-postgres settlement::funding::tests::live_request_funds_preserve_decimal_holds_across_ordinary_settlement_and_postpaid
run_test aether-data-postgres settlement::funding::tests::live_request_funds_sum_entitlement_decimals_without_phantom_debt
run_test aether-data-postgres settlement::funding::tests::live_request_funds_retention_preserves_reconciliation_and_allows_later_settlement
run_test aether-data-postgres settlement::funding::attempts::tests::live_attempt_funds_retry_late_charge_and_provider_rebuild_are_idempotent
run_test aether-data-postgres settlement::funding::attempts::tests::live_attempt_funds_admission_entitlements_and_concurrent_settlement
run_test aether-data-postgres settlement::funding::attempts::tests::live_attempt_funds_quote_mismatch_preserves_known_charge_and_audit
run_test aether-data-postgres settlement::funding::attempts::tests::live_attempt_funds_settled_parent_accepts_final_lifecycle_without_financial_mutation
run_test aether-data-postgres settlement::funding::attempts::tests::stale_cleanup::live_stale_pending_cleanup_preserves_attempt_facts_and_processes_legacy_rows
run_test aether-data-postgres settlement::funding::attempts::tests::daily_cost::live_daily_cost_late_attempts_replay_and_rollback
run_test aether-data-postgres settlement::funding::attempts::tests::daily_cost::live_daily_cost_legacy_identity_scopes_and_concurrent_parents
run_test aether-data-postgres settlement::funding::attempts::tests::daily_cost::live_daily_cost_backfill_replays_source_and_preserves_frozen_identity
run_test aether-data-postgres settlement::funding::attempts::tests::daily_cost::live_daily_cost_pending_batches_freeze_identity_and_rollback_reuse
run_test aether-data-postgres settlement::funding::attempts::tests::daily_cost::live_daily_cost_fractional_midnight_backfill_keeps_day_after_revision
run_test aether-data-postgres settlement::funding::attempts::tests::quota::live_attempt_quota_unknown_retry_late_charge_and_replay
run_test aether-data-postgres settlement::funding::attempts::tests::quota::live_attempt_quota_freezes_policy_and_survives_legacy_expiry
run_test aether-data-postgres settlement::funding::attempts::tests::quota::live_attempt_quota_concurrent_requests_and_atomic_rollback
run_test aether-data-postgres settlement::funding::attempts::tests::quota::live_attempt_quota_entitlement_admission_and_outcome_rollback

# Recharge recovery: actual credits, immutable eligibility, concurrency and delivery.
run_test aether-data lifecycle::export::postgres::recharge_restore_tests::live_recharge_restore_never_reauthorizes_history_and_does_not_suppress_later_callbacks
run_test aether-data-postgres settlement::recharge_recovery::native_restore_tests::live_native_postgres_restore_preserves_nonempty_funds_and_replay_boundaries
run_test aether-data-postgres wallet::refund_notifications::tests::live_refund_outbox_all_terminal_branches_roll_back_when_enqueue_fails
run_test aether-data-postgres wallet::refund_notifications::tests::live_refund_outbox_concurrent_replay_and_restart_preserve_single_event
run_test aether-data-postgres wallet::refund_notifications::tests::live_refund_outbox_fences_expired_leases_and_counts_only_delivery_failures
run_test aether-data-postgres wallet::refund_notifications::tests::live_refund_outbox_quarantines_changed_owner_and_terminal_state

run_test aether-data-postgres settlement::recharge_recovery::boundary_tests::live_recharge_recovery_transaction_started_before_activation_can_credit_after_activation
run_test aether-data-postgres settlement::recharge_recovery::boundary_tests::live_recharge_recovery_excludes_later_failure_and_backdated_insert_from_old_credit
run_test aether-data-postgres settlement::recharge_recovery::boundary_tests::live_recharge_recovery_excludes_uncommitted_debt_even_when_its_transaction_started_first
run_test aether-data-postgres settlement::recharge_recovery::credit_lock_tests::live_recharge_recovery_callback_key_contention_rolls_back_then_replays_once
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_candidate_owner_change_requires_review
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_candidate_mode_change_requires_review
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_candidate_key_change_requires_review
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_candidate_missing_without_receipts_requires_review
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_paid_candidate_retention_allows_remaining_collection
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_settled_candidate_changed_cost_requires_review
run_test aether-data-postgres settlement::recharge_recovery::integrity_tests::live_recharge_recovery_unsafe_json_debt_total_requires_review_after_exact_debit
run_test aether-data-postgres settlement::recharge_recovery::refund_tests::live_recharge_recovery_refund_processing_locks_payment_before_wallet
run_test aether-data-postgres settlement::recharge_recovery::refund_tests::live_recharge_recovery_failed_refund_locks_payment_before_wallet
run_test aether-data-postgres settlement::recharge_recovery::refund_tests::live_recharge_recovery_refund_creation_locks_payment_before_wallet
run_test aether-data-postgres settlement::recharge_recovery::tests::live_recharge_recovery_callback_deduplicates_and_preserves_gifts_holds_and_budgets
run_test aether-data-postgres settlement::recharge_recovery::tests::live_recharge_recovery_activation_skips_history_and_collects_only_prior_debts_fifo
run_test aether-data-postgres settlement::recharge_recovery::tests::live_recharge_recovery_write_failure_rolls_back_and_sequence_replay_cannot_double_collect
run_test aether-data-postgres settlement::recharge_recovery::tests::live_recharge_recovery_standalone_wallet_does_not_pay_owner_or_foreign_key_debt
run_test aether-data-postgres settlement::recharge_recovery::tests::live_recharge_recovery_retains_known_collection_but_flags_unknown_remaining_debt
run_test aether-data-postgres settlement::recharge_recovery::tests::live_recharge_recovery_notifications_fence_leases_and_count_only_failed_delivery_retries

# Credit flows share the migrated schema but isolate their rows in pg_temp.
# Keep each existing regression visible as an exact target in required CI.
run_test aether-data-postgres wallet::tests::live_payment_callback_user_wallet_credits_once
run_test aether-data-postgres wallet::tests::live_payment_callback_api_key_wallet_credits_once
run_test aether-data-postgres wallet::tests::live_payment_callback_rejects_wrong_or_missing_wallet_owner
run_test aether-data-postgres wallet::tests::live_manual_recharge_commits_wallet_order_and_transaction
run_test aether-data-postgres wallet::tests::live_admin_order_state_changes_preserve_metadata
run_test aether-data-postgres wallet::tests::live_admin_wallet_order_credit_commits_once
run_test aether-data-postgres wallet::tests::live_admin_plan_order_credit_commits_once
run_test aether-data-postgres wallet::tests::live_redeem_code_commits_order_and_wallet_once_for_each_bucket

# Each Gateway provider-cost fixture asserts that its database is empty. Give
# each exact target a fresh owned database rather than sharing the regression
# database or another Gateway fixture's migration state.
run_provider_cost_gateway_test() {
  local test_name="$1"
  local provider_cost_database="aether_provider_cost_${$}_${RANDOM}"
  createdb --maintenance-db="$AETHER_TEST_DATABASE_URL" "$provider_cost_database"
  provider_cost_databases+=("$provider_cost_database")
  export AETHER_TEST_PROVIDER_COST_DATABASE_URL="$(python3 - "$AETHER_TEST_DATABASE_URL" "$provider_cost_database" <<'PY'
import sys
from urllib.parse import quote, urlsplit, urlunsplit

database_url = urlsplit(sys.argv[1])
if not database_url.scheme or not database_url.path or database_url.path == "/":
    raise SystemExit("AETHER_TEST_DATABASE_URL must include a database path")
print(urlunsplit(database_url._replace(path="/" + quote(sys.argv[2], safe=""))))
PY
)"
  run_test aether-gateway "$test_name"
}

run_provider_cost_gateway_test tests::provider_costs::settled_usage_capture_uses_effective_prices_and_replay_keeps_one_estimate
run_provider_cost_gateway_test tests::provider_costs::live_provider_cost_http_imports_are_authorized_idempotent_and_keep_unknown_null

printf 'PASS: selected isolated PostgreSQL live-DB tests\n'
