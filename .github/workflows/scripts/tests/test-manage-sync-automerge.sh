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
    'case "$*" in' \
    '  api\ users/aeris-sync%5Bbot%5D) printf "{\"login\":\"aeris-sync[bot]\",\"id\":987654,\"type\":\"Bot\"}\n"; exit 0 ;;' \
    '  api\ repos/owner/repo/git/ref/heads/main) printf "{\"ref\":\"refs/heads/main\",\"object\":{\"sha\":\"%s\"}}\n" "${FAKE_GH_BASE_SHA}"; exit 0 ;;' \
    '  api\ repos/owner/repo/git/ref/heads/automation/sync-upstream)' \
    '    ref_sha="${FAKE_GH_HEAD_SHA}"; [[ "${FAKE_GH_REF_DRIFT:-false}" != true ]] || ref_sha=ffffffffffffffffffffffffffffffffffffffff' \
    '    printf "{\"ref\":\"refs/heads/automation/sync-upstream\",\"object\":{\"sha\":\"%s\"}}\n" "${ref_sha}"; exit 0 ;;' \
    '  api\ repos/owner/repo/pulls/*)' \
    '    count=0; if [[ -n "${FAKE_GH_PR_COUNTER:-}" ]]; then count="$(wc -l <"${FAKE_GH_PR_COUNTER}")"; printf "x\n" >>"${FAKE_GH_PR_COUNTER}"; fi' \
    '    draft=false; body="<!-- upstream-sync-managed -->\\n<!-- upstream-sync-owned-tip:${FAKE_GH_HEAD_SHA} -->\\n<!-- upstream-sync-source:example/Upstream@${FAKE_GH_HEAD_SHA} -->"' \
    '    if [[ "${FAKE_GH_RETARGET_ON_SECOND:-false}" == true && "${count}" -ge 1 ]]; then draft=true; body="${body}\\nretargeted"; fi' \
    '    number="${2##*/}"' \
    '    printf "{\"number\":%s,\"state\":\"open\",\"draft\":%s,\"body\":\"%s\",\"user\":{\"login\":\"aeris-sync[bot]\",\"id\":987654,\"type\":\"Bot\"},\"base\":{\"ref\":\"main\",\"sha\":\"%s\",\"repo\":{\"full_name\":\"owner/repo\"}},\"head\":{\"ref\":\"automation/sync-upstream\",\"sha\":\"%s\",\"repo\":{\"full_name\":\"owner/repo\"}}}\n" "${number}" "${draft}" "${body}" "${FAKE_GH_BASE_SHA}" "${FAKE_GH_HEAD_SHA}"; exit 0 ;;' \
    'esac' \
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
  local fake_bin="$1" log="$2" body body_sha verified_number
  shift 2
  local head_sha=0123456789abcdef0123456789abcdef01234567
  [[ "${1:-}" != arm ]] || head_sha="$4"
  body="<!-- upstream-sync-managed -->
<!-- upstream-sync-owned-tip:${head_sha} -->
<!-- upstream-sync-source:example/Upstream@${head_sha} -->"
  body_sha="$(printf '%s' "${body}" | sha256sum | awk '{ print $1 }')"
  verified_number="${3:-}"
  if [[ "${verified_number}" == https://* ]]; then
    verified_number="${verified_number%/}"
    verified_number="${verified_number##*/}"
  fi
  PATH="${fake_bin}:${PATH}" FAKE_GH_LOG="${log}" \
    FAKE_GH_HEAD_SHA="${head_sha}" FAKE_GH_BASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    AERIS_SYNC_APP_SLUG=aeris-sync VERIFIED_PR_NUMBER="${verified_number}" \
    VERIFIED_BASE_REF=main VERIFIED_BASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    VERIFIED_HEAD_REF=automation/sync-upstream VERIFIED_HEAD_SHA="${head_sha}" \
    VERIFIED_AUTHOR_LOGIN='aeris-sync[bot]' VERIFIED_AUTHOR_ID=987654 VERIFIED_AUTHOR_TYPE=Bot \
    VERIFIED_BODY_SHA256="${body_sha}" "$HELPER" "$@"
}

test_arm_accepts_number_and_full_sha() {
  local bin="${RUN_ROOT}/arm-number/bin" log="${RUN_ROOT}/arm-number/gh.log" sha output
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  output="$(run_helper "${bin}" "${log}" arm owner/repo 42 "${sha}")"
  assert_eq '' "${output}" 'arm should not emit output'
  assert_eq "pr merge 42 --repo owner/repo --auto --squash --match-head-commit ${sha} " \
    "$(tail -n1 "${log}")" 'arm arguments'
  assert_eq 2 "$(grep -c 'api repos/owner/repo/pulls/42' "${log}")" 'arm PR reread count'
}

test_arm_accepts_matching_url() {
  local bin="${RUN_ROOT}/arm-url/bin" log="${RUN_ROOT}/arm-url/gh.log" sha
  sha='abcdefabcdefabcdefabcdefabcdefabcdefabcd'
  new_fake_gh "${bin}"
  run_helper "${bin}" "${log}" arm owner/repo https://github.com/owner/repo/pull/7/ "${sha}"
  assert_eq "pr merge 7 --repo owner/repo --auto --squash --match-head-commit ${sha} " \
    "$(tail -n1 "${log}")" 'URL arm arguments'
}

test_arm_rejects_retarget_between_reads() {
  local bin="${RUN_ROOT}/arm-retarget/bin" log="${RUN_ROOT}/arm-retarget/gh.log"
  local counter="${RUN_ROOT}/arm-retarget/counter" sha status body_sha
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  : >"${counter}"
  body_sha="$(printf '%s' "<!-- upstream-sync-managed -->
<!-- upstream-sync-owned-tip:${sha} -->
<!-- upstream-sync-source:example/Upstream@${sha} -->" | sha256sum | awk '{ print $1 }')"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_HEAD_SHA="${sha}" \
    FAKE_GH_BASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    FAKE_GH_PR_COUNTER="${counter}" FAKE_GH_RETARGET_ON_SECOND=true \
    AERIS_SYNC_APP_SLUG=aeris-sync VERIFIED_PR_NUMBER=42 VERIFIED_BASE_REF=main \
    VERIFIED_BASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    VERIFIED_HEAD_REF=automation/sync-upstream VERIFIED_HEAD_SHA="${sha}" \
    VERIFIED_AUTHOR_LOGIN='aeris-sync[bot]' VERIFIED_AUTHOR_ID=987654 VERIFIED_AUTHOR_TYPE=Bot \
    VERIFIED_BODY_SHA256="${body_sha}" "$HELPER" arm owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'retarget between PR reads must fail closed'
  ! grep -q '^pr merge ' "${log}" || fail 'retargeted PR reached merge mutation'
}

test_arm_rejects_head_ref_force_push() {
  local bin="${RUN_ROOT}/arm-head-drift/bin" log="${RUN_ROOT}/arm-head-drift/gh.log"
  local sha status body_sha
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  body_sha="$(printf '%s' "<!-- upstream-sync-managed -->
<!-- upstream-sync-owned-tip:${sha} -->
<!-- upstream-sync-source:example/Upstream@${sha} -->" | sha256sum | awk '{ print $1 }')"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_HEAD_SHA="${sha}" \
    FAKE_GH_BASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa FAKE_GH_REF_DRIFT=true \
    AERIS_SYNC_APP_SLUG=aeris-sync VERIFIED_PR_NUMBER=42 VERIFIED_BASE_REF=main \
    VERIFIED_BASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    VERIFIED_HEAD_REF=automation/sync-upstream VERIFIED_HEAD_SHA="${sha}" \
    VERIFIED_AUTHOR_LOGIN='aeris-sync[bot]' VERIFIED_AUTHOR_ID=987654 VERIFIED_AUTHOR_TYPE=Bot \
    VERIFIED_BODY_SHA256="${body_sha}" "$HELPER" arm owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'head ref force-push must fail closed'
  ! grep -q '^pr merge ' "${log}" || fail 'force-pushed head reached merge mutation'
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
  [[ "${status}" -ne 0 ]] || fail 'gh failure was accepted'
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
test_arm_rejects_retarget_between_reads
test_arm_rejects_head_ref_force_push
test_invalid_input_never_calls_gh
test_gh_failure_propagates
test_disarm_when_enabled
test_disarm_is_noop_when_disabled
test_disarm_fails_closed_on_unknown_response
test_disarm_propagates_query_error
test_disarm_propagates_disable_error
test_disarm_blocks_mutation_when_expiry_crosses_after_read

printf 'PASS manage sync automerge (%s)\n' "${RUN_ROOT}"
