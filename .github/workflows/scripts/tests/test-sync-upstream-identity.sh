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
  local name="$1" login="$2" comment_id="$3" expect_post="${4:-false}"
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
      if [[ -n "${COMMENT_ID}" ]]; then
        printf '%s\n' "${COMMENT_ID}"
      fi
    else
      printf '%s\n' '<!-- upstream-sync-once -->'
    fi
    ;;
  *'--method PATCH repos/example/repo/issues/comments/'*) ;;
  *'--method POST repos/example/repo/issues/42/comments'*)
    [[ "${EXPECT_POST}" == true ]] || {
      printf 'unexpected REST comment creation\n' >&2
      exit 1
    }
    [[ "$*" == *'upstream-sync-pending-tip:'* ]] || {
      printf 'REST pending-tip creation omitted its marker\n' >&2
      exit 1
    }
    ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${fake_bin}/gh"
  cp "${SCRIPT_ROOT}/github-autonomy.sh" "${root}/github-autonomy.sh"
  sed '/^parent="$(aeris_gh api "repos\/\${GITHUB_REPOSITORY}"/,$d' \
    "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  cat >>"${harness}" <<'EOF'
issue_comment_once 42 once 'must not duplicate'
set_pending_tip 42 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
EOF

  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" COMMENT_ID="${comment_id}" COMMENT_LOGIN="${login}" EXPECT_POST="${expect_post}" \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_ISSUES_GH_TOKEN=test-issues-token AERIS_WRITER_APP_SLUG=aeris-writer \
    bash "${harness}"

  if [[ "${expect_post}" == true ]]; then
    assert_eq 1 "$(grep -c -- '--method POST repos/example/repo/issues/42/comments' "${calls}" || true)" \
      "${name} pending-tip comment must use REST creation"
    assert_eq 0 "$(grep -c -- '^pr comment ' "${calls}" || true)" \
      "${name} pending-tip comment must not use GraphQL CLI"
  else
    assert_eq 0 "$(grep -Ec -- '--method POST|^pr comment ' "${calls}" || true)" \
      "${name} comments must not be duplicated"
    assert_eq 1 "$(grep -c -- "--method PATCH repos/example/repo/issues/comments/${comment_id}" "${calls}" || true)" \
      "${name} pending-tip comment must be updated"
  fi
  grep -q 'aeris-writer\[bot\]' "${calls}" || fail "${name} query omitted the Writer App bot"
  grep -q 'github-actions\[bot\]' "${calls}" || fail "${name} query omitted the legacy bot"
}

run_identity_case app 'aeris-writer[bot]' 102
run_identity_case legacy 'github-actions[bot]' 202
run_identity_case rest-create 'aeris-writer[bot]' '' true
printf 'PASS sync upstream identity migration (%s)\n' "${RUN_ROOT}"
