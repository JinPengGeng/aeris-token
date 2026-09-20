#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/postgres-live-gate-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/logs"

cat > "$fixture/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CALLS"
printf '%s\n' "${AETHER_TEST_PROVIDER_COST_DATABASE_URL-}" >> "$MOCK_PROVIDER_COST_URLS"
case "$MOCK_RESULT" in
  zero) printf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n' ;;
  ignored) printf 'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n' ;;
  multiple) printf 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n' ;;
  missing) printf 'Finished test compilation without executing a test\n' ;;
  duplicate)
    printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    printf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n' ;;
  *) printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out; finished in 0.01s\n' ;;
esac
if [[ "$MOCK_RESULT" == failure ]]; then exit 42; fi
SH
cat > "$fixture/bin/pg_isready" <<'SH'
#!/usr/bin/env bash
exit 0
SH
cat > "$fixture/bin/createdb" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CREATEDB_CALLS"
SH
cat > "$fixture/bin/dropdb" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_DROPDB_CALLS"
if [[ "${MOCK_DROPDB_RESULT:-success}" == failure ]]; then
  exit 47
fi
SH
chmod +x "$fixture/bin/cargo" "$fixture/bin/pg_isready" "$fixture/bin/createdb" "$fixture/bin/dropdb"

run_fixture() {
  local result="$1"
  local dropdb_result="${2:-success}"
  : > "$fixture/$result.createdb.calls"
  : > "$fixture/$result.dropdb.calls"
  : > "$fixture/$result.provider-cost.urls"
  PATH="$fixture/bin:$PATH" AGENT_TMP_DIR="$fixture/logs" \
    MOCK_RESULT="$result" MOCK_CALLS="$fixture/$result.calls" \
    MOCK_CREATEDB_CALLS="$fixture/$result.createdb.calls" \
    MOCK_DROPDB_CALLS="$fixture/$result.dropdb.calls" \
    MOCK_DROPDB_RESULT="$dropdb_result" \
    MOCK_PROVIDER_COST_URLS="$fixture/$result.provider-cost.urls" \
    AETHER_TEST_DATABASE_URL='postgres://fixture.invalid/unused?host=%2Ftmp%2Ffixture&sslmode=disable' \
    AETHER_TEST_POSTGRES_URL='postgres://fixture.invalid/unused?host=%2Ftmp%2Ffixture&sslmode=disable' \
    bash "$repo_root/tools/ci/run_postgres_live_tests.sh" > "$fixture/$result.output" 2>&1
}

run_fixture success
grep -Fq 'PASS: selected isolated PostgreSQL live-DB tests' "$fixture/success.output"
grep -Fq 'video_tasks::tests::live_video_task_capture_claim_and_completion_preserve_business_fields --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'usage::tests::live_full_http_capture_round_trips_for_direct_and_batch_writes --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'provider_catalog::tests::live_endpoint_health_score_mapping_preserves_null_legacy_and_decode_failures --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'candidate_selection::tests::live_declared_global_models_include_unavailable_model_aliases --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'candidates::tests::live_postgres_candidate_nul_is_sanitized_and_legacy_json_is_discarded --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'emergency_chain::tests::live_emergency_chain_consume_is_single_use_and_checks_time_and_revocation --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'tests::control::admin::emergency_chain::live_admin_emergency_chain_uses_declared_order_and_stops_after_success --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'provider_cost::tests::provider_cost_imports_are_idempotent_and_unknown_stays_null --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'tests::provider_costs::settled_usage_capture_uses_effective_prices_and_replay_keeps_one_estimate --lib -- --exact --include-ignored' "$fixture/success.calls"
grep -Fq 'tests::provider_costs::live_provider_cost_http_imports_are_authorized_idempotent_and_keep_unknown_null --lib -- --exact --include-ignored' "$fixture/success.calls"
expected_count="$((
  $(grep -c '^run_test ' "$repo_root/tools/ci/run_postgres_live_tests.sh")
  + $(grep -c '^run_provider_cost_gateway_test ' "$repo_root/tools/ci/run_postgres_live_tests.sh")
))"
[[ "$(wc -l < "$fixture/success.calls" | tr -d ' ')" -eq "$expected_count" ]]
[[ "$(wc -l < "$fixture/success.createdb.calls" | tr -d ' ')" -eq 2 ]]
[[ "$(grep -c -- '--maintenance-db=postgres://fixture.invalid/unused?host=%2Ftmp%2Ffixture&sslmode=disable aether_provider_cost_' "$fixture/success.createdb.calls")" -eq 2 ]]
[[ "$(grep -Ec '^postgres://fixture.invalid/aether_provider_cost_[0-9]+_[0-9]+\?host=%2Ftmp%2Ffixture&sslmode=disable$' "$fixture/success.provider-cost.urls")" -eq 2 ]]
[[ "$(grep -E '^postgres://fixture.invalid/aether_provider_cost_[0-9]+_[0-9]+\?host=%2Ftmp%2Ffixture&sslmode=disable$' "$fixture/success.provider-cost.urls" | sort -u | wc -l | tr -d ' ')" -eq 2 ]]
[[ "$(wc -l < "$fixture/success.dropdb.calls" | tr -d ' ')" -eq 2 ]]

status=0
run_fixture dropdb_failure failure || status=$?
[[ "$status" -eq 1 ]]
[[ "$(wc -l < "$fixture/dropdb_failure.calls" | tr -d ' ')" -eq "$expected_count" ]]
[[ "$(wc -l < "$fixture/dropdb_failure.dropdb.calls" | tr -d ' ')" -eq 2 ]]
evidence="$(sed -n 's/^PostgreSQL live-test evidence retained at //p' "$fixture/dropdb_failure.output")"
[[ -d "$evidence" ]]

for result in zero ignored multiple missing duplicate failure; do
  status=0
  run_fixture "$result" || status=$?
  if [[ "$status" -eq 0 ]]; then
    printf 'FAIL: live DB gate accepted %s\n' "$result" >&2
    exit 1
  fi
  if [[ "$result" == failure && "$status" -ne 42 ]]; then
    printf 'FAIL: cargo failure status was not preserved: %s\n' "$status" >&2
    exit 1
  fi
  [[ "$(wc -l < "$fixture/$result.calls" | tr -d ' ')" -eq 1 ]]
  evidence="$(sed -n 's/^PostgreSQL live-test evidence retained at //p' "$fixture/$result.output")"
  [[ -d "$evidence" ]]
  [[ -n "$(find "$evidence" -name '*.log' -print -quit)" ]]
  [[ "$(wc -l < "$fixture/$result.dropdb.calls" | tr -d ' ')" -eq 0 ]]
done

# A full tee write followed by failure must not be hidden by cargo's success.
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

printf 'PASS: live PostgreSQL gate requires one executed test and preserves failures\n'
