#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/github-autonomy.sh"

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
    aeris_gh pr merge "${PR_NUMBER}" --repo "${REPOSITORY}" --auto --squash \
      --match-head-commit "${HEAD_SHA}"
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
