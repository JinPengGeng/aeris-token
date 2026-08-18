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
if [[ "${1:-}" == api ]]; then
  printf 'false\n'
fi
EOF
chmod +x "${FAKE_BIN}/gh"

common_env=(
  "PATH=${FAKE_BIN}:${PATH}"
  "GH_CALLS=${CALLS}"
  'GITHUB_REPOSITORY=example/repo'
  'SYNC_BRANCH=automation/sync-upstream'
  'SYNCED_SHA=0123456789abcdef'
  'AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z'
  'AERIS_CHECK_POLL_ATTEMPTS=1'
  'AERIS_CHECK_POLL_SECONDS=0'
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

echo 'ensure required checks token tests passed'
