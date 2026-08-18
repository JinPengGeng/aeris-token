#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="${SCRIPT_ROOT}/manage-sync-automerge.sh"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-automerge.XXXXXX")"
export AERIS_AUTONOMY_EXPIRES_AT='2099-01-01T00:00:00Z'

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

assert_status() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" -eq "${expected}" ]] ||
    fail "${message}: expected status ${expected}, got ${actual}"
}

new_fake_gh() {
  local bin="$1"
  mkdir -p "${bin}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf "%q " "$@" >>"$FAKE_GH_LOG"' \
    'printf "\n" >>"$FAKE_GH_LOG"' \
    'if [[ "${FAKE_GH_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_STATUS"; fi' \
    'if [[ "$1 $2" == "pr merge" && "${FAKE_GH_MERGE_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_MERGE_STATUS"; fi' \
    'if [[ "$1 $2 $3" == "pr view "* ]]; then printf "%s\n" "${FAKE_GH_AUTO_MERGE:-false}"; fi' \
    >"${bin}/gh"
  chmod +x "${bin}/gh"
}

new_expiring_clock() {
  local bin="$1" clock_calls="$2"
  : >"${clock_calls}"
  cat >"${bin}/date" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  '-u -d 2033-05-18T03:33:20Z +%s') printf '2000000000\n' ;;
  '-u -d @2000000000 +%Y-%m-%dT%H:%M:%SZ') printf '2033-05-18T03:33:20Z\n' ;;
  '-u +%s')
    count="$(wc -l <"${CLOCK_CALLS}")"
    printf 'tick\n' >>"${CLOCK_CALLS}"
    if [[ "${count}" -eq 0 ]]; then printf '1999999999\n'; else printf '2000000000\n'; fi
    ;;
  *) printf 'unexpected date invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${bin}/date"
}

run_helper() {
  local fake_bin="$1" log="$2"
  shift 2
  PATH="${fake_bin}:${PATH}" FAKE_GH_LOG="${log}" "$HELPER" "$@"
}

test_arm_accepts_number_and_full_sha() {
  local bin="${RUN_ROOT}/arm-number/bin" log="${RUN_ROOT}/arm-number/gh.log" sha output
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  output="$(run_helper "${bin}" "${log}" arm owner/repo 42 "${sha}")"
  assert_eq '' "${output}" 'arm should not emit output'
  assert_eq "pr merge 42 --repo owner/repo --auto --squash --match-head-commit ${sha} " \
    "$(<"${log}")" 'arm arguments'
}

test_arm_accepts_matching_url() {
  local bin="${RUN_ROOT}/arm-url/bin" log="${RUN_ROOT}/arm-url/gh.log" sha
  sha='abcdefabcdefabcdefabcdefabcdefabcdefabcd'
  new_fake_gh "${bin}"
  run_helper "${bin}" "${log}" arm owner/repo https://github.com/owner/repo/pull/7/ "${sha}"
  assert_eq "pr merge 7 --repo owner/repo --auto --squash --match-head-commit ${sha} " \
    "$(<"${log}")" 'URL arm arguments'
}

test_invalid_input_never_calls_gh() {
  local bin="${RUN_ROOT}/invalid/bin" log="${RUN_ROOT}/invalid/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" arm owner/repo https://github.com/other/repo/pull/7 abc >/dev/null 2>&1
  status=$?
  set -e
  [[ ! -e "${log}" ]] || fail 'invalid PR URL called gh'
  assert_status 64 "${status}" 'invalid PR URL status'

  set +e
  run_helper "${bin}" "${log}" arm owner/repo 7 deadbeef >/dev/null 2>&1
  status=$?
  set -e
  [[ ! -e "${log}" ]] || fail 'short SHA called gh'
  assert_status 64 "${status}" 'short SHA status'
}

test_gh_failure_propagates() {
  local bin="${RUN_ROOT}/gh-failure/bin" log="${RUN_ROOT}/gh-failure/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_STATUS=23 \
    "$HELPER" arm owner/repo 7 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 23 "${status}" 'gh failure status'
  [[ -s "${log}" ]] || fail 'gh failure did not invoke gh'
}

test_disarm_when_enabled() {
  local bin="${RUN_ROOT}/disarm-enabled/bin" log="${RUN_ROOT}/disarm-enabled/gh.log"
  new_fake_gh "${bin}"
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=true \
    "$HELPER" disarm owner/repo 9
  assert_eq $'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\\ \\!=\\ null \npr merge 9 --repo owner/repo --disable-auto ' \
    "$(<"${log}")" 'enabled disarm arguments'
}

test_disarm_is_noop_when_disabled() {
  local bin="${RUN_ROOT}/disarm-disabled/bin" log="${RUN_ROOT}/disarm-disabled/gh.log"
  new_fake_gh "${bin}"
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=false \
    "$HELPER" disarm owner/repo 9
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'disabled disarm must not merge'
}

test_disarm_fails_closed_on_unknown_response() {
  local bin="${RUN_ROOT}/disarm-unknown/bin" log="${RUN_ROOT}/disarm-unknown/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=unknown \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'unknown auto-merge state must fail closed'
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'unknown state must not disable auto merge'
}

test_disarm_propagates_query_error() {
  local bin="${RUN_ROOT}/disarm-query-error/bin" log="${RUN_ROOT}/disarm-query-error/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_STATUS=31 \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 31 "${status}" 'auto-merge query error status'
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'query failure must not disable auto merge'
}

test_disarm_propagates_disable_error() {
  local bin="${RUN_ROOT}/disarm-disable-error/bin" log="${RUN_ROOT}/disarm-disable-error/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=true FAKE_GH_MERGE_STATUS=37 \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 37 "${status}" 'disable auto-merge error status'
  assert_eq $'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\\ \\!=\\ null \npr merge 9 --repo owner/repo --disable-auto ' \
    "$(<"${log}")" 'disable failure arguments'
}

test_disarm_blocks_mutation_when_expiry_crosses_after_read() {
  local bin="${RUN_ROOT}/disarm-expiry/bin" log="${RUN_ROOT}/disarm-expiry/gh.log"
  local clock_calls="${RUN_ROOT}/disarm-expiry/clock.log" status
  new_fake_gh "${bin}"
  new_expiring_clock "${bin}" "${clock_calls}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=true \
    CLOCK_CALLS="${clock_calls}" AERIS_AUTONOMY_EXPIRES_AT='2033-05-18T03:33:20Z' \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 78 "${status}" 'expiry after auto-merge read must fail closed'
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'expired disarm must not reach the mutation'
}

test_arm_accepts_number_and_full_sha
test_arm_accepts_matching_url
test_invalid_input_never_calls_gh
test_gh_failure_propagates
test_disarm_when_enabled
test_disarm_is_noop_when_disabled
test_disarm_fails_closed_on_unknown_response
test_disarm_propagates_query_error
test_disarm_propagates_disable_error
test_disarm_blocks_mutation_when_expiry_crosses_after_read

printf 'PASS manage sync automerge (%s)\n' "${RUN_ROOT}"
