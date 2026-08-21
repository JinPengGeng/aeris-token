#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="${SCRIPT_ROOT}/manage-sync-automerge.sh"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-automerge.XXXXXX")"
export AERIS_AUTONOMY_EXPIRES_AT='2099-01-01T00:00:00Z'
export AERIS_WRITER_APP_SLUG='aeris-writer'
export BASE_BRANCH='main'

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
    'if [[ "$1" == "api" && "$2" == "--method" && "$3" == "PUT" && "${FAKE_GH_API_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_API_STATUS"; fi' \
    'if [[ "$1" == "api" && "$2" == "--method" && "$3" == "PUT" ]]; then printf "%s\n" "${FAKE_GH_API_RESPONSE:-}"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/pulls/* ]]; then printf "%s\n" "${FAKE_GH_PULL_RESPONSE:-}"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/commits/* && "${FAKE_GH_COMMIT_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_COMMIT_STATUS"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/commits/* ]]; then printf "%s\n" "${FAKE_GH_COMMIT_RESPONSE:-}"; fi' \
    'if [[ "$1 $2 ${3:-}" == "pr view "* ]]; then if [[ "${FAKE_GH_VIEW_RESPONSE+x}" == x ]]; then printf "%s\n" "${FAKE_GH_VIEW_RESPONSE}"; else printf "%s\n" "${FAKE_GH_AUTO_MERGE:-false}"; fi; fi' \
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
  PATH="${fake_bin}:${PATH}" FAKE_GH_LOG="${log}" \
    FAKE_GH_API_STATUS="${FAKE_GH_API_STATUS:-0}" \
    FAKE_GH_API_RESPONSE="${FAKE_GH_API_RESPONSE:-}" \
    FAKE_GH_PULL_RESPONSE="${FAKE_GH_PULL_RESPONSE:-}" \
    FAKE_GH_COMMIT_STATUS="${FAKE_GH_COMMIT_STATUS:-0}" \
    FAKE_GH_COMMIT_RESPONSE="${FAKE_GH_COMMIT_RESPONSE:-}" \
    "$HELPER" "$@"
}

test_merge_accepts_number_and_full_sha() {
  local bin="${RUN_ROOT}/merge-number/bin" log="${RUN_ROOT}/merge-number/gh.log" sha output
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"fedcba9876543210fedcba9876543210fedcba98"}' \
    FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_RESPONSE='{"sha":"fedcba9876543210fedcba9876543210fedcba98","parents":[{"sha":"abcdefabcdefabcdefabcdefabcdefabcdefabcd"}]}' \
  output="$(run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}")"
  assert_eq '' "${output}" 'merge should not emit output'
  assert_eq "api --method PUT repos/owner/repo/pulls/42/merge -f merge_method=squash -f sha=${sha} "$'\n'"api repos/owner/repo/pulls/42 "$'\n'"api repos/owner/repo/commits/fedcba9876543210fedcba9876543210fedcba98 " \
    "$(<"${log}")" 'merge arguments'
}

test_merge_accepts_matching_url() {
  local bin="${RUN_ROOT}/merge-url/bin" log="${RUN_ROOT}/merge-url/gh.log" sha
  sha='abcdefabcdefabcdefabcdefabcdefabcdefabcd'
  new_fake_gh "${bin}"
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"fedcba9876543210fedcba9876543210fedcba98"}' \
    FAKE_GH_PULL_RESPONSE="{\"number\":7,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_RESPONSE='{"sha":"fedcba9876543210fedcba9876543210fedcba98","parents":[{"sha":"abcdefabcdefabcdefabcdefabcdefabcdefabcd"}]}' \
    run_helper "${bin}" "${log}" merge owner/repo https://github.com/owner/repo/pull/7/ "${sha}"
  assert_eq "api --method PUT repos/owner/repo/pulls/7/merge -f merge_method=squash -f sha=${sha} "$'\n'"api repos/owner/repo/pulls/7 "$'\n'"api repos/owner/repo/commits/fedcba9876543210fedcba9876543210fedcba98 " \
    "$(<"${log}")" 'URL merge arguments'
}

test_merge_rejects_unproven_response() {
  local bin="${RUN_ROOT}/merge-unproven/bin" log="${RUN_ROOT}/merge-unproven/gh.log" status
  local sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"wrong"}' \
    FAKE_GH_PULL_RESPONSE='{"number":42,"state":"open"}' \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'unproven merge response must fail closed'
  assert_eq "api --method PUT repos/owner/repo/pulls/42/merge -f merge_method=squash -f sha=${sha} "$'\n'"api repos/owner/repo/pulls/42 " \
    "$(<"${log}")" 'unproven response must still perform one readback'
}

test_merge_accepts_lost_response_when_readback_is_exact() {
  local bin="${RUN_ROOT}/merge-lost/bin" log="${RUN_ROOT}/merge-lost/gh.log" sha
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  FAKE_GH_API_RESPONSE='not-json' \
    FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_RESPONSE='{"sha":"fedcba9876543210fedcba9876543210fedcba98","parents":[{"sha":"abcdefabcdefabcdefabcdefabcdefabcdefabcd"}]}' \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}"
  [[ "$(grep -c '^api ' "${log}")" -eq 3 ]] || fail 'lost response did not perform one mutation and two readbacks'
}

test_merge_failed_mutation_open_readback_fails() {
  local bin="${RUN_ROOT}/merge-failed-open/bin" log="${RUN_ROOT}/merge-failed-open/gh.log" sha status
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_STATUS=17 FAKE_GH_PULL_RESPONSE='{"number":42,"state":"open","head":{"sha":"0123456789abcdef0123456789abcdef01234567"}}' \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'failed mutation with open readback must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'failed mutation retried'
  [[ "$(grep -c '^api repos/owner/repo/pulls/42 ' "${log}")" -eq 1 ]] ||
    fail 'failed mutation did not perform exactly one pull readback'
}

test_merge_commit_readback_failure_fails_closed() {
  local bin="${RUN_ROOT}/merge-commit-failed/bin" log="${RUN_ROOT}/merge-commit-failed/gh.log" status
  local sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"fedcba9876543210fedcba9876543210fedcba98"}' \
    FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_STATUS=29 \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'failed commit readback must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'commit readback failure retried mutation'
  [[ "$(grep -c '^api repos/owner/repo/pulls/42 ' "${log}")" -eq 1 ]] ||
    fail 'commit readback failure did not perform exactly one pull readback'
  [[ "$(grep -c '^api repos/owner/repo/commits/fedcba9876543210fedcba9876543210fedcba98 ' "${log}")" -eq 1 ]] ||
    fail 'commit readback failure did not perform exactly one commit readback'
}

test_invalid_input_never_calls_gh() {
  local bin="${RUN_ROOT}/invalid/bin" log="${RUN_ROOT}/invalid/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" merge owner/repo https://github.com/other/repo/pull/7 abc >/dev/null 2>&1
  status=$?
  set -e
  [[ ! -e "${log}" ]] || fail 'invalid PR URL called gh'
  assert_status 64 "${status}" 'invalid PR URL status'

  set +e
  run_helper "${bin}" "${log}" merge owner/repo 7 deadbeef >/dev/null 2>&1
  status=$?
  set -e
  [[ ! -e "${log}" ]] || fail 'short SHA called gh'
  assert_status 64 "${status}" 'short SHA status'
}

test_api_failure_with_open_readback_fails_closed() {
  local bin="${RUN_ROOT}/api-failure/bin" log="${RUN_ROOT}/api-failure/gh.log" status
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_STATUS=23 \
    FAKE_GH_PULL_RESPONSE='{"number":7,"state":"open","head":{"sha":"0123456789abcdef0123456789abcdef01234567"}}' \
    run_helper "${bin}" "${log}" merge owner/repo 7 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'API failure with open readback must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'API failure retried mutation'
  [[ "$(grep -c '^api repos/owner/repo/pulls/7 ' "${log}")" -eq 1 ]] ||
    fail 'API failure did not perform exactly one pull readback'
}

test_legacy_arm_action_is_rejected() {
  local bin="${RUN_ROOT}/legacy-arm/bin" log="${RUN_ROOT}/legacy-arm/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" arm owner/repo 7 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'legacy arm action must be rejected'
  [[ ! -e "${log}" ]] || fail 'legacy arm action invoked gh'
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

test_merge_accepts_number_and_full_sha
test_merge_accepts_matching_url
test_merge_rejects_unproven_response
test_merge_accepts_lost_response_when_readback_is_exact
test_merge_failed_mutation_open_readback_fails
test_merge_commit_readback_failure_fails_closed
test_invalid_input_never_calls_gh
test_api_failure_with_open_readback_fails_closed
test_legacy_arm_action_is_rejected
test_disarm_when_enabled
test_disarm_is_noop_when_disabled
test_disarm_fails_closed_on_unknown_response
test_disarm_propagates_query_error
test_disarm_propagates_disable_error
test_disarm_blocks_mutation_when_expiry_crosses_after_read

printf 'PASS manage sync automerge (%s)\n' "${RUN_ROOT}"
