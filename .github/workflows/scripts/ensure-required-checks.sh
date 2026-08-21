#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/github-autonomy.sh"

: "${GH_TOKEN:?GH_TOKEN must be the workflow job token with actions:write}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${SYNC_BRANCH:?SYNC_BRANCH is required}"
: "${SYNCED_SHA:?SYNCED_SHA is required}"
: "${PR_URL:?PR_URL is required}"

[[ "${SYNCED_SHA}" =~ ^[0-9A-Fa-f]{40}$ ]] || {
  echo 'error: SYNCED_SHA must be a full 40-character hexadecimal commit SHA' >&2
  exit 78
}
if [[ "${PR_URL}" =~ ^https://github\.com/([A-Za-z0-9][A-Za-z0-9._-]*)/([A-Za-z0-9][A-Za-z0-9._-]*)/pull/([1-9][0-9]*)/?$ ]]; then
  [[ "${BASH_REMATCH[1]}/${BASH_REMATCH[2]}" == "${GITHUB_REPOSITORY}" ]] || {
    echo 'error: PR_URL repository does not match GITHUB_REPOSITORY' >&2
    exit 78
  }
  PR_NUMBER="${BASH_REMATCH[3]}"
else
  echo 'error: PR_URL must be a GitHub pull request URL' >&2
  exit 78
fi

poll_attempts="${AERIS_CHECK_POLL_ATTEMPTS:-6}"
poll_seconds="${AERIS_CHECK_POLL_SECONDS:-5}"
wait_attempts="${AERIS_REQUIRED_CHECK_WAIT_ATTEMPTS:-180}"
wait_seconds="${AERIS_REQUIRED_CHECK_WAIT_SECONDS:-10}"
[[ "${poll_attempts}" =~ ^[1-9][0-9]*$ ]] || {
  echo 'error: AERIS_CHECK_POLL_ATTEMPTS must be a positive integer' >&2
  exit 78
}
[[ "${poll_seconds}" =~ ^[0-9]+$ ]] || {
  echo 'error: AERIS_CHECK_POLL_SECONDS must be a non-negative integer' >&2
  exit 78
}
[[ "${wait_attempts}" =~ ^[1-9][0-9]*$ ]] || {
  echo 'error: AERIS_REQUIRED_CHECK_WAIT_ATTEMPTS must be a positive integer' >&2
  exit 78
}
[[ "${wait_seconds}" =~ ^[0-9]+$ ]] || {
  echo 'error: AERIS_REQUIRED_CHECK_WAIT_SECONDS must be a non-negative integer' >&2
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

required_check_state() {
  local checks="$1"
  jq -r --arg head_sha "${SYNCED_SHA}" \
    --arg actions_prefix "https://github.com/${GITHUB_REPOSITORY}/actions/runs/" '
    def required_names:
      ["Automation Policy / gate", "Frontend CI / check", "Rust CI / check"];
    if type != "object" or (.total_count | type) != "number" or
       (.check_runs | type) != "array" or .total_count > 100 or
       .total_count != (.check_runs | length) then
      "invalid"
    else
      [required_names[] as $name |
        [.check_runs[] |
          select(.name == $name and .head_sha == $head_sha and
                 .app.id == 15368 and .app.slug == "github-actions")] |
        sort_by(.id // 0) | last
      ] as $latest |
      if any($latest[]; . == null) then
        "pending"
      elif any($latest[];
        (.id | type) != "number" or .id <= 0 or
        (.check_suite.id | type) != "number" or .check_suite.id <= 0 or
        (try (.details_url | startswith($actions_prefix)) catch false) != true) then
        "invalid"
      elif any($latest[]; .status == "completed" and .conclusion != "success") then
        "failed"
      elif all($latest[]; .status == "completed" and .conclusion == "success") then
        "success"
      elif all($latest[];
        (.status == "queued" or .status == "in_progress" or .status == "pending" or
         .status == "waiting" or .status == "requested") and .conclusion == null) then
        "pending"
      else
        "invalid"
      end
    end
  ' <<<"${checks}"
}

wait_for_required_checks() {
  local attempt pr checks state
  for ((attempt = 1; attempt <= wait_attempts; attempt++)); do
    pr="$(aeris_gh pr view "${PR_NUMBER}" --repo "${GITHUB_REPOSITORY}" \
      --json state,isDraft,headRefOid,headRefName,headRepository,baseRefName,autoMergeRequest)"
    jq -e --arg head_sha "${SYNCED_SHA}" --arg head_branch "${SYNC_BRANCH}" \
      --arg repository "${GITHUB_REPOSITORY}" '
      type == "object" and .state == "OPEN" and .isDraft == false and
      .headRefOid == $head_sha and .headRefName == $head_branch and
      .headRepository.nameWithOwner == $repository and .baseRefName == "main" and
      .autoMergeRequest == null
    ' <<<"${pr}" >/dev/null || {
      echo 'error: synchronization pull request identity or state drifted while waiting for checks' >&2
      exit 78
    }

    checks="$(aeris_gh api "repos/${GITHUB_REPOSITORY}/commits/${SYNCED_SHA}/check-runs?per_page=100")"
    if ! state="$(required_check_state "${checks}")"; then
      echo 'error: required check response was not valid JSON' >&2
      exit 78
    fi
    case "${state}" in
      success)
        echo 'all exact-head required checks completed successfully'
        return
        ;;
      failed)
        echo 'error: an exact-head required check completed unsuccessfully' >&2
        exit 1
        ;;
      pending) ;;
      *)
        echo 'error: required check response was incomplete or malformed' >&2
        exit 78
        ;;
    esac
    ((attempt == wait_attempts)) || sleep "${wait_seconds}"
  done
  echo 'error: timed out waiting for exact-head required checks' >&2
  exit 78
}

ensure_check rust-ci.yml "Rust CI / check"
ensure_check frontend-ci.yml "Frontend CI / check"
wait_for_required_checks
