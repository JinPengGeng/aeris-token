#!/usr/bin/env bash
set -Eeuo pipefail

# This harness runs only against an explicitly supplied disposable CI database.
: "${AETHER_TEST_DATABASE_URL:?AETHER_TEST_DATABASE_URL must point at a disposable PostgreSQL test database}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

tests=(
  live_gateway_image_attempts_retry_late_charge_and_replay
  live_gateway_image_attempts_reject_second_upstream_when_held
  live_gateway_image_wallet_unavailable_preserves_prior_attempt_facts
  live_gateway_image_attempts_persistence_failure_sends_no_upstream
  live_gateway_image_attempt_cancellation_preserves_dispatched_hold
  live_gateway_image_attempt_reserve_commit_after_cancellation_is_released
  live_gateway_image_attempt_close_and_parent_write_retry_preserve_terminal
  live_gateway_image_attempt_changed_specs_require_reconciliation_without_overrun
  live_gateway_image_heartbeat_keeps_admission_after_response_headers
  public_tests::live_public_images_fund_user_standalone_unlimited_and_no_wallet_entitlement
  public_tests::live_public_images_reject_unbounded_and_stream_before_every_account_shortcut
  public_tests::live_public_image_retry_reserves_each_send_and_retains_unknown_hold
  public_tests::live_public_image_hard_quota_retry_unknown_late_charge_and_replay
  public_tests::live_public_image_hard_quota_applies_to_user_unlimited_and_entitlement_only
  public_tests::live_public_image_hard_quota_serializes_distinct_requests
  public_tests::live_public_image_wallet_disabled_after_auth_finishes_failed_parent
  public_tests::live_public_image_daily_cost_counts_late_charge_once_and_limits_next_request
)

for test_name in "${tests[@]}"; do
  target="execution_runtime::funded_image::tests::$test_name"
  printf '>>> Gateway attempt funds live test: %s\n' "$target"
  cargo test --locked -p aether-gateway --lib "$target" -- \
    --exact --include-ignored --nocapture --test-threads=1
done

printf 'PASS: selected Gateway attempt funds PostgreSQL/HTTP tests\n'
