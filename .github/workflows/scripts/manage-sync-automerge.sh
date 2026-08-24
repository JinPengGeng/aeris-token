#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/github-autonomy.sh"

aeris_checks_gh() {
  aeris_require_active_autonomy_window || return
  GH_TOKEN="${AERIS_CHECKS_GH_TOKEN:?AERIS_CHECKS_GH_TOKEN is required}" command gh "$@"
}

usage() {
  printf '%s\n' 'usage: manage-sync-automerge.sh merge <owner/repo> <pr-number|pr-url> <head-sha> <base-sha> <source> <eligible|conflict_ai_review> [attestation-path attestation-sha] | disarm <owner/repo> <pr-number|pr-url>'
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
    [[ $# -eq 7 || $# -eq 9 ]] || usage
    HEAD_SHA="$4"
    BASE_SHA="$5"
    SYNC_SOURCE="$6"
    POLICY_VERDICT="$7"
    [[ "${HEAD_SHA}" =~ ^[0-9A-Fa-f]{40}$ ]] ||
      fail 'head SHA must be a full 40-character hexadecimal commit SHA'
    [[ "${BASE_SHA}" =~ ^[0-9A-Fa-f]{40}$ ]] ||
      fail 'base SHA must be a full 40-character hexadecimal commit SHA'
    [[ "${HEAD_SHA}" != "${BASE_SHA}" ]] || fail 'head and base SHA must differ'
    [[ "${SYNC_SOURCE}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*@[0-9A-Fa-f]{40}$ ]] ||
      fail 'source must be owner/repo@sha'
    [[ -n "${AERIS_CHECKS_GH_TOKEN:-}" ]] ||
      fail 'AERIS_CHECKS_GH_TOKEN is required for exact-head check reads'
    [[ "${POLICY_VERDICT}" == eligible || "${POLICY_VERDICT}" == conflict_ai_review ]] ||
      fail 'only an eligible or independently reviewed synchronization verdict permits direct merge'
    if [[ "${POLICY_VERDICT}" == conflict_ai_review ]]; then
      [[ $# -eq 9 ]] || fail 'AI conflict review requires an exact attestation artifact and hash'
      ATTESTATION_PATH="$8"
      ATTESTATION_SHA="$9"
      [[ "${ATTESTATION_SHA}" =~ ^[0-9a-f]{64}$ ]] || fail 'conflict attestation SHA is invalid'
      upstream_repository="${SYNC_SOURCE%@*}"
      upstream_sha="${SYNC_SOURCE#*@}"
      GITHUB_REPOSITORY="${REPOSITORY}" \
      AERIS_CONFLICT_PULL_NUMBER="${PR_NUMBER}" \
      AERIS_CONFLICT_HEAD_SHA="${HEAD_SHA}" \
      AERIS_CONFLICT_BASE_SHA="${BASE_SHA}" \
      AERIS_CONFLICT_UPSTREAM_REPOSITORY="${upstream_repository}" \
      AERIS_CONFLICT_UPSTREAM_SHA="${upstream_sha}" \
      AERIS_CONFLICT_ATTESTATION_PATH="${ATTESTATION_PATH}" \
      AERIS_CONFLICT_ATTESTATION_SHA="${ATTESTATION_SHA}" \
      AERIS_CONFLICT_RUN_ID="${GITHUB_RUN_ID:?GITHUB_RUN_ID is required for conflict review}" \
      AERIS_CONFLICT_RUN_ATTEMPT="${GITHUB_RUN_ATTEMPT:?GITHUB_RUN_ATTEMPT is required for conflict review}" \
        node "${SCRIPT_DIR}/../../automation/src/sync-conflict-review.mjs" verify-attestation ||
        fail 'AI conflict review attestation is invalid or stale'
    else
      [[ $# -eq 7 ]] || fail 'clean synchronization must not supply a conflict attestation'
    fi

    preflight_pr="$(aeris_gh api "repos/${REPOSITORY}/pulls/${PR_NUMBER}")"
    jq -e --argjson number "${PR_NUMBER}" --arg head_sha "${HEAD_SHA}" \
      --arg base_sha "${BASE_SHA}" --arg repository "${REPOSITORY}" \
      --arg head_branch "${SYNC_BRANCH:-automation/sync-upstream}" \
      --arg base_branch "${BASE_BRANCH:-main}" '
      type == "object" and .number == $number and .state == "open" and .merged == false and
      .draft == false and .head.sha == $head_sha and .head.ref == $head_branch and
      .head.repo.full_name == $repository and .base.ref == $base_branch and .base.sha == $base_sha and
      .auto_merge == null
    ' <<<"${preflight_pr}" >/dev/null ||
      fail 'pull request drifted before merge mutation'

    head_commit="$(aeris_gh api "repos/${REPOSITORY}/commits/${HEAD_SHA}")"
    jq -e --arg head_sha "${HEAD_SHA}" --arg base_sha "${BASE_SHA}" \
      --arg source "${SYNC_SOURCE}" --arg verdict "${POLICY_VERDICT}" '
      type == "object" and .sha == $head_sha and
      (.parents | type == "array" and length == 1 and .[0].sha == $base_sha) and
      (.commit.message | type == "string") and
      ([.commit.message | split("\n")[] | select(. == "Sync-Upstream-Automation: true")] | length == 1) and
      ([.commit.message | split("\n")[] | select(. == ("Sync-Upstream-Source: " + $source))] | length == 1) and
      ([.commit.message | split("\n")[] | select(. == ("Sync-Upstream-Base: " + $base_sha))] | length == 1) and
      ([.commit.message | split("\n")[] | select(. == ("Sync-Upstream-Policy-Verdict: " + $verdict))] | length == 1)
    ' <<<"${head_commit}" >/dev/null ||
      fail 'head commit does not prove the trusted synchronization source, base, and verdict'

    checks="$(aeris_checks_gh api "repos/${REPOSITORY}/commits/${HEAD_SHA}/check-runs?per_page=100")"
    jq -e --arg head_sha "${HEAD_SHA}" --arg actions_prefix "https://github.com/${REPOSITORY}/actions/runs/" '
      def required_names: ["Automation Policy / gate", "Frontend CI / check", "Rust CI / check"];
      type == "object" and (.total_count | type) == "number" and .total_count <= 100 and
      (.check_runs | type) == "array" and .total_count == (.check_runs | length) and
      ([required_names[] as $name |
        [.check_runs[] |
          select(.name == $name and .head_sha == $head_sha and
                 .app.id == 15368 and .app.slug == "github-actions")] |
        sort_by(.id // 0) | last
      ] | all(.[];
        . != null and (.id | type) == "number" and .id > 0 and
        (.check_suite.id | type) == "number" and .check_suite.id > 0 and
        .status == "completed" and .conclusion == "success" and
        (.details_url | type) == "string" and (.details_url | startswith($actions_prefix))))
    ' <<<"${checks}" >/dev/null ||
      fail 'exact-head required checks are missing, duplicated beyond the bounded snapshot, or unsuccessful'

    governance="$(aeris_gh api graphql \
      -f query='query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){mergeCommitAllowed rebaseMergeAllowed squashMergeAllowed isArchived isDisabled isLocked branchProtectionRules(first:100){totalCount pageInfo{hasNextPage} nodes{pattern allowsDeletions allowsForcePushes requiresStatusChecks requiresStrictStatusChecks isAdminEnforced requiresConversationResolution requiresLinearHistory bypassPullRequestAllowances(first:100){totalCount pageInfo{hasNextPage}} bypassForcePushAllowances(first:100){totalCount pageInfo{hasNextPage}} requiredStatusChecks{context app{databaseId slug}}}} rulesets(first:100,includeParents:true,targets:[BRANCH]){totalCount pageInfo{hasNextPage} nodes{enforcement target}} pullRequest(number:$number){number state isDraft mergeable mergeStateStatus headRefName headRefOid baseRefName baseRefOid headRepository{nameWithOwner} autoMergeRequest{enabledAt} reviewDecision reviewThreads(first:100){nodes{isResolved} pageInfo{hasNextPage}}}}}' \
      -f owner="${REPOSITORY%%/*}" -f name="${REPOSITORY#*/}" -F number="${PR_NUMBER}")"
    jq -e --argjson number "${PR_NUMBER}" --arg head_sha "${HEAD_SHA}" \
      --arg base_sha "${BASE_SHA}" --arg repository "${REPOSITORY}" \
      --arg head_branch "${SYNC_BRANCH:-automation/sync-upstream}" \
      --arg base_branch "${BASE_BRANCH:-main}" '
      def complete_zero_allowance:
        type == "object" and .totalCount == 0 and .pageInfo.hasNextPage == false;
      .data.repository as $repository_profile |
      .data.repository.branchProtectionRules.nodes[0] as $rule |
      .data.repository.pullRequest as $pr |
      $repository_profile.mergeCommitAllowed == false and
      $repository_profile.rebaseMergeAllowed == false and
      $repository_profile.squashMergeAllowed == true and
      $repository_profile.isArchived == false and
      $repository_profile.isDisabled == false and
      $repository_profile.isLocked == false and
      ($repository_profile.branchProtectionRules | type) == "object" and
      $repository_profile.branchProtectionRules.totalCount == 1 and
      $repository_profile.branchProtectionRules.pageInfo.hasNextPage == false and
      ($repository_profile.branchProtectionRules.nodes | type) == "array" and
      ($repository_profile.branchProtectionRules.nodes | length) == 1 and
      $rule.pattern == $base_branch and
      $rule.allowsDeletions == false and $rule.allowsForcePushes == false and
      $rule.requiresStatusChecks == true and $rule.requiresStrictStatusChecks == true and
      $rule.isAdminEnforced == true and $rule.requiresConversationResolution == true and
      $rule.requiresLinearHistory == true and
      ($rule.bypassPullRequestAllowances | complete_zero_allowance) and
      ($rule.bypassForcePushAllowances | complete_zero_allowance) and
      ($rule.requiredStatusChecks | type) == "array" and
      ($rule.requiredStatusChecks | length) == 3 and
      ([ $rule.requiredStatusChecks[].context ] | sort) ==
        (["Automation Policy / gate", "Frontend CI / check", "Rust CI / check"] | sort) and
      all($rule.requiredStatusChecks[];
        .app.databaseId == 15368 and .app.slug == "github-actions") and
      ($repository_profile.rulesets | type) == "object" and
      $repository_profile.rulesets.pageInfo.hasNextPage == false and
      ($repository_profile.rulesets.nodes | type) == "array" and
      $repository_profile.rulesets.totalCount == ($repository_profile.rulesets.nodes | length) and
      all($repository_profile.rulesets.nodes[];
        .target == "BRANCH" and
        (.enforcement == "DISABLED" or .enforcement == "EVALUATE")) and
      $pr.number == $number and $pr.state == "OPEN" and $pr.isDraft == false and
      $pr.mergeable == "MERGEABLE" and $pr.mergeStateStatus == "CLEAN" and
      $pr.headRefName == $head_branch and $pr.headRefOid == $head_sha and
      $pr.baseRefName == $base_branch and $pr.baseRefOid == $base_sha and
      $pr.headRepository.nameWithOwner == $repository and $pr.autoMergeRequest == null and
      ($pr.reviewDecision == null or $pr.reviewDecision == "APPROVED") and
      $pr.reviewThreads.pageInfo.hasNextPage == false and
      ($pr.reviewThreads.nodes | type == "array" and all(.[]; .isResolved == true))
    ' <<<"${governance}" >/dev/null ||
      fail 'pull request or branch protection governance drifted before merge mutation'

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
    jq -e --argjson number "${PR_NUMBER}" --arg head_sha "${HEAD_SHA}" \
      --arg base_sha "${BASE_SHA}" --arg writer_login "${AERIS_WRITER_APP_SLUG}[bot]" \
      --arg base_branch "${BASE_BRANCH:-main}" \
      'type == "object" and .number == $number and .state == "closed" and .merged == true and
       (.merged_at // "") != "" and .draft == false and .head.sha == $head_sha and
       .base.ref == $base_branch and .base.sha == $base_sha and
       (.auto_merge == null) and .merged_by.login == $writer_login and
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
    jq -e --arg base_sha "${BASE_SHA}" --arg merge_commit_sha "${merge_commit_sha}" \
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
