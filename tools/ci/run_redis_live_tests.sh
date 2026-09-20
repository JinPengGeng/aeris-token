#!/usr/bin/env bash
set -Eeuo pipefail

# Only child Redis processes with disposable directories and loopback ports are
# allowed here. An inherited external URL must never select another service.
if [[ -n "${AETHER_TEST_REDIS_URL:-}" ]]; then
  printf 'Unset AETHER_TEST_REDIS_URL: this harness only uses isolated Redis processes\n' >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
evidence_dir="${AETHER_REDIS_TEST_EVIDENCE_DIR:-artifacts/redis-runtime}"
mkdir -p "$evidence_dir"
export CARGO_TERM_COLOR=never
export AETHER_REQUIRE_LOCAL_REDIS_TESTS=1
redis_binary="${AETHER_REDIS_SERVER_BIN:-redis-server}"
probe_test=tests::redis_runtime_reuses_fixed_connections_for_repeated_operations
half_open_probe_test=orchestration::half_open_probe::tests::redis_two_gateway_probe_claim_allows_one_owner_and_rejects_stale_operations
send_admission_test=scheduler::send_admission::tests::redis_two_gateways_admit_one_due_half_open_probe_and_recheck_authority
key_concurrency_test=scheduler::send_admission::tests::redis_two_gateways_key_concurrent_limit_admits_one_then_recovers_after_release
scheduler_affinity_test=ai_serving::planner::candidate_source::tests::redis_affinity::redis_two_gateways_share_affinity_across_selector_pages_without_duplicates_or_omissions
stream_client_commit_test=tests::ai_execute::lifecycle::redis_two_gateways_do_not_replay_a_stream_after_client_commit
attempt_budget_locality_test=tests::ai_execute::lifecycle::redis_two_gateways_keep_attempt_budget_request_local_and_bounded

{
  printf 'Revision: '
  git rev-parse HEAD
  rustc --version
  cargo --version
  printf 'Redis binary: %s\n' "$redis_binary"
  "$redis_binary" --version
  printf 'AETHER_REQUIRE_LOCAL_REDIS_TESTS=%s\n' "$AETHER_REQUIRE_LOCAL_REDIS_TESTS"
  printf 'External Redis URL: disabled\n'
} 2>&1 | tee "$evidence_dir/environment.log"

run_probe() {
  cargo test --locked -p aether-runtime-state --lib "$probe_test" -- --exact --nocapture --test-threads=1
}

expect_strict_failure() {
  local label="$1" binary="$2" expected="$3"
  local log="$evidence_dir/$label.log"
  printf '>>> strict negative check: %s (%s)\n' "$label" "$probe_test" | tee "$log"
  if AETHER_REDIS_SERVER_BIN="$binary" run_probe >>"$log" 2>&1; then
    cat "$log"
    printf 'FAIL: strict Redis probe unexpectedly succeeded: %s\n' "$label" >&2
    exit 1
  fi
  cat "$log"
  # A compiler error, missing cargo or unmatched test filter is not evidence
  # that the Redis harness rejected the unavailable fixture.
  grep -Fq 'required Redis test failed (start isolated server)' "$log"
  grep -Fq "$expected" "$log"
  grep -Eq 'test result: FAILED\. 0 passed; 1 failed;' "$log"
  printf 'PASS: strict harness rejected %s\n' "$label" | tee -a "$log"
}

fixture="$(mktemp -d "${TMPDIR:-/tmp}/aether-redis-harness.XXXXXX")"
trap 'rmdir "$fixture"' EXIT
expect_strict_failure missing-binary "$fixture/unavailable-redis-server" 'could not spawn Redis binary'
expect_strict_failure failed-startup "$(type -P false)" 'test redis-server exited with'

printf '>>> optional local skip check: %s\n' "$probe_test" | tee "$evidence_dir/optional-skip.log"
AETHER_REQUIRE_LOCAL_REDIS_TESTS=0 AETHER_REDIS_SERVER_BIN="$fixture/unavailable-redis-server" \
  run_probe 2>&1 | tee -a "$evidence_dir/optional-skip.log"
grep -Fq 'SKIP: optional Redis test (start isolated server)' "$evidence_dir/optional-skip.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/optional-skip.log"

# Run the full crate, including shared memory/Redis contracts whose names do
# not contain "redis". Serial execution avoids contention in timing assertions.
printf '>>> cargo test --locked -p aether-runtime-state --lib -- --nocapture --test-threads=1\n' \
  | tee "$evidence_dir/runtime-tests.log"
cargo test --locked -p aether-runtime-state --lib -- --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/runtime-tests.log"
grep -Fq 'Redis test fixture ready: isolated server' "$evidence_dir/runtime-tests.log"
# Preserve the one deliberately ignored performance benchmark; no additional
# ignored contract or recovery tests may silently enter this required gate.
grep -Eq 'test result: ok\. [1-9][0-9]* passed; 0 failed; 1 ignored;' "$evidence_dir/runtime-tests.log"
grep -Fq 'test tests::usage_limit_cleanup::redis_usage_cleanup_large_window_timing ... ignored, isolated large-window timing baseline' \
  "$evidence_dir/runtime-tests.log"
if grep -Fq 'SKIP: optional Redis test' "$evidence_dir/runtime-tests.log"; then
  printf 'FAIL: strict Redis run contained an optional skip\n' >&2
  exit 1
fi

printf '>>> cargo test --locked -p aether-gateway --lib %s -- --ignored --exact --nocapture --test-threads=1\n' \
  "$half_open_probe_test" | tee "$evidence_dir/half-open-probe.log"
cargo test --locked -p aether-gateway --lib "$half_open_probe_test" -- --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/half-open-probe.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/half-open-probe.log"

printf '>>> cargo test --locked -p aether-gateway --lib %s -- --ignored --exact --nocapture --test-threads=1\n' \
  "$send_admission_test" | tee "$evidence_dir/send-admission.log"
cargo test --locked -p aether-gateway --lib "$send_admission_test" -- --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/send-admission.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/send-admission.log"

printf '>>> cargo test --locked -p aether-gateway --lib %s -- --ignored --exact --nocapture --test-threads=1\n' \
  "$key_concurrency_test" | tee "$evidence_dir/key-concurrency.log"
cargo test --locked -p aether-gateway --lib "$key_concurrency_test" -- --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/key-concurrency.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/key-concurrency.log"

printf '>>> cargo test --locked -p aether-gateway --lib %s -- --ignored --exact --nocapture --test-threads=1\n' \
  "$scheduler_affinity_test" | tee "$evidence_dir/scheduler-affinity.log"
cargo test --locked -p aether-gateway --lib "$scheduler_affinity_test" -- --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/scheduler-affinity.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/scheduler-affinity.log"

printf '>>> cargo test --locked -p aether-gateway --lib %s -- --ignored --exact --nocapture --test-threads=1\n' \
  "$stream_client_commit_test" | tee "$evidence_dir/stream-client-commit.log"
cargo test --locked -p aether-gateway --lib "$stream_client_commit_test" -- --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/stream-client-commit.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/stream-client-commit.log"

printf '>>> cargo test --locked -p aether-gateway --lib %s -- --ignored --exact --nocapture --test-threads=1\n' \
  "$attempt_budget_locality_test" | tee "$evidence_dir/attempt-budget-locality.log"
cargo test --locked -p aether-gateway --lib "$attempt_budget_locality_test" -- --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee -a "$evidence_dir/attempt-budget-locality.log"
grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$evidence_dir/attempt-budget-locality.log"

printf 'PASS: isolated strict Redis runtime tests, missing/startup failures, and optional local skip\n' \
  | tee "$evidence_dir/result.log"
