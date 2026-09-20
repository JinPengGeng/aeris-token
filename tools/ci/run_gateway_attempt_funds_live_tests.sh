#!/usr/bin/env bash
set -Eeuo pipefail

# This harness runs only against an explicitly supplied disposable CI database.
: "${AETHER_TEST_DATABASE_URL:?AETHER_TEST_DATABASE_URL must point at a disposable PostgreSQL test database}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

log_root="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/aether-gateway-live.XXXXXX")"
cleanup_logs() {
  local status=$?
  if [[ "$status" -eq 0 ]]; then
    rm -rf -- "$log_root"
  else
    printf 'Gateway live-test evidence retained at %s\n' "$log_root" >&2
  fi
}
trap cleanup_logs EXIT

# Accept the full libtest path so additional Gateway modules can share the gate.
run_test() {
  local target="$1"
  local log_file="${log_root}/${target//:/_}.log"
  local -a command_status
  printf '>>> Gateway live test: %s\n' "$target"
  if cargo test --locked -p aether-gateway --lib "$target" -- \
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
  # libtest exits successfully for an exact filter that matched zero tests.
  if ! awk '
    /^test result:/ {
      summaries++
      if ($0 ~ /^test result: ok\. 1 passed; 0 failed; 0 ignored; /) passed++
    }
    END { exit !(summaries == 1 && passed == 1) }
  ' "$log_file"; then
    printf 'Expected exactly 1 passed, 0 failed, 0 ignored for %s; see %s\n' "$target" "$log_file" >&2
    return 1
  fi
}

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
  public_tests::crash_tests::live_public_image_process_kill_restart_unknown_hold_late_receipt_and_replay
  public_tests::receipt_tests::live_public_synthetic_image_receipt_matrix_uses_frozen_prices_and_retains_unknown
  public_tests::receipt_tests::live_public_synthetic_image_receipt_preserves_quote_across_catalog_price_change
  public_tests::receipt_tests::live_public_synthetic_image_truncated_receipt_retains_hold_then_settles_once
)

for test_name in "${tests[@]}"; do
  run_test "execution_runtime::funded_image::tests::$test_name"
done

run_test maintenance::runtime::recharge_recovery::live_tests::live_gateway_recharge_callback_collects_once_exposes_history_and_acks_smtp_retry
run_test maintenance::runtime::refund_notifications::live_tests::live_gateway_refund_notification_restarts_after_smtp_failure_without_repeating_money
run_test maintenance::runtime::refund_notifications::live_tests::live_gateway_refund_notification_preserves_consent_skips_and_retries_preference_read_failure

printf 'PASS: selected Gateway attempt funds PostgreSQL/HTTP tests\n'
