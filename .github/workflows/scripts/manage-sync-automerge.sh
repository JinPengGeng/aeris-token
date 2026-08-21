#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/github-autonomy.sh"

usage() {
  printf '%s\n' 'usage: manage-sync-automerge.sh <merge|disarm> <owner/repo> <pr-number|pr-url> [head-sha]'
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

[[ $# -ge 3 ]] || usage

ACTION="$1"
REPOSITORY="$2"
PR_VALUE="$3"
require_repo "${REPOSITORY}"
parse_pr "${PR_VALUE}"

case "${ACTION}" in
  merge)
    [[ $# -eq 4 ]] || usage
    HEAD_SHA="$4"
    [[ "${HEAD_SHA}" =~ ^[0-9A-Fa-f]{40}$ ]] ||
      fail 'head SHA must be a full 40-character hexadecimal commit SHA'
    set +e
    merge_response="$(aeris_gh api --method PUT \
      "repos/${REPOSITORY}/pulls/${PR_NUMBER}/merge" \
      -f merge_method=squash \
      -f sha="${HEAD_SHA}")"
    merge_status=$?

    # The endpoint response can be lost or malformed after the mutation. Always
    # perform one independent readback; it is the sole success criterion.
    response_valid=false
    if jq -e '.merged == true and (.sha | type == "string" and test("^[0-9a-fA-F]{40}$"))' \
      <<<"${merge_response}" >/dev/null 2>&1; then
      response_valid=true
    fi
    if [[ "${merge_status}" -ne 0 || "${response_valid}" != true ]]; then
      printf 'warning: merge response was not authoritative; relying on readback\n' >&2
    fi
    merged_pr="$(aeris_gh api "repos/${REPOSITORY}/pulls/${PR_NUMBER}")"
    readback_status=$?
    set -e
    if [[ "${readback_status}" -ne 0 ]]; then
      [[ "${readback_status}" -eq 78 ]] && exit 78
      fail 'unable to read back the pull request after merge mutation'
    fi
    merge_commit_sha="$(jq -r '.merge_commit_sha // empty' <<<"${merged_pr}")"
    base_sha="$(jq -r '.base.sha // empty' <<<"${merged_pr}")"
    jq -e --argjson number "${PR_NUMBER}" --arg head_sha "${HEAD_SHA}" \
      --arg writer_login "${AERIS_WRITER_APP_SLUG}[bot]" --arg base_branch "${BASE_BRANCH:-main}" \
      'type == "object" and .number == $number and .state == "closed" and .merged == true and
       (.merged_at // "") != "" and .draft == false and .head.sha == $head_sha and
       .base.ref == $base_branch and (.auto_merge == null) and .merged_by.login == $writer_login and
       (.merge_commit_sha | type == "string" and test("^[0-9a-fA-F]{40}$")) and
       .merge_commit_sha != $head_sha and (.base.sha | type == "string" and test("^[0-9a-fA-F]{40}$")) and
       .merge_commit_sha != .base.sha' \
      <<<"${merged_pr}" >/dev/null ||
      fail 'merged PR readback did not prove the exact expected outcome'
    set +e
    merge_commit="$(aeris_gh api "repos/${REPOSITORY}/commits/${merge_commit_sha}")"
    readback_status=$?
    set -e
    [[ "${readback_status}" -eq 0 ]] || {
      [[ "${readback_status}" -eq 78 ]] && exit 78
      fail 'unable to read back the squash merge commit'
    }
    jq -e --arg base_sha "${base_sha}" --arg merge_commit_sha "${merge_commit_sha}" \
      'type == "object" and .sha == $merge_commit_sha and
       (.parents | type == "array" and length == 1) and
       .parents[0].sha == $base_sha' <<<"${merge_commit}" >/dev/null ||
      fail 'merge commit readback did not prove a single-parent squash outcome'
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
