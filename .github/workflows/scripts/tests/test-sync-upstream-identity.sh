#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-sync-identity.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

run_identity_case() {
  local name="$1" login="$2" comment_id="$3" expected_posts="${4:-0}" exercise_pr_comment="${5:-false}"
  local root="${RUN_ROOT}/${name}" fake_bin="${RUN_ROOT}/${name}/bin"
  local calls="${RUN_ROOT}/${name}/gh-calls" harness="${RUN_ROOT}/${name}/harness.sh"
  mkdir -p "${fake_bin}"
  : >"${calls}"

  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
case "$*" in
  *'/issues/42/comments?per_page=100'*)
    filter=''
    while (($#)); do
      if [[ "$1" == --jq ]]; then filter="$2"; break; fi
      shift
    done
    [[ -n "${filter}" ]] || { printf 'missing jq filter\n' >&2; exit 1; }
    [[ "${filter}" == *'aeris-writer[bot]'* && "${filter}" == *'github-actions[bot]'* ]] || {
      printf 'comment filter omitted an accepted bot identity\n' >&2
      exit 1
    }
    [[ "${filter}" == *"${COMMENT_LOGIN}"* ]] || {
      printf 'configured comment author was not accepted\n' >&2
      exit 1
    }
    if [[ "${filter}" == *'startswith('* ]]; then
      [[ "${GH_TOKEN}" == test-writer-token && "${AERIS_TEST_CHANNEL:-}" == writer ]] || {
        printf 'pending-tip lookup used the wrong token channel\n' >&2
        exit 1
      }
      if [[ -n "${COMMENT_ID}" ]]; then
        printf '%s\n' "${COMMENT_ID}"
      fi
    else
      case "${AERIS_TEST_CHANNEL:-}" in
        issue)
          [[ "${GH_TOKEN}" == test-issues-token ]] || {
            printf 'ordinary issue comment lookup used the wrong token channel\n' >&2
            exit 1
          }
          ;;
        writer)
          [[ "${GH_TOKEN}" == test-writer-token ]] || {
            printf 'PR comment lookup used the wrong token channel\n' >&2
            exit 1
          }
          ;;
        *)
          printf 'comment lookup did not declare its channel\n' >&2
          exit 1
          ;;
      esac
      printf '%s\n' '<!-- upstream-sync-once -->'
    fi
    ;;
  *'--method PATCH repos/example/repo/issues/comments/'*)
    [[ "${GH_TOKEN}" == test-writer-token ]] || {
      printf 'pending-tip update used the wrong token channel\n' >&2
      exit 1
    }
    ;;
  *'--method POST repos/example/repo/issues/42/comments'*)
    [[ "${GH_TOKEN}" == test-writer-token ]] || {
      printf 'pending-tip creation used the wrong token channel\n' >&2
      exit 1
    }
    if [[ "$*" == *'upstream-sync-pending-tip:'* ]]; then
      [[ "${EXPECT_PENDING_POST}" == true ]] || {
        printf 'unexpected pending-tip REST comment creation\n' >&2
        exit 1
      }
    else
      [[ "${EXPECT_PR_POST}" == true ]] || {
        printf 'unexpected PR REST comment creation\n' >&2
        exit 1
      }
    fi
    ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${fake_bin}/gh"
  cp "${SCRIPT_ROOT}/github-autonomy.sh" "${root}/github-autonomy.sh"
  sed '/^parent="$(aeris_gh api "repos\/\${GITHUB_REPOSITORY}"/,$d' \
    "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
cat >>"${harness}" <<'EOF'
AERIS_TEST_CHANNEL=issue issue_comment_once 42 once 'must not duplicate'
AERIS_TEST_CHANNEL=writer set_pending_tip 42 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
if [[ "${EXERCISE_PR_COMMENT:-false}" == true ]]; then
  AERIS_TEST_CHANNEL=writer pr_comment_once 42 pr-once 'writer PR comment'
fi
EOF

  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" COMMENT_ID="${comment_id}" COMMENT_LOGIN="${login}" \
    EXPECT_PENDING_POST="$([[ "${expected_posts}" -ge 1 ]] && printf true || printf false)" \
    EXPECT_PR_POST="${exercise_pr_comment}" EXERCISE_PR_COMMENT="${exercise_pr_comment}" GH_TOKEN=test-writer-token \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_ISSUES_GH_TOKEN=test-issues-token AERIS_WRITER_APP_SLUG=aeris-writer \
    bash "${harness}"

  assert_eq "${expected_posts}" "$(grep -c -- '--method POST repos/example/repo/issues/42/comments' "${calls}" || true)" \
    "${name} PR comment writes must use REST"
  assert_eq 0 "$(grep -c -- '^pr comment ' "${calls}" || true)" \
    "${name} PR comments must not use GraphQL CLI"
  if [[ -n "${comment_id}" ]]; then
    assert_eq 1 "$(grep -c -- "--method PATCH repos/example/repo/issues/comments/${comment_id}" "${calls}" || true)" \
      "${name} pending-tip comment must be updated"
  fi
  grep -q 'aeris-writer\[bot\]' "${calls}" || fail "${name} query omitted the Writer App bot"
  grep -q 'github-actions\[bot\]' "${calls}" || fail "${name} query omitted the legacy bot"
}

run_identity_case app 'aeris-writer[bot]' 102
run_identity_case legacy 'github-actions[bot]' 202
run_identity_case rest-create 'aeris-writer[bot]' '' 1
run_identity_case pr-comment 'aeris-writer[bot]' 102 1 true
printf 'PASS sync upstream identity migration (%s)\n' "${RUN_ROOT}"
