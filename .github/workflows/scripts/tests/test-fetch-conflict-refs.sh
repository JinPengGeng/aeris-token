#!/usr/bin/env bash
set -euo pipefail

# Deterministic fixtures for the conflict review/finalize ref fetch: the
# published head fence, the live upstream tip fence, and the bounded
# transport's hard deadline must each fail closed on a stale or stalled
# coordinate instead of revalidating or merging a stale conflict candidate.

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-conflict-refs-test.XXXXXX")"
STAGES="${RUN_ROOT}/stages"
mkdir -p "${STAGES}"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_stages_clean() {
  local residue
  residue="$(find "${STAGES}" -mindepth 1 -maxdepth 1 -type d -name 'aeris-bounded-fetch.*' -print -quit)"
  [[ -z "${residue}" ]] || fail "isolated receiver was not cleaned: ${residue}"
  residue="$(find "${STAGES}" -mindepth 1 -maxdepth 1 -type f -name 'aeris-remote-ref.*' -print -quit)"
  [[ -z "${residue}" ]] || fail "bounded remote-ref output was not cleaned: ${residue}"
}

# Upstream fixture: an independent history whose tip the conflict candidate
# pinned as its source.
UPSTREAM_SOURCE="${RUN_ROOT}/upstream-source"
UPSTREAM_REMOTE="${RUN_ROOT}/upstream-remote.git"
git init -q "${UPSTREAM_SOURCE}"
git -C "${UPSTREAM_SOURCE}" config user.name 'Conflict Fetch Fixture'
git -C "${UPSTREAM_SOURCE}" config user.email 'conflict-fetch@example.com'
git -C "${UPSTREAM_SOURCE}" config core.autocrlf false
printf 'upstream base\n' >"${UPSTREAM_SOURCE}/u.txt"
git -C "${UPSTREAM_SOURCE}" add .
git -C "${UPSTREAM_SOURCE}" commit -qm 'upstream base'
UPSTREAM_TIP="$(git -C "${UPSTREAM_SOURCE}" rev-parse HEAD)"
git init -q --bare "${UPSTREAM_REMOTE}"
git -C "${UPSTREAM_SOURCE}" push -q "${UPSTREAM_REMOTE}" "${UPSTREAM_TIP}:refs/heads/main"
git -C "${UPSTREAM_REMOTE}" symbolic-ref HEAD refs/heads/main

# Fork fixture: protected main plus the published synchronization branch head
# that the isolated jobs must re-prove.
ORIGIN_SOURCE="${RUN_ROOT}/origin-source"
ORIGIN_REMOTE="${RUN_ROOT}/origin-remote.git"
git init -q "${ORIGIN_SOURCE}"
git -C "${ORIGIN_SOURCE}" config user.name 'Conflict Fetch Fixture'
git -C "${ORIGIN_SOURCE}" config user.email 'conflict-fetch@example.com'
git -C "${ORIGIN_SOURCE}" config core.autocrlf false
printf 'fork base\n' >"${ORIGIN_SOURCE}/f.txt"
git -C "${ORIGIN_SOURCE}" add .
git -C "${ORIGIN_SOURCE}" commit -qm 'fork base'
printf 'resolved sync tree\n' >>"${ORIGIN_SOURCE}/f.txt"
git -C "${ORIGIN_SOURCE}" commit -qam 'published conflict candidate'
HEAD_SHA="$(git -C "${ORIGIN_SOURCE}" rev-parse HEAD)"
git init -q --bare "${ORIGIN_REMOTE}"
git -C "${ORIGIN_SOURCE}" push -q "${ORIGIN_REMOTE}" \
  HEAD:refs/heads/main \
  "${HEAD_SHA}:refs/heads/automation/sync-upstream"
git -C "${ORIGIN_REMOTE}" symbolic-ref HEAD refs/heads/main

# A job checkout clones the fork. The production upstream URL is rerouted to
# the local fixture remote through environment-scoped Git config, the only
# redirect the credentialless bounded transport tolerates; it reaches the
# isolated staging fetch as well, and GIT_ALLOW_PROTOCOL fails closed if the
# redirect ever stops applying.
new_checkout() {
  local name="$1"
  local path="${RUN_ROOT}/${name}"
  git clone -q "file://${ORIGIN_REMOTE}" "${path}"
  printf '%s\n' "${path}"
}

invoke_conflict_fetch() {
  local work="$1"
  shift
  (
    cd "${work}"
    unset GIT_SSH GIT_SSH_COMMAND SSH_ASKPASS GIT_ASKPASS GIT_CONFIG_COUNT \
      GIT_CONFIG_PARAMETERS GIT_CONFIG_SYSTEM GIT_CONFIG_GLOBAL GIT_CONFIG \
      GIT_PROXY_COMMAND GIT_HTTP_PROXY_AUTHMETHOD GIT_SSL_NO_VERIFY GIT_SSL_CAINFO \
      GIT_SSL_CAPATH CURL_HOME HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY \
      http_proxy https_proxy all_proxy no_proxy
    export HOME="${RUN_ROOT}/sterile-home" XDG_CONFIG_HOME="${RUN_ROOT}/sterile-home"
    export GIT_CONFIG_NOSYSTEM=1 GIT_ALLOW_PROTOCOL='file'
    export GIT_CONFIG_COUNT=1 \
      GIT_CONFIG_KEY_0="url.file://${UPSTREAM_REMOTE}.insteadOf" \
      GIT_CONFIG_VALUE_0='https://github.com/fixture/upstream.git'
    export AERIS_BOUNDED_FETCH_TEST_MODE=true \
      AERIS_BOUNDED_FETCH_TEST_FIXTURE=true \
      AERIS_BOUNDED_CREDENTIALLESS_TEST_ALLOW_CONFIG=true \
      AERIS_BOUNDED_FETCH_TMP_ROOT="${STAGES}"
    "$@"
  )
}
mkdir -p "${RUN_ROOT}/sterile-home"

expect_rejected_with() {
  local label="$1" pattern="$2" log="$3"
  shift 3
  if "$@" >"${log}" 2>&1; then
    fail "${label} was accepted"
  fi
  grep -qF "${pattern}" "${log}" ||
    fail "${label} missed the expected diagnostic '${pattern}': $(cat "${log}")"
}

WORK="$(new_checkout work-happy)"
invoke_conflict_fetch "${WORK}" \
  bash "${SCRIPT_ROOT}/fetch-conflict-refs.sh" \
    "${HEAD_SHA}" fixture/upstream main "${UPSTREAM_TIP}" automation/sync-upstream
[[ "$(git -C "${WORK}" rev-parse refs/remotes/origin/automation/sync-upstream)" == "${HEAD_SHA}" ]] ||
  fail 'bounded conflict fetch did not retain the exact published head'
[[ "$(git -C "${WORK}" rev-parse refs/remotes/upstream/main)" == "${UPSTREAM_TIP}" ]] ||
  fail 'bounded conflict fetch did not retain the exact upstream tip'
git -C "${WORK}" cat-file -e "${UPSTREAM_TIP}^{commit}" ||
  fail 'bounded conflict fetch did not import the pinned upstream commit'
assert_stages_clean

# Concurrency fence: upstream moved after the candidate was generated. The
# pinned source SHA must fail closed even though every object transfer would
# still succeed.
printf 'upstream moved\n' >>"${UPSTREAM_SOURCE}/u.txt"
git -C "${UPSTREAM_SOURCE}" commit -qam 'upstream drifted after candidate generation'
UPSTREAM_MOVED="$(git -C "${UPSTREAM_SOURCE}" rev-parse HEAD)"
git -C "${UPSTREAM_SOURCE}" push -q --force "${UPSTREAM_REMOTE}" "${UPSTREAM_MOVED}:refs/heads/main"
WORK_UPSTREAM_DRIFT="$(new_checkout work-upstream-drift)"
expect_rejected_with 'upstream tip drift fence' \
  "drifted from ${UPSTREAM_TIP} to ${UPSTREAM_MOVED}" "${RUN_ROOT}/upstream-drift.log" \
  invoke_conflict_fetch "${WORK_UPSTREAM_DRIFT}" \
    bash "${SCRIPT_ROOT}/fetch-conflict-refs.sh" \
      "${HEAD_SHA}" fixture/upstream main "${UPSTREAM_TIP}" automation/sync-upstream
git -C "${WORK_UPSTREAM_DRIFT}" show-ref --verify --quiet refs/remotes/upstream/main &&
  fail 'upstream drift fence published a stale upstream ref'
assert_stages_clean
git -C "${UPSTREAM_SOURCE}" push -q --force "${UPSTREAM_REMOTE}" "${UPSTREAM_TIP}:refs/heads/main"

# Concurrency fence: the synchronization branch moved after publication. The
# published head must fail closed before upstream is consulted, even though
# both commits are already local.
printf 'out-of-band write\n' >>"${ORIGIN_SOURCE}/f.txt"
git -C "${ORIGIN_SOURCE}" commit -qam 'synchronization branch moved after publication'
MOVED_HEAD="$(git -C "${ORIGIN_SOURCE}" rev-parse HEAD)"
git -C "${ORIGIN_SOURCE}" push -q --force "${ORIGIN_REMOTE}" \
  "${MOVED_HEAD}:refs/heads/automation/sync-upstream"
WORK_HEAD_DRIFT="$(new_checkout work-head-drift)"
expect_rejected_with 'published head drift fence' \
  "published synchronization head drifted from ${HEAD_SHA} to ${MOVED_HEAD}" \
  "${RUN_ROOT}/head-drift.log" \
  invoke_conflict_fetch "${WORK_HEAD_DRIFT}" \
    bash "${SCRIPT_ROOT}/fetch-conflict-refs.sh" \
      "${HEAD_SHA}" fixture/upstream main "${UPSTREAM_TIP}" automation/sync-upstream
git -C "${WORK_HEAD_DRIFT}" show-ref --verify --quiet refs/remotes/upstream/main &&
  fail 'published head fence did not stop before the upstream fetch'
assert_stages_clean

# Slow-transport bound: a stalled remote must hit the hard deadline instead of
# blocking the isolated job without a bound.
WORK_SLOW="$(new_checkout work-slow)"
expect_rejected_with 'stalled conflict fetch deadline' \
  'deadline' "${RUN_ROOT}/slow.log" \
  invoke_conflict_fetch "${WORK_SLOW}" \
    env AERIS_TEST_FETCH_TIMEOUT_SECONDS=1 AERIS_TEST_FETCH_DELAY_SECONDS=3 \
    bash "${SCRIPT_ROOT}/fetch-conflict-refs.sh" \
      "${HEAD_SHA}" fixture/upstream main "${UPSTREAM_TIP}" automation/sync-upstream
assert_stages_clean

expect_rejected_with 'malformed upstream source SHA' \
  'upstream source SHA' "${RUN_ROOT}/invalid.log" \
  invoke_conflict_fetch "${WORK}" \
    bash "${SCRIPT_ROOT}/fetch-conflict-refs.sh" \
      "${HEAD_SHA}" fixture/upstream main 'not-a-sha' automation/sync-upstream

echo 'conflict ref fetch fixtures passed'
