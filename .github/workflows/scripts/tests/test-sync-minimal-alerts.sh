#!/usr/bin/env bash
# Regression fixture for the sync-upstream-minimal alert path (#180 first run):
# gh api defaults to POST when -f fields are present, so a read without an
# explicit --method became a bodiless POST /issues/{n}/comments (HTTP 422) and
# killed the run before any alert landed. The fake gh below enforces that
# method discipline, so dropping `--method` anywhere in the alert helper fails
# this test.
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-min-alert.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

fake_bin="${RUN_ROOT}/bin"
mkdir -p "${fake_bin}"
: >"${RUN_ROOT}/gh-calls"
cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
if [[ "$1" == api ]]; then
  has_method=false
  has_field=false
  for a in "$@"; do
    case "${a}" in
      --method) has_method=true ;;
      -f|-F|--raw-field|--field) has_field=true ;;
    esac
  done
  # gh CLI defaults to POST when -f/-F fields are present; the comments
  # endpoint then rejects the bodiless write with this exact 422.
  if [[ "${has_field}" == true && "${has_method}" == false ]]; then
    printf 'gh: Invalid request.\n\n"body" was'"'"'nt supplied. (HTTP 422)\n' >&2
    exit 1
  fi
fi
case "$*" in
  'issue list '*)
    if [[ -f "${GH_EXISTING_ISSUE}" ]]; then
      printf '42\n'
    fi
    ;;
  'api --method GET repos/example/repo/issues/42/comments '*)
    if [[ -n "${GH_FAIL_COMMENTS:-}" ]]; then
      printf 'gh: Invalid request.\n\n"body" was'"'"'nt supplied. (HTTP 422)\n' >&2
      exit 1
    fi
    if [[ -f "${GH_COMMENT_CREATED}" ]]; then
      printf '%s\n' '<!-- upstream-sync-alert-comment:conflict:deadbeef -->'
    fi
    ;;
  'api --method POST repos/example/repo/issues/42/comments '*)
    touch "${GH_COMMENT_CREATED}"
    ;;
  'issue create '*)
    touch "${GH_ISSUE_CREATED}"
    ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
chmod +x "${fake_bin}/gh"

harness="${RUN_ROOT}/alert-harness.sh"
cp "${SCRIPT_ROOT}/bounded-git-fetch.sh" "${RUN_ROOT}/bounded-git-fetch.sh"
sed '/^aeris_bounded_fetch_init /,$d' "${SCRIPT_ROOT}/sync-upstream-minimal.sh" >"${harness}"
printf '%s\n' 'report_sync_alert conflict deadbeef "merge conflict summary" "git merge exit=1 raw conflict output"' >>"${harness}"

run_harness() {
  PATH="${fake_bin}:${PATH}" \
    GH_CALLS="${RUN_ROOT}/gh-calls" \
    GH_EXISTING_ISSUE="${RUN_ROOT}/existing-issue" \
    GH_COMMENT_CREATED="${RUN_ROOT}/comment-created" \
    GH_ISSUE_CREATED="${RUN_ROOT}/issue-created" \
    GH_FAIL_COMMENTS="${GH_FAIL_COMMENTS:-}" \
    GITHUB_OUTPUT="${RUN_ROOT}/output" \
    GITHUB_STEP_SUMMARY='' \
    GITHUB_REPOSITORY=example/repo \
    UPSTREAM_REPOSITORY=up/stream \
    GH_TOKEN=test-token \
    bash "${harness}"
}

# A: existing open issue with the marker → exactly one comment across repeats,
#    with the raw error passed through into the comment body.
touch "${RUN_ROOT}/existing-issue"
run_harness
run_harness
assert_eq 1 "$(grep -c -- '--method POST repos/example/repo/issues/42/comments' "${RUN_ROOT}/gh-calls")" \
  'existing alert issue must receive exactly one comment across repeat failures'
[[ ! -f "${RUN_ROOT}/issue-created" ]] || fail 'existing issue path must not create a new issue'
grep -q -- '### Raw error' "${RUN_ROOT}/gh-calls" ||
  fail 'alert comment must carry the raw error section'
grep -q -- 'git merge exit=1 raw conflict output' "${RUN_ROOT}/gh-calls" ||
  fail 'alert comment must pass the raw error output through verbatim'

# B: no existing issue → created exactly once, with marker and raw error.
rm -f -- "${RUN_ROOT}/existing-issue" "${RUN_ROOT}/comment-created"
: >"${RUN_ROOT}/gh-calls"
run_harness
assert_eq 1 "$(grep -c -- '^issue create ' "${RUN_ROOT}/gh-calls")" 'missing alert issue must be created once'
grep -q -- '<!-- upstream-sync-alert:conflict:deadbeef -->' "${RUN_ROOT}/gh-calls" ||
  fail 'created alert issue must carry the dedupe marker'

# C: if the comment inventory read ever regresses to a bodiless POST (the #180
#    first-run 422), the alert helper must fail loudly on stderr, not silently.
: >"${RUN_ROOT}/gh-calls"
touch "${RUN_ROOT}/existing-issue"
rm -f -- "${RUN_ROOT}/comment-created"
if GH_FAIL_COMMENTS=1 run_harness 2>"${RUN_ROOT}/stderr-c"; then
  fail 'alert helper must fail when its comment inventory read fails'
fi
grep -q -- '"body" was'"'"'nt supplied' "${RUN_ROOT}/stderr-c" ||
  fail 'alert helper failure must pass the raw gh diagnostic to stderr'
[[ ! -f "${RUN_ROOT}/comment-created" ]] || fail 'a failed comment inventory must not post anything'

# D: empty raw detail is rejected fail-closed.
harness_empty="${RUN_ROOT}/alert-harness-empty.sh"
sed '/^aeris_bounded_fetch_init /,$d' "${SCRIPT_ROOT}/sync-upstream-minimal.sh" >"${harness_empty}"
printf '%s\n' 'report_sync_alert conflict deadbeef "summary" ""' >>"${harness_empty}"
if PATH="${fake_bin}:${PATH}" GH_CALLS="${RUN_ROOT}/gh-calls" \
   GITHUB_OUTPUT="${RUN_ROOT}/output" GITHUB_STEP_SUMMARY='' \
   GITHUB_REPOSITORY=example/repo UPSTREAM_REPOSITORY=up/stream GH_TOKEN=test-token \
   bash "${harness_empty}" 2>/dev/null; then
  fail 'empty raw detail must be rejected'
fi

printf 'PASS sync minimal alerts (%s)\n' "${RUN_ROOT}"
