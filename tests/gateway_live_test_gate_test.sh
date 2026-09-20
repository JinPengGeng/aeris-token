#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runner="$repo_root/tools/ci/run_gateway_attempt_funds_live_tests.sh"
fixture="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/gateway-live-gate-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/logs"

cat > "$fixture/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CALLS"
printf 'synthetic gateway cargo stderr evidence\n' >&2
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
chmod +x "$fixture/bin/cargo"

# Pin the pre-existing inventory, including all three synthetic receipt targets.
cat > "$fixture/legacy-targets" <<'TARGETS'
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
TARGETS
[[ "$(wc -l < "$fixture/legacy-targets" | tr -d ' ')" -eq 21 ]]
sed 's/^/execution_runtime::funded_image::tests::/' "$fixture/legacy-targets" > "$fixture/expected-targets"
# Pin the real callback-to-SMTP target separately from the image module.
printf '%s\n' maintenance::runtime::recharge_recovery::live_tests::live_gateway_recharge_callback_collects_once_exposes_history_and_acks_smtp_retry >> "$fixture/expected-targets"
printf '%s\n' maintenance::runtime::refund_notifications::live_tests::live_gateway_refund_notification_restarts_after_smtp_failure_without_repeating_money >> "$fixture/expected-targets"
printf '%s\n' maintenance::runtime::refund_notifications::live_tests::live_gateway_refund_notification_preserves_consent_skips_and_retries_preference_read_failure >> "$fixture/expected-targets"
[[ "$(wc -l < "$fixture/expected-targets" | tr -d ' ')" -eq 24 ]]
while IFS= read -r target; do
  printf 'test --locked -p aether-gateway --lib %s -- --exact --include-ignored --nocapture --test-threads=1 --color never\n' "$target"
done < "$fixture/expected-targets" > "$fixture/expected-calls"

run_fixture() {
  local result="$1"
  PATH="$fixture/bin:$PATH" AGENT_TMP_DIR="$fixture/logs" \
    MOCK_RESULT="$result" MOCK_CALLS="$fixture/$result.calls" \
    AETHER_TEST_DATABASE_URL=postgres://fixture.invalid/unused \
    bash "$runner" > "$fixture/$result.output" 2>&1
}

assert_retained_evidence() {
  local result="$1"
  local evidence
  evidence="$(sed -n 's/^Gateway live-test evidence retained at //p' "$fixture/$result.output")"
  [[ -d "$evidence" ]]
  [[ -n "$(find "$evidence" -name '*.log' -print -quit)" ]]
  grep -Fq 'synthetic gateway cargo stderr evidence' "$evidence"/*.log
  ! grep -Fq 'PASS: selected Gateway attempt funds PostgreSQL/HTTP tests' "$fixture/$result.output"
}

run_fixture success
grep -Fq 'PASS: selected Gateway attempt funds PostgreSQL/HTTP tests' "$fixture/success.output"
cmp "$fixture/expected-calls" "$fixture/success.calls"
[[ -z "$(find "$fixture/logs" -name '*.log' -print -quit)" ]]

# Source the actual runner and invoke its function with a different full module.
full_target=maintenance::runtime::recharge_recovery::live_tests::synthetic_full_target_contract
PATH="$fixture/bin:$PATH" AGENT_TMP_DIR="$fixture/logs" \
  MOCK_RESULT=success MOCK_CALLS="$fixture/full.calls" \
  AETHER_TEST_DATABASE_URL=postgres://fixture.invalid/unused \
  bash -c 'source "$1"; run_test "$2"' _ "$runner" "$full_target" > "$fixture/full.output" 2>&1
cp "$fixture/expected-calls" "$fixture/full.expected"
printf 'test --locked -p aether-gateway --lib %s -- --exact --include-ignored --nocapture --test-threads=1 --color never\n' "$full_target" >> "$fixture/full.expected"
cmp "$fixture/full.expected" "$fixture/full.calls"

for result in zero ignored multiple missing duplicate failed_summary failure; do
  status=0
  run_fixture "$result" || status=$?
  if [[ "$status" -eq 0 ]]; then
    printf 'FAIL: Gateway live gate accepted %s\n' "$result" >&2
    exit 1
  fi
  if [[ "$result" == failure && "$status" -ne 42 ]]; then
    printf 'FAIL: cargo failure status was not preserved: %s\n' "$status" >&2
    exit 1
  fi
  [[ "$(wc -l < "$fixture/$result.calls" | tr -d ' ')" -eq 1 ]]
  assert_retained_evidence "$result"
done

# A complete write followed by tee failure must also preserve failure and logs.
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
[[ "$(wc -l < "$fixture/tee_failure.calls" | tr -d ' ')" -eq 1 ]]
assert_retained_evidence tee_failure

printf 'PASS: Gateway live gate executes its full inventory and requires one passed test\n'
