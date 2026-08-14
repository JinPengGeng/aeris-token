#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="${SCRIPT_ROOT}/checkpoint-merge.sh"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-checkpoint.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

assert_file_missing() {
  local tree="$1" path="$2"
  if git cat-file -e "${tree}:${path}" 2>/dev/null; then
    fail "${path} should not exist in ${tree}"
  fi
}

clean_tree() {
  local output tree
  output="$("${HELPER}" "$1" "$2" "$3")"
  grep -Fx 'state=clean' <<<"${output}" >/dev/null || fail 'clean state missing'
  tree="$(sed -n 's/^tree=//p' <<<"${output}")"
  git rev-parse --verify "${tree}^{tree}" >/dev/null
  printf '%s\n' "${tree}"
}

new_repo() {
  local path="$1"
  mkdir -p "${path}"
  git -C "${path}" init -q
  git -C "${path}" config user.name 'Checkpoint Test'
  git -C "${path}" config user.email 'checkpoint@example.com'
  git -C "${path}" config core.autocrlf false
}

test_incremental_and_noop() {
  local repo="${RUN_ROOT}/incremental" u0 u1 u2 main0 merged_tree main1 next_tree noop_tree
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >app.txt
  printf 'base\n' >fork.txt
  printf 'old\n' >obsolete.txt
  git add .
  git commit -qm 'base'
  u0="$(git rev-parse HEAD)"

  git switch -qc upstream
  printf 'upstream\n' >app.txt
  printf 'from upstream\n' >upstream.txt
  git mv obsolete.txt renamed.txt
  git add .
  git commit -qm 'upstream one'
  u1="$(git rev-parse HEAD)"

  git switch -qc main "${u0}"
  printf 'fork customization\n' >fork.txt
  mkdir -p .github
  printf '{"last_integrated_sha":"%s"}\n' "${u0}" >.github/upstream-sync-state.json
  git add .
  git commit -qm 'fork customization'
  main0="$(git rev-parse HEAD)"

  merged_tree="$(clean_tree "${u0}" "${main0}" "${u1}")"
  assert_eq 'upstream' "$(git show "${merged_tree}:app.txt")" 'upstream change missing'
  assert_eq 'fork customization' "$(git show "${merged_tree}:fork.txt")" 'fork change missing'
  assert_eq 'from upstream' "$(git show "${merged_tree}:upstream.txt")" 'upstream file missing'
  assert_eq 'old' "$(git show "${merged_tree}:renamed.txt")" 'rename missing'
  assert_file_missing "${merged_tree}" obsolete.txt

  git switch -q main
  git read-tree --reset -u "${merged_tree}"
  printf '{"last_integrated_sha":"%s"}\n' "${u1}" >.github/upstream-sync-state.json
  git add .
  git commit -qm 'integrate upstream one'
  main1="$(git rev-parse HEAD)"

  noop_tree="$(clean_tree "${u1}" "${main1}" "${u1}")"
  assert_eq "$(git rev-parse "${main1}^{tree}")" "${noop_tree}" 'same checkpoint must be a no-op'

  git switch -q upstream
  printf 'second upstream change\n' >second.txt
  git add second.txt
  git commit -qm 'upstream two'
  u2="$(git rev-parse HEAD)"

  next_tree="$(clean_tree "${u1}" "${main1}" "${u2}")"
  assert_eq 'fork customization' "$(git show "${next_tree}:fork.txt")" 'fork change lost on next sync'
  assert_eq 'second upstream change' "$(git show "${next_tree}:second.txt")" 'incremental change missing'
}

test_conflict_resolved_once() {
  local repo="${RUN_ROOT}/conflict" u0 u1 u2 main0 main1 next_tree output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >shared.txt
  git add shared.txt
  git commit -qm 'base'
  u0="$(git rev-parse HEAD)"

  git switch -qc upstream
  printf 'upstream\n' >shared.txt
  git commit -qam 'upstream conflict'
  u1="$(git rev-parse HEAD)"

  git switch -qc main "${u0}"
  printf 'fork\n' >shared.txt
  git commit -qam 'fork conflict'
  main0="$(git rev-parse HEAD)"

  set +e
  output="$("${HELPER}" "${u0}" "${main0}" "${u1}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 1 "${status}" 'same-hunk merge must report conflict'
  assert_eq 'state=conflict' "${output}" 'conflict state missing'

  printf 'resolved downstream policy\n' >shared.txt
  mkdir -p .github
  printf '{"last_integrated_sha":"%s"}\n' "${u1}" >.github/upstream-sync-state.json
  git add .
  git commit -qm 'resolve and integrate upstream one'
  main1="$(git rev-parse HEAD)"

  git switch -q upstream
  printf 'new upstream file\n' >new.txt
  git add new.txt
  git commit -qm 'upstream after resolution'
  u2="$(git rev-parse HEAD)"

  next_tree="$(clean_tree "${u1}" "${main1}" "${u2}")"
  assert_eq 'resolved downstream policy' "$(git show "${next_tree}:shared.txt")" \
    'resolved conflict should not recur'
  assert_eq 'new upstream file' "$(git show "${next_tree}:new.txt")" \
    'new upstream delta missing after resolution'
}

test_history_rewrite_rejected() {
  local repo="${RUN_ROOT}/rewrite" u0 main0 rewritten output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >file.txt
  git add file.txt
  git commit -qm 'base'
  u0="$(git rev-parse HEAD)"
  main0="${u0}"

  git switch -q --orphan rewritten
  printf 'rewritten\n' >file.txt
  git add file.txt
  git commit -qm 'rewritten history'
  rewritten="$(git rev-parse HEAD)"

  set +e
  output="$("${HELPER}" "${u0}" "${main0}" "${rewritten}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 2 "${status}" 'history rewrite must be rejected'
  assert_eq 'state=history_rewrite' "${output}" 'history rewrite state missing'
}

test_invalid_ref_reports_structured_error() {
  local repo="${RUN_ROOT}/invalid-ref" output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >file.txt
  git add file.txt
  git commit -qm 'base'

  set +e
  output="$("${HELPER}" missing-ref HEAD HEAD 2>/dev/null)"
  status=$?
  set -e
  assert_eq 3 "${status}" 'invalid ref must use the stable error exit code'
  assert_eq 'state=error' "${output}" 'invalid ref state missing'
}

test_unsupported_git_reports_structured_error() {
  local repo="${RUN_ROOT}/unsupported-git" fake_bin real_git output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >file.txt
  git add file.txt
  git commit -qm 'base'

  fake_bin="${repo}/fake-bin"
  real_git="$(command -v git)"
  mkdir -p "${fake_bin}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'if [[ "$1" == "version" ]]; then' \
    '  printf "git version 2.37.9\\n"' \
    '  exit 0' \
    'fi' \
    'exec "$REAL_GIT" "$@"' >"${fake_bin}/git"
  chmod +x "${fake_bin}/git"

  set +e
  output="$(PATH="${fake_bin}:${PATH}" REAL_GIT="${real_git}" \
    "${HELPER}" HEAD HEAD HEAD 2>/dev/null)"
  status=$?
  set -e
  assert_eq 3 "${status}" 'unsupported Git must use the stable error exit code'
  assert_eq 'state=error' "${output}" 'unsupported Git state missing'
}

test_git_helper_failure_reports_structured_error() {
  local repo="${RUN_ROOT}/git-helper-failure" fake_bin real_git output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >file.txt
  git add file.txt
  git commit -qm 'base'

  fake_bin="${repo}/fake-bin"
  real_git="$(command -v git)"
  mkdir -p "${fake_bin}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'if [[ "$1" == "merge-base" ]]; then' \
    '  printf "simulated Git helper failure\\n" >&2' \
    '  exit 128' \
    'fi' \
    'exec "$REAL_GIT" "$@"' >"${fake_bin}/git"
  chmod +x "${fake_bin}/git"

  set +e
  output="$(PATH="${fake_bin}:${PATH}" REAL_GIT="${real_git}" \
    "${HELPER}" HEAD HEAD HEAD 2>/dev/null)"
  status=$?
  set -e
  assert_eq 3 "${status}" 'Git helper failure must use the stable error exit code'
  assert_eq 'state=error' "${output}" 'Git helper failure state missing'
}

test_incremental_and_noop
test_conflict_resolved_once
test_history_rewrite_rejected
test_invalid_ref_reports_structured_error
test_unsupported_git_reports_structured_error
test_git_helper_failure_reports_structured_error

printf 'PASS checkpoint merge PoC (%s)\n' "${RUN_ROOT}"
