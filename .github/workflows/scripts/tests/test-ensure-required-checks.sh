#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-check-dispatch.XXXXXX")"
FAKE_BIN="${RUN_ROOT}/bin"
CALLS="${RUN_ROOT}/gh-calls"
mkdir -p "${FAKE_BIN}"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

cat >"${FAKE_BIN}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${GH_TOKEN:-}" == 'expected-job-token' ]] || {
  echo 'unexpected GH_TOKEN' >&2
  exit 41
}
printf '%s\n' "$*" >>"${GH_CALLS}"
if [[ "${1:-}" == api && "$*" == *' --jq '* ]]; then
  printf 'false\n'
  exit 0
fi
if [[ "${1:-}" == pr && "${2:-}" == view ]]; then
  pr_head="${FAKE_PR_HEAD_SHA:-${SYNCED_SHA}}"
  pr_base="${FAKE_PR_BASE_SHA:-${EXPECTED_BASE_SHA}}"
  printf '{"state":"OPEN","isDraft":false,"headRefOid":"%s","headRefName":"%s","headRepository":{"nameWithOwner":"%s"},"baseRefName":"main","baseRefOid":"%s","autoMergeRequest":null}\n' \
    "${pr_head}" "${SYNC_BRANCH}" "${GITHUB_REPOSITORY}" "${pr_base}"
  exit 0
fi
if [[ "${1:-}" == api && "${2:-}" == *'/check-runs?per_page=100' ]]; then
  mode="${FAKE_CHECK_MODE:-success}"
  rust_status='completed'
  rust_conclusion='"success"'
  policy_entry=',{"id":1003,"name":"Automation Policy / gate","head_sha":"'"${SYNCED_SHA}"'","status":"completed","conclusion":"success","details_url":"https://github.com/'"${GITHUB_REPOSITORY}"'/actions/runs/103/job/1003","check_suite":{"id":503},"app":{"id":15368,"slug":"github-actions"}}'
  total=3
  case "${mode}" in
    success) ;;
    pending) rust_status='in_progress'; rust_conclusion='null' ;;
    failure) rust_conclusion='"failure"' ;;
    missing-policy) policy_entry=''; total=2 ;;
    malformed) printf 'not-json\n'; exit 0 ;;
    *) exit 43 ;;
  esac
  printf '{"total_count":%s,"check_runs":[{"id":1001,"name":"Rust CI / check","head_sha":"%s","status":"%s","conclusion":%s,"details_url":"https://github.com/%s/actions/runs/101/job/1001","check_suite":{"id":501},"app":{"id":15368,"slug":"github-actions"}},{"id":1002,"name":"Frontend CI / check","head_sha":"%s","status":"completed","conclusion":"success","details_url":"https://github.com/%s/actions/runs/102/job/1002","check_suite":{"id":502},"app":{"id":15368,"slug":"github-actions"}}%s]}\n' \
    "${total}" "${SYNCED_SHA}" "${rust_status}" "${rust_conclusion}" "${GITHUB_REPOSITORY}" \
    "${SYNCED_SHA}" "${GITHUB_REPOSITORY}" "${policy_entry}"
  exit 0
fi
EOF
chmod +x "${FAKE_BIN}/gh"

common_env=(
  "PATH=${FAKE_BIN}:${PATH}"
  "GH_CALLS=${CALLS}"
  'GITHUB_REPOSITORY=example/repo'
  'SYNC_BRANCH=automation/sync-upstream'
  'SYNCED_SHA=0123456789abcdef0123456789abcdef01234567'
  'EXPECTED_BASE_SHA=abcdefabcdefabcdefabcdefabcdefabcdefabcd'
  'PR_URL=https://github.com/example/repo/pull/42'
  'AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z'
  'AERIS_CHECK_POLL_ATTEMPTS=1'
  'AERIS_CHECK_POLL_SECONDS=0'
  'AERIS_REQUIRED_CHECK_WAIT_ATTEMPTS=2'
  'AERIS_REQUIRED_CHECK_WAIT_SECONDS=0'
)

if env "${common_env[@]}" bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1; then
  fail 'dispatch accepted a missing workflow job token'
fi
if env "${common_env[@]}" GH_TOKEN=wrong-token \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1; then
  fail 'dispatch reached GitHub with an unexpected token'
fi

env "${common_env[@]}" GH_TOKEN=expected-job-token \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null

grep -Fxq 'workflow run --repo example/repo rust-ci.yml --ref automation/sync-upstream' "${CALLS}" ||
  fail 'Rust CI fallback was not dispatched with the workflow job token'
grep -Fxq 'workflow run --repo example/repo frontend-ci.yml --ref automation/sync-upstream' "${CALLS}" ||
  fail 'Frontend CI fallback was not dispatched with the workflow job token'
grep -Fq 'pr view 42 --repo example/repo --json state,isDraft,headRefOid,headRefName,headRepository,baseRefName,baseRefOid,autoMergeRequest' "${CALLS}" ||
  fail 'required-check wait did not bind the synchronization PR identity'
grep -Fxq 'api repos/example/repo/commits/0123456789abcdef0123456789abcdef01234567/check-runs?per_page=100' "${CALLS}" ||
  fail 'required-check wait did not inspect the exact synchronization head'

if env "${common_env[@]}" GH_TOKEN=expected-job-token \
  FAKE_PR_HEAD_SHA=fedcba9876543210fedcba9876543210fedcba98 \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1; then
  fail 'required-check wait accepted a drifted pull request head'
fi
if env "${common_env[@]}" GH_TOKEN=expected-job-token \
  FAKE_PR_BASE_SHA=fedcba9876543210fedcba9876543210fedcba98 \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1; then
  fail 'required-check wait accepted a drifted pull request base'
fi

set +e
env "${common_env[@]}" GH_TOKEN=expected-job-token FAKE_CHECK_MODE=pending \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1
pending_status=$?
env "${common_env[@]}" GH_TOKEN=expected-job-token FAKE_CHECK_MODE=failure \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1
failure_status=$?
env "${common_env[@]}" GH_TOKEN=expected-job-token FAKE_CHECK_MODE=missing-policy \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1
missing_status=$?
env "${common_env[@]}" GH_TOKEN=expected-job-token FAKE_CHECK_MODE=malformed \
  bash "${SCRIPT_ROOT}/ensure-required-checks.sh" >/dev/null 2>&1
malformed_status=$?
set -e

[[ "${pending_status}" -eq 78 ]] || fail 'pending required checks did not time out fail closed'
[[ "${failure_status}" -eq 1 ]] || fail 'failed required check did not stop immediately'
[[ "${missing_status}" -eq 78 ]] || fail 'missing Automation Policy gate did not time out fail closed'
[[ "${malformed_status}" -eq 78 ]] || fail 'malformed check response did not fail closed'

echo 'ensure required checks token tests passed'
