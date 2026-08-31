#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-sync-alert.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

test_existing_issue_uses_issue_comments_api_once() {
  local fake_bin="${RUN_ROOT}/bin" calls="${RUN_ROOT}/gh-calls" harness
  mkdir -p "${fake_bin}"
  : >"${calls}"
  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
case "$*" in
  'issue list '*)
    [[ "${GH_TOKEN}" == test-issues-token ]] || {
      printf 'issue inventory used the wrong token channel\n' >&2
      exit 1
    }
    printf '42\n'
    ;;
  *'api --method GET repos/example/repo/issues/42/comments'*'-f page=1'*)
    [[ "${GH_TOKEN}" == test-issues-token ]] || {
      printf 'ordinary issue comment lookup used the wrong token channel\n' >&2
      exit 1
    }
    if [[ -f "${GH_COMMENT_CREATED}" ]]; then
      printf '%s\n' '[{"id":1,"user":{"login":"aeris-writer[bot]"},"body":"<!-- upstream-sync-alert:conflict:deadbeef -->"}]'
    else
      printf '%s\n' '[]'
    fi
    ;;
  *'--method POST repos/example/repo/issues/42/comments'*)
    [[ "${GH_TOKEN}" == test-issues-token ]] || {
      printf 'ordinary issue comment write used the wrong token channel\n' >&2
      exit 1
    }
    touch "${GH_COMMENT_CREATED}"
    ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${fake_bin}/gh"

  harness="${RUN_ROOT}/alert-harness.sh"
  cp "${SCRIPT_ROOT}/github-autonomy.sh" "${RUN_ROOT}/github-autonomy.sh"
  cp "${SCRIPT_ROOT}/bounded-git-fetch.sh" "${RUN_ROOT}/bounded-git-fetch.sh"
  sed '/^mapfile -t sync_identity /,$d' "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  printf '%s\n' 'report_sync_alert conflict deadbeef "conflict detected"' >>"${harness}"

  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" GH_COMMENT_CREATED="${RUN_ROOT}/comment-created" GITHUB_OUTPUT="${RUN_ROOT}/output" GITHUB_REPOSITORY=example/repo AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z \
    AERIS_ISSUES_GH_TOKEN=test-issues-token AERIS_WRITER_APP_SLUG=aeris-writer bash "${harness}"
  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" GH_COMMENT_CREATED="${RUN_ROOT}/comment-created" GITHUB_OUTPUT="${RUN_ROOT}/output" GITHUB_REPOSITORY=example/repo AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z \
    AERIS_ISSUES_GH_TOKEN=test-issues-token AERIS_WRITER_APP_SLUG=aeris-writer bash "${harness}"

  assert_eq 1 "$(grep -c -- '--method POST repos/example/repo/issues/42/comments' "${calls}")" \
    'existing normal issue must receive one API comment'
  assert_eq 0 "$(grep -c -- '^pr comment ' "${calls}" || true)" \
    'normal issue must not use gh pr comment'
}

test_existing_issue_uses_issue_comments_api_once
printf 'PASS sync upstream alerts (%s)\n' "${RUN_ROOT}"
