#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/github-autonomy.sh"
source "${SCRIPT_DIR}/bounded-git-fetch.sh"

BASE_BRANCH="${BASE_BRANCH:-main}"
SYNC_BRANCH="${SYNC_BRANCH:-automation/sync-upstream}"
MAX_PR_JSON_BYTES=2097152

usage() {
  printf '%s\n' 'usage: manage-sync-automerge.sh <arm|disarm> <owner/repo> <pr-number|pr-url> [head-sha]'
  exit 64
}

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 64
}

require_repo() {
  [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$ ]] ||
    fail 'repository must be owner/repo'
}

parse_pr() {
  local value="$1" url_owner url_repo

  if [[ "${value}" =~ ^[1-9][0-9]*$ ]]; then
    PR_NUMBER="${value}"
    return
  fi

  if [[ "${value}" =~ ^https://github\.com/([A-Za-z0-9][A-Za-z0-9._-]*)/([A-Za-z0-9][A-Za-z0-9._-]*)/pull/([1-9][0-9]*)/?$ ]]; then
    url_owner="${BASH_REMATCH[1]}"
    url_repo="${BASH_REMATCH[2]}"
    PR_NUMBER="${BASH_REMATCH[3]}"
    [[ "${url_owner}/${url_repo}" == "${REPOSITORY}" ]] ||
      fail 'pull request URL repository does not match repository argument'
    return
  fi

  fail 'pull request must be a positive number or a GitHub pull request URL'
}

read_bounded_api_json() {
  local endpoint="$1" destination="$2" size
  aeris_require_active_autonomy_window || return
  aeris_bounded_run "${MAX_PR_JSON_BYTES}" gh api "${endpoint}" >"${destination}" || return
  size="$(wc -c <"${destination}")"
  [[ "${size}" =~ ^[0-9]+$ && ${size} -gt 0 && ${size} -le ${MAX_PR_JSON_BYTES} ]]
}

validate_arm_snapshot() {
  local pr_file="$1" prior_file="$2" user_file="$3" base_file="$4" head_file="$5"
  node - "${pr_file}" "${prior_file}" "${user_file}" "${base_file}" "${head_file}" \
    "${PR_NUMBER}" "${REPOSITORY}" "${BASE_BRANCH}" "${SYNC_BRANCH}" "${HEAD_SHA}" \
    "${AERIS_SYNC_APP_SLUG}[bot]" <<'NODE'
const fs = require('node:fs');
const [prPath, priorPath, userPath, basePath, headPath, number, repository,
  baseRef, headRef, headSha, expectedLogin] = process.argv.slice(2);
const read = (path) => JSON.parse(fs.readFileSync(path, 'utf8'));
const pr = read(prPath); const user = read(userPath);
const base = read(basePath); const head = read(headPath);
const body = pr.body;
const bodySha256 = require('node:crypto').createHash('sha256').update(body ?? '', 'utf8').digest('hex');
const project = (value) => ({
  number: value.number, state: value.state, draft: value.draft,
  user: { login: value.user?.login, id: value.user?.id, type: value.user?.type },
  body: value.body,
  base: { ref: value.base?.ref, sha: value.base?.sha,
    repo: { full_name: value.base?.repo?.full_name } },
  head: { ref: value.head?.ref, sha: value.head?.sha,
    repo: { full_name: value.head?.repo?.full_name } },
});
const priorMatches = priorPath === '-' ||
  JSON.stringify(project(pr)) === JSON.stringify(project(read(priorPath)));
const valid = user && user.login === expectedLogin && Number.isSafeInteger(user.id) &&
  user.id > 0 && user.type === 'Bot' && pr && pr.number === Number(number) &&
  pr.state === 'open' && pr.draft === false && pr.user?.login === user.login &&
  pr.user?.id === user.id && pr.user?.type === user.type &&
  pr.base?.repo?.full_name === repository && pr.base?.ref === baseRef &&
  pr.base?.sha === base.object?.sha && base.ref === `refs/heads/${baseRef}` &&
  pr.head?.repo?.full_name === repository && pr.head?.ref === headRef &&
  pr.head?.sha === headSha && head.object?.sha === headSha &&
  head.ref === `refs/heads/${headRef}` && typeof body === 'string' &&
  body.includes('<!-- upstream-sync-managed -->') &&
  body.includes(`<!-- upstream-sync-owned-tip:${headSha} -->`) &&
  /^<!-- upstream-sync-source:[^\s@]+\/[^\s@]+@[0-9a-f]{40} -->$/m.test(body) &&
  number === process.env.VERIFIED_PR_NUMBER && baseRef === process.env.VERIFIED_BASE_REF &&
  base.object?.sha === process.env.VERIFIED_BASE_SHA &&
  headRef === process.env.VERIFIED_HEAD_REF && headSha === process.env.VERIFIED_HEAD_SHA &&
  user.login === process.env.VERIFIED_AUTHOR_LOGIN &&
  String(user.id) === process.env.VERIFIED_AUTHOR_ID && user.type === process.env.VERIFIED_AUTHOR_TYPE &&
  bodySha256 === process.env.VERIFIED_BODY_SHA256 &&
  priorMatches;
process.exit(valid ? 0 : 1);
NODE
}

arm_verified_candidate() {
  local work_dir user_file base_file head_file pr_first pr_final
  work_dir="$(mktemp -d)" || fail 'unable to create auto-merge verification workspace'
  user_file="${work_dir}/user.json"; base_file="${work_dir}/base.json"
  head_file="${work_dir}/head.json"; pr_first="${work_dir}/pr-first.json"
  pr_final="${work_dir}/pr-final.json"
  trap 'rm -rf -- "${work_dir}"' RETURN
  read_bounded_api_json "users/${AERIS_SYNC_APP_SLUG}%5Bbot%5D" "${user_file}" ||
    fail 'unable to read authoritative Sync App identity'
  read_bounded_api_json "repos/${REPOSITORY}/git/ref/heads/${BASE_BRANCH}" "${base_file}" ||
    fail 'unable to read authoritative base ref'
  read_bounded_api_json "repos/${REPOSITORY}/git/ref/heads/${SYNC_BRANCH}" "${head_file}" ||
    fail 'unable to read authoritative synchronization ref'
  read_bounded_api_json "repos/${REPOSITORY}/pulls/${PR_NUMBER}" "${pr_first}" ||
    fail 'unable to read authoritative pull request'
  validate_arm_snapshot "${pr_first}" - "${user_file}" "${base_file}" "${head_file}" ||
    fail 'pull request is not the exact managed synchronization candidate'
  if [[ "${AERIS_AUTOMERGE_TEST_MODE:-false}" == true &&
        "${AERIS_AUTOMERGE_TEST_FIXTURE:-false}" == true &&
        -n "${AERIS_AUTOMERGE_BEFORE_FINAL_PR_READ_HOOK:-}" ]]; then
    "${AERIS_AUTOMERGE_BEFORE_FINAL_PR_READ_HOOK}" ||
      fail 'auto-merge final-read test hook failed'
  fi
  read_bounded_api_json "repos/${REPOSITORY}/pulls/${PR_NUMBER}" "${pr_final}" ||
    fail 'unable to reread authoritative pull request'
  validate_arm_snapshot "${pr_final}" "${pr_first}" "${user_file}" "${base_file}" "${head_file}" ||
    fail 'pull request changed before auto-merge arming'
  aeris_gh pr merge "${PR_NUMBER}" --repo "${REPOSITORY}" --auto --squash \
    --match-head-commit "${HEAD_SHA}"
}

[[ $# -ge 3 ]] || usage

ACTION="$1"
REPOSITORY="$2"
PR_VALUE="$3"
require_repo "${REPOSITORY}"
parse_pr "${PR_VALUE}"

case "${ACTION}" in
  arm)
    [[ $# -eq 4 ]] || usage
    HEAD_SHA="$4"
    [[ "${HEAD_SHA}" =~ ^[0-9A-Fa-f]{40}$ ]] ||
      fail 'head SHA must be a full 40-character hexadecimal commit SHA'
    : "${AERIS_SYNC_APP_SLUG:?AERIS_SYNC_APP_SLUG is required}"
    [[ "${AERIS_SYNC_APP_SLUG}" =~ ^[a-z0-9][a-z0-9-]{0,99}$ ]] ||
      fail 'AERIS_SYNC_APP_SLUG must be a lowercase GitHub App slug'
    : "${VERIFIED_PR_NUMBER:?VERIFIED_PR_NUMBER is required}"
    : "${VERIFIED_BASE_REF:?VERIFIED_BASE_REF is required}"
    : "${VERIFIED_BASE_SHA:?VERIFIED_BASE_SHA is required}"
    : "${VERIFIED_HEAD_REF:?VERIFIED_HEAD_REF is required}"
    : "${VERIFIED_HEAD_SHA:?VERIFIED_HEAD_SHA is required}"
    : "${VERIFIED_AUTHOR_LOGIN:?VERIFIED_AUTHOR_LOGIN is required}"
    : "${VERIFIED_AUTHOR_ID:?VERIFIED_AUTHOR_ID is required}"
    : "${VERIFIED_AUTHOR_TYPE:?VERIFIED_AUTHOR_TYPE is required}"
    : "${VERIFIED_BODY_SHA256:?VERIFIED_BODY_SHA256 is required}"
    arm_verified_candidate
    ;;
  disarm)
    [[ $# -eq 3 ]] || usage
    auto_merge_enabled="$(aeris_gh pr view "${PR_NUMBER}" --repo "${REPOSITORY}" \
      --json autoMergeRequest --jq '.autoMergeRequest != null')"
    case "${auto_merge_enabled}" in
      true)
        aeris_gh pr merge "${PR_NUMBER}" --repo "${REPOSITORY}" --disable-auto
        ;;
      false) ;;
      *) fail 'unable to determine whether automatic merge is enabled' ;;
    esac
    ;;
  *) usage ;;
esac
