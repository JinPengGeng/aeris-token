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

if command -v pg_isready >/dev/null 2>&1; then
  pg_isready --dbname="${AETHER_TEST_DATABASE_URL}"
fi

run_test() {
  local package="$1"
  local test_name="$2"
  printf '>>> live PostgreSQL test: %s (%s)\n' "${test_name}" "${package}"
  cargo test -p "${package}" --all-features "${test_name}" --lib -- \
    --exact --include-ignored --nocapture --test-threads=1
}

# The migration smoke test creates the public schema used by the settlement
# fixture. Every test below is then run as an exact, serially logged target;
# no other ignored test is silently enabled by this harness.
run_test aether-data lifecycle::migrate::tests::postgres_migrations_create_core_config_tables_when_url_is_set
run_test aether-data-postgres usage::tests::live_first_byte_reads_provider_contribution_after_waiting_for_canonical_lock
run_test aether-data-postgres settlement::tests::live_usage_policy_window_aggregates_preserve_exact_admission_and_idempotency
run_test aether-data-postgres settlement::tests::live_daily_quota_serializes_each_entitlement_without_locking_shared_plan
run_test aether-data-postgres usage::tests::live_stale_terminal_event_is_a_full_transaction_noop
run_test aether-data-postgres pool::tests::live_session_deadlines_rollback_transactions_and_isolate_migration_overrides
run_test aether-data-postgres settlement::funding::tests::live_request_funds_reserve_settle_release_protect_shared_wallet_and_rollback
run_test aether-data-postgres settlement::funding::tests::live_request_funds_recovery_collects_only_unreserved_funds_once
run_test aether-data-postgres settlement::funding::tests::live_request_funds_freeze_entitlement_day_and_recover_legacy_partial_debit
run_test aether-data-postgres settlement::funding::tests::live_request_funds_admission_time_controls_grant_eligibility_and_frozen_day
run_test aether-data-postgres settlement::funding::tests::live_request_funds_preserve_decimal_holds_across_ordinary_settlement_and_postpaid
run_test aether-data-postgres settlement::funding::tests::live_request_funds_sum_entitlement_decimals_without_phantom_debt

printf 'PASS: selected isolated PostgreSQL live-DB tests\n'
