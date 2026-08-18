#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/github-autonomy.sh"

: "${GH_TOKEN:?GH_TOKEN must be the workflow job token with actions:write}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${SYNC_BRANCH:?SYNC_BRANCH is required}"
: "${SYNCED_SHA:?SYNCED_SHA is required}"

poll_attempts="${AERIS_CHECK_POLL_ATTEMPTS:-6}"
poll_seconds="${AERIS_CHECK_POLL_SECONDS:-5}"
[[ "${poll_attempts}" =~ ^[1-9][0-9]*$ ]] || {
  echo 'error: AERIS_CHECK_POLL_ATTEMPTS must be a positive integer' >&2
  exit 78
}
[[ "${poll_seconds}" =~ ^[0-9]+$ ]] || {
  echo 'error: AERIS_CHECK_POLL_SECONDS must be a non-negative integer' >&2
  exit 78
}

ensure_check() {
  local workflow="$1" context="$2" attempt check_active status_active run_active
  for ((attempt = 1; attempt <= poll_attempts; attempt++)); do
    check_active="$(aeris_gh api "repos/${GITHUB_REPOSITORY}/commits/${SYNCED_SHA}/check-runs?per_page=100" \
      --jq "[.check_runs[] | select(.name == \"${context}\")] | sort_by(.started_at // .completed_at // .created_at) | last // {} | if .status == null then false elif (.status != \"completed\" or .conclusion == \"success\") then true else false end")"
    status_active="$(aeris_gh api "repos/${GITHUB_REPOSITORY}/commits/${SYNCED_SHA}/status" \
      --jq "[.statuses[] | select(.context == \"${context}\")] | sort_by(.created_at) | last // {} | if .state == \"success\" or .state == \"pending\" then true else false end")"
    run_active="$(aeris_gh api "repos/${GITHUB_REPOSITORY}/actions/workflows/${workflow}/runs?head_sha=${SYNCED_SHA}&per_page=100" \
      --jq '[.workflow_runs[] | select(.status != "completed" or .conclusion == "success")] | length > 0')"
    if [[ "${check_active}" == true || "${status_active}" == true || "${run_active}" == true ]]; then
      echo "${context}: an active or successful result already exists"
      return
    fi
    ((attempt == poll_attempts)) && break
    sleep "${poll_seconds}"
  done

  # The PR event normally wins during polling. Dispatch is the fallback for
  # App-token-created PRs that do not emit that event.
  aeris_gh workflow run --repo "${GITHUB_REPOSITORY}" "${workflow}" --ref "${SYNC_BRANCH}"
  echo "${context}: fallback workflow_dispatch created"
}

ensure_check rust-ci.yml "Rust CI / check"
ensure_check frontend-ci.yml "Frontend CI / check"
