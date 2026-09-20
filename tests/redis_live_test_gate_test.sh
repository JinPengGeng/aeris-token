#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/redis-live-gate-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/evidence"

cat > "$fixture/bin/redis-server" <<'SH'
#!/usr/bin/env bash
printf 'Redis server v=fixture\n'
SH
cat > "$fixture/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CALLS"
if [[ "$*" == *'tests::redis_runtime_reuses_fixed_connections_for_repeated_operations'* ]]; then
  if [[ "${AETHER_REQUIRE_LOCAL_REDIS_TESTS:-}" == 0 ]]; then
    printf 'SKIP: optional Redis test (start isolated server)\n'
    printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    exit 0
  fi
  if [[ "${AETHER_REDIS_SERVER_BIN:-}" == *unavailable-redis-server ]]; then
    printf 'required Redis test failed (start isolated server): could not spawn Redis binary\n'
  else
    printf 'required Redis test failed (start isolated server): test redis-server exited with 1\n'
  fi
  printf 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n'
  exit 1
fi
if [[ "$*" == 'test --locked -p aether-runtime-state --lib -- --nocapture --test-threads=1' ]]; then
  printf 'Redis test fixture ready: isolated server\n'
  printf 'test tests::usage_limit_cleanup::redis_usage_cleanup_large_window_timing ... ignored, isolated large-window timing baseline\n'
  printf 'test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n'
  exit 0
fi
printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
SH
chmod +x "$fixture/bin/redis-server" "$fixture/bin/cargo"

calls="$fixture/cargo.calls"
PATH="$fixture/bin:$PATH" \
  AGENT_TMP_DIR="$fixture/evidence" \
  AETHER_REDIS_TEST_EVIDENCE_DIR="$fixture/evidence" \
  MOCK_CALLS="$calls" \
  AETHER_REDIS_SERVER_BIN=redis-server \
  env -u AETHER_TEST_REDIS_URL \
  bash "$repo_root/tools/ci/run_redis_live_tests.sh" > "$fixture/output" 2>&1

grep -Fq 'PASS: isolated strict Redis runtime tests' "$fixture/output"
grep -Fq 'orchestration::half_open_probe::tests::redis_two_gateway_probe_claim_allows_one_owner_and_rejects_stale_operations -- --ignored --exact --nocapture --test-threads=1' "$calls"
grep -Fq 'scheduler::send_admission::tests::redis_two_gateways_admit_one_due_half_open_probe_and_recheck_authority -- --ignored --exact --nocapture --test-threads=1' "$calls"
grep -Fq 'scheduler::send_admission::tests::redis_two_gateways_key_concurrent_limit_admits_one_then_recovers_after_release -- --ignored --exact --nocapture --test-threads=1' "$calls"
grep -Fq 'ai_serving::planner::candidate_source::tests::redis_affinity::redis_two_gateways_share_affinity_across_selector_pages_without_duplicates_or_omissions -- --ignored --exact --nocapture --test-threads=1' "$calls"
grep -Fq 'tests::ai_execute::lifecycle::redis_two_gateways_do_not_replay_a_stream_after_client_commit -- --ignored --exact --nocapture --test-threads=1' "$calls"
grep -Fq 'tests::ai_execute::lifecycle::redis_two_gateways_keep_attempt_budget_request_local_and_bounded -- --ignored --exact --nocapture --test-threads=1' "$calls"
[[ "$(wc -l < "$calls" | tr -d ' ')" -eq 10 ]]

printf 'PASS: Redis live-test gate registers shared Redis selector acceptance\n'
