#!/usr/bin/env bash

set -euo pipefail

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"

BASE_BRANCH="${BASE_BRANCH:-main}"
SYNC_BRANCH="${SYNC_BRANCH:-automation/sync-upstream}"
RESUME="${RESUME:-false}"

MANAGED_MARKER='<!-- upstream-sync-managed -->'
AUTO_CLOSED_MARKER='<!-- upstream-sync-auto-closed -->'
BOT_LOGIN='github-actions[bot]'
BOT_EMAIL='41898282+github-actions[bot]@users.noreply.github.com'

repo_owner="${GITHUB_REPOSITORY%%/*}"
sync_prs='[]'
open_pr=''
open_prs='[]'
latest_pr=''
tracked_pr=''
resumed_closed_number=''
remote_sha=''
parent=''
upstream_branch=''
base_sha=''
upstream_sha=''

output() {
  printf '%s=%s\n' "$1" "$2" >>"${GITHUB_OUTPUT}"
}

list_sync_prs() {
  gh api --paginate --slurp \
    --method GET \
    "repos/${GITHUB_REPOSITORY}/pulls" \
    -f state=all \
    -f base="${BASE_BRANCH}" \
    -f head="${repo_owner}:${SYNC_BRANCH}" \
    -f sort=updated \
    -f direction=desc \
    -f per_page=100 |
    jq -c '[.[][] | {
      number,
      url:.html_url,
      state:(.state | ascii_upcase),
      mergedAt:.merged_at,
      closedAt:.closed_at,
      headRefOid:.head.sha,
      createdAt:.created_at,
      updatedAt:.updated_at,
      body:(.body // ""),
      author:(.user.login // "")
    }]'
}

refresh_prs() {
  local candidate
  sync_prs="$(list_sync_prs)"
  open_prs="$(jq -c '[.[] | select(.state == "OPEN")]' <<<"${sync_prs}")"
  if (("$(jq 'length' <<<"${open_prs}")" > 1)); then
    echo 'More than one open synchronization PR exists.' >&2
    return 1
  fi
  open_pr="$(jq -c '.[0] // empty' <<<"${open_prs}")"
  latest_pr=''
  while IFS= read -r candidate; do
    if pr_is_managed "${candidate}"; then
      latest_pr="${candidate}"
      break
    fi
  done < <(jq -c 'sort_by(.closedAt // .createdAt) | reverse[]' <<<"${sync_prs}")
  if [[ -n "${open_pr}" ]] && pr_is_managed "${open_pr}"; then
    latest_pr="${open_pr}"
  fi
}

pr_bot_comments() {
  gh api --paginate \
    "repos/${GITHUB_REPOSITORY}/issues/$1/comments?per_page=100" \
    --jq ".[] | select(.user.login == \"${BOT_LOGIN}\") | .body"
}

pr_comment_once() {
  local number="$1" key="$2" message="$3" marker comments
  marker="<!-- upstream-sync-${key} -->"
  comments="$(pr_bot_comments "${number}" || true)"
  if [[ "${comments}" != *"${marker}"* ]]; then
    gh pr comment --repo "${GITHUB_REPOSITORY}" "${number}" --body "${marker}
${message}" >/dev/null
  fi
}

# Authenticate the planned SHA before push so an interrupted publication can
# recover without treating commit author fields as an ownership boundary.
set_pending_tip() {
  local number="$1" sha="$2" comment_id body
  body="<!-- upstream-sync-pending-tip:${sha} -->
Prepared automation branch tip ${sha}."
  comment_id="$(gh api --paginate \
    "repos/${GITHUB_REPOSITORY}/issues/${number}/comments?per_page=100" \
    --jq ".[] | select(.user.login == \"${BOT_LOGIN}\" and (.body | startswith(\"<!-- upstream-sync-pending-tip:\"))) | .id" |
    tail -n1)"
  if [[ -n "${comment_id}" ]]; then
    gh api \
      --method PATCH \
      "repos/${GITHUB_REPOSITORY}/issues/comments/${comment_id}" \
      -f body="${body}" >/dev/null
  else
    gh pr comment --repo "${GITHUB_REPOSITORY}" "${number}" --body "${body}" >/dev/null
  fi
}

latest_close_actor() {
  gh api --paginate \
    "repos/${GITHUB_REPOSITORY}/issues/$1/events?per_page=100" \
    --jq '.[] | select(.event == "closed") | .actor.login' | tail -n1
}

pr_was_auto_closed() {
  local number="$1"
  [[ "$(latest_close_actor "${number}" || true)" == "${BOT_LOGIN}" ]] || return 1
  [[ "$(pr_bot_comments "${number}" || true)" == *"${AUTO_CLOSED_MARKER}"* ]]
}

source_from_pr() {
  local pr_json="$1" body source sha
  [[ -n "${pr_json}" ]] || return 0
  body="$(jq -r '.body // ""' <<<"${pr_json}")"
  source="$(sed -n 's/.*upstream-sync-source:\([^ ]*\).*/\1/p' <<<"${body}" | head -n1)"
  if [[ -n "${source}" ]]; then
    printf '%s\n' "${source}"
    return 0
  fi
  sha="$(grep -oE '[0-9a-f]{40}' <<<"${body}" | head -n1 || true)"
  [[ -z "${sha}" ]] || printf '%s@%s\n' "${parent}" "${sha}"
}

owned_tip_from_pr() {
  jq -r '.body // ""' <<<"$1" |
    sed -n 's/.*upstream-sync-owned-tip:\([0-9a-f]\{40\}\).*/\1/p' |
    head -n1
}

pr_is_managed() {
  local body author
  body="$(jq -r '.body // ""' <<<"$1")"
  author="$(jq -r '.author // ""' <<<"$1")"
  [[ "${author}" == "${BOT_LOGIN}" || "${author}" == app/github-actions ]] || return 1
  [[ "${body}" == *"${MANAGED_MARKER}"* ||
     "${body}" == *'Automated synchronization from '* ]]
}

latest_manual_pause_pr() {
  local candidate closed_pr number
  candidate=''
  while IFS= read -r closed_pr; do
    if pr_is_managed "${closed_pr}"; then
      candidate="${closed_pr}"
      break
    fi
  done < <(jq -c '[.[] | select(.state == "CLOSED")] | sort_by(.closedAt // .createdAt) | reverse[]' <<<"${sync_prs}")
  [[ -n "${candidate}" ]] || return 0
  [[ -z "$(jq -r '.mergedAt // empty' <<<"${candidate}")" ]] || return 0
  number="$(jq -r '.number' <<<"${candidate}")"
  [[ "${number}" != "${resumed_closed_number}" ]] || return 0
  pr_was_auto_closed "${number}" && return 0
  printf '%s\n' "${candidate}"
}

is_automation_commit() {
  local sha="$1" current_base="$2"
  local subject body author committer source automation base_trailer actual_parent
  subject="$(git show -s --format=%s "${sha}")"
  body="$(git show -s --format=%B "${sha}")"
  author="$(git show -s --format=%ae "${sha}")"
  committer="$(git show -s --format=%ce "${sha}")"
  source="$(sed -n 's/^Sync-Upstream-Source: //p' <<<"${body}" | tail -n1)"
  automation="$(sed -n 's/^Sync-Upstream-Automation: //p' <<<"${body}" | tail -n1)"
  base_trailer="$(sed -n 's/^Sync-Upstream-Base: //p' <<<"${body}" | tail -n1)"
  [[ "$(git rev-list --parents -n1 "${sha}" | wc -w)" -eq 2 ]] || return 1
  actual_parent="$(git rev-parse "${sha}^")"
  [[ "${author}" == "${BOT_EMAIL}" && "${committer}" == "${BOT_EMAIL}" ]] || return 1
  [[ "${automation}" == true && "${source}" == "${parent}@"* ]] || return 1
  [[ "${source##*@}" =~ ^[0-9a-f]{40}$ ]] || return 1
  [[ "${subject}" == "chore: sync ${source}" ]] || return 1
  [[ "${base_trailer}" == "${actual_parent}" ]] || return 1
  git merge-base --is-ancestor "${actual_parent}" "${current_base}"
}

is_legacy_tip() {
  local sha="$1" current_base="$2" pr_json="$3"
  local source source_sha subject author committer actual_parent
  [[ -n "${pr_json}" && "$(jq -r '.headRefOid' <<<"${pr_json}")" == "${sha}" ]] || return 1
  source="$(source_from_pr "${pr_json}")"
  [[ "${source}" == "${parent}@"* && "${source##*@}" =~ ^[0-9a-f]{40}$ ]] || return 1
  source_sha="${source##*@}"
  subject="$(git show -s --format=%s "${sha}")"
  author="$(git show -s --format=%ae "${sha}")"
  committer="$(git show -s --format=%ce "${sha}")"
  [[ "$(git rev-list --parents -n1 "${sha}" | wc -w)" -eq 2 ]] || return 1
  actual_parent="$(git rev-parse "${sha}^")"
  [[ "${author}" == "${BOT_EMAIL}" && "${committer}" == "${BOT_EMAIL}" ]] || return 1
  [[ "${subject}" == "chore: sync ${parent}@${source_sha}" ||
     "${subject}" == "chore: sync ${parent}@${source_sha:0:12}" ]] || return 1
  git merge-base --is-ancestor "${actual_parent}" "${current_base}"
}

fetch_remote_tip() {
  remote_sha="$(git ls-remote --heads origin "refs/heads/${SYNC_BRANCH}" | awk '{print $1}')"
  if [[ -n "${remote_sha}" ]]; then
    git fetch --no-tags origin \
      "+refs/heads/${SYNC_BRANCH}:refs/remotes/origin/${SYNC_BRANCH}"
  fi
}

remote_tip_owned() {
  local current_base="$1" reference_pr owned_tip number comments
  [[ -z "${remote_sha}" ]] && return 0
  reference_pr="${open_pr:-${latest_pr}}"
  owned_tip="$(owned_tip_from_pr "${reference_pr}")"
  if [[ -n "${owned_tip}" ]]; then
    [[ "${remote_sha}" == "${owned_tip}" ]] && return 0
    number="$(jq -r '.number' <<<"${reference_pr}")"
    comments="$(pr_bot_comments "${number}" || true)"
    [[ "${comments}" == *"<!-- upstream-sync-pending-tip:${remote_sha} -->"* ]]
    return
  fi
  is_legacy_tip "${remote_sha}" "${current_base}" "${reference_pr}" ||
    is_automation_commit "${remote_sha}" "${current_base}"
}

assert_remote_owned() {
  if ! remote_tip_owned "$1"; then
    echo "Refusing to overwrite unrecognized synchronization branch tip ${remote_sha}." >&2
    return 1
  fi
}

pause_or_resume() {
  local paused number current_number attempt
  refresh_prs
  if [[ -n "${open_pr}" ]]; then
    pr_is_managed "${open_pr}" || {
      echo 'A non-automation PR uses the reserved synchronization branch.' >&2
      return 1
    }
    tracked_pr="${open_pr}"
    return 0
  fi
  paused="$(latest_manual_pause_pr)"
  [[ -n "${paused}" ]] || return 0
  number="$(jq -r '.number' <<<"${paused}")"
  if [[ "${RESUME}" != true ]]; then
    echo "Synchronization is paused because PR #${number} was closed without merge."
    output state paused
    output has_changes false
    return 20
  fi
  for attempt in 1 2; do
    if gh pr reopen --repo "${GITHUB_REPOSITORY}" "${number}" >/dev/null 2>&1; then
      refresh_prs
      [[ -n "${open_pr}" && "$(jq -r '.number' <<<"${open_pr}")" == "${number}" ]] || return 1
      tracked_pr="${open_pr}"
      return 0
    fi
    ((attempt == 2)) || sleep 2
  done

  refresh_prs
  if [[ -n "${open_pr}" ]]; then
    current_number="$(jq -r '.number' <<<"${open_pr}")"
    if [[ "${current_number}" == "${number}" ]] && pr_is_managed "${open_pr}"; then
      tracked_pr="${open_pr}"
      return 0
    fi
    echo 'The open synchronization PR changed while resuming.' >&2
    return 1
  fi
  resumed_closed_number="${number}"
  echo "PR #${number} could not be reopened; this explicit run may create one replacement PR."
}

gate() {
  local paused current_number tracked_number
  refresh_prs
  if [[ -n "${tracked_pr}" ]]; then
    tracked_number="$(jq -r '.number' <<<"${tracked_pr}")"
    if [[ -n "${open_pr}" ]]; then
      current_number="$(jq -r '.number' <<<"${open_pr}")"
      [[ "${current_number}" == "${tracked_number}" ]] || {
        echo 'The open synchronization PR changed during the run.' >&2
        return 1
      }
      pr_is_managed "${open_pr}" || {
        echo 'The tracked synchronization PR is no longer automation-managed.' >&2
        return 1
      }
      tracked_pr="${open_pr}"
      return 0
    fi
  elif [[ -n "${open_pr}" ]]; then
    pr_is_managed "${open_pr}" || {
      echo 'A non-automation PR uses the reserved synchronization branch.' >&2
      return 1
    }
    tracked_pr="${open_pr}"
    return 0
  fi

  paused="$(latest_manual_pause_pr)"
  if [[ -n "${paused}" ]]; then
    echo "Synchronization paused after PR #$(jq -r '.number' <<<"${paused}") was closed."
    output state paused
    output has_changes false
    return 20
  fi
  tracked_pr=''
}

require_gate() {
  if gate; then
    return 0
  fi
  local rc=$?
  ((rc == 20)) && exit 0
  exit "${rc}"
}

report_workflow_drift() {
  local current_tree previous_source changed title existing
  current_tree="$(git rev-parse "${upstream_sha}:.github/workflows" 2>/dev/null || printf absent)"
  previous_source="$(source_from_pr "${latest_pr}")"
  [[ "${previous_source}" != "${parent}@${upstream_sha}" ]] || return 0

  if [[ "${previous_source}" == "${parent}@"* ]]; then
    previous_sha="${previous_source##*@}"
    git cat-file -e "${previous_sha}^{commit}" 2>/dev/null ||
      git fetch --no-tags upstream "${previous_sha}" >/dev/null 2>&1 || true
  else
    previous_sha=''
  fi

  if [[ -n "${previous_sha}" ]] && git cat-file -e "${previous_sha}^{commit}" 2>/dev/null; then
    changed="$(git diff --name-only "${previous_sha}" "${upstream_sha}" -- .github/workflows || true)"
  else
    changed="$(git diff --name-only "${base_sha}" "${upstream_sha}" -- .github/workflows || true)"
  fi
  [[ -n "${changed}" ]] || return 0

  title="[sync-upstream] Review upstream workflow tree ${current_tree:0:12}"
  existing="$(gh issue list \
    --repo "${GITHUB_REPOSITORY}" \
    --state all \
    --limit 100 \
    --search "\"${title}\" in:title" \
    --json title \
    --jq ".[] | select(.title == \"${title}\") | .title" | head -n1)"
  if [[ -z "${existing}" ]]; then
    gh issue create \
      --repo "${GITHUB_REPOSITORY}" \
      --title "${title}" \
      --body "<!-- upstream-sync-workflow-tree:${current_tree} -->
Upstream changed fork-owned workflow files. Review these paths manually:

${changed}" >/dev/null
  fi
  echo "::warning title=Upstream workflow drift::${changed//$'\n'/, }"
}

close_obsolete_pr() {
  require_gate
  if [[ -n "${tracked_pr}" ]]; then
    local number tip
    number="$(jq -r '.number' <<<"${tracked_pr}")"
    tip="$(jq -r '.headRefOid' <<<"${tracked_pr}")"
    fetch_remote_tip
    [[ -n "${remote_sha}" && "${remote_sha}" == "${tip}" ]] || {
      echo 'Obsolete PR head no longer matches the synchronization branch.' >&2
      exit 1
    }
    assert_remote_owned "${base_sha}"
    pr_comment_once "${number}" auto-closed 'Closed automatically because the base branch already contains the applicable upstream content.'
    gh pr close --repo "${GITHUB_REPOSITORY}" "${number}" >/dev/null
  fi
  output state noop
  output has_changes false
  exit 0
}

publish_pr() {
  local body number pr_url create_error create_output view_data
  require_gate
  fetch_remote_tip
  [[ "${remote_sha}" == "${published_sha}" ]] || {
    echo 'Synchronization branch moved before PR publication.' >&2
    return 1
  }

  body="${MANAGED_MARKER}
<!-- upstream-sync-owned-tip:${published_sha} -->
<!-- upstream-sync-source:${parent}@${upstream_sha} -->
Automated synchronization from ${parent}:${upstream_branch} at ${upstream_sha}.

This pull request requires protected branch checks and a maintainer decision.
Fork-owned files under .github/workflows are preserved for separate review."

  if [[ -n "${tracked_pr}" ]]; then
    number="$(jq -r '.number' <<<"${tracked_pr}")"
    pr_url="$(jq -r '.url' <<<"${tracked_pr}")"
    [[ "$(jq -r '.headRefOid' <<<"${tracked_pr}")" == "${published_sha}" ]] || return 1
  else
    create_error="$(mktemp)"
    if create_output="$(gh pr create \
      --repo "${GITHUB_REPOSITORY}" \
      --base "${BASE_BRANCH}" \
      --head "${SYNC_BRANCH}" \
      --title 'chore: sync upstream' \
      --body "${body}" 2>"${create_error}")"; then
      pr_url="$(tail -n1 <<<"${create_output}")"
    else
      refresh_prs
      if [[ "$(jq 'length' <<<"${open_prs}")" -eq 1 ]] && pr_is_managed "${open_pr}"; then
        tracked_pr="${open_pr}"
        pr_url="$(jq -r '.url' <<<"${tracked_pr}")"
      else
        cat "${create_error}" >&2
        return 1
      fi
    fi
    view_data="$(gh pr view --repo "${GITHUB_REPOSITORY}" "${pr_url}" --json state,headRefOid)"
    [[ "$(jq -r '.state' <<<"${view_data}")" == OPEN &&
       "$(jq -r '.headRefOid' <<<"${view_data}")" == "${published_sha}" ]] || return 1
  fi

  gh pr edit \
    --repo "${GITHUB_REPOSITORY}" \
    "${pr_url}" \
    --title 'chore: sync upstream' \
    --body "${body}"
  refresh_prs
  [[ "$(jq 'length' <<<"${open_prs}")" -eq 1 &&
     "$(jq -r '.headRefOid' <<<"${open_pr}")" == "${published_sha}" ]] || return 1
  output pr_url "${pr_url}"
}

parent="$(gh api "repos/${GITHUB_REPOSITORY}" --jq '.parent.full_name // empty')"
[[ -n "${parent}" ]] || {
  echo "${GITHUB_REPOSITORY} is not a fork." >&2
  exit 1
}
upstream_branch="$(gh api "repos/${parent}" --jq '.default_branch')"
output parent "${parent}"
output upstream_branch "${upstream_branch}"

git remote remove upstream >/dev/null 2>&1 || true
git remote add upstream "https://github.com/${parent}.git"
git config user.name 'github-actions[bot]'
git config user.email "${BOT_EMAIL}"

if pause_or_resume; then
  :
else
  rc=$?
  ((rc == 20)) && exit 0
  exit "${rc}"
fi

for attempt in 1 2 3; do
  git fetch --no-tags origin "${BASE_BRANCH}"
  git fetch --no-tags upstream "${upstream_branch}"
  base_sha="$(git rev-parse "origin/${BASE_BRANCH}")"
  upstream_sha="$(git rev-parse "upstream/${upstream_branch}")"
  output upstream_sha "${upstream_sha}"

  refresh_prs
  fetch_remote_tip
  expected_remote_sha="${remote_sha}"
  assert_remote_owned "${base_sha}"
  report_workflow_drift

  if git merge-base --is-ancestor "${upstream_sha}" "${base_sha}"; then
    close_obsolete_pr
  fi

  git switch --force-create "${SYNC_BRANCH}" "${base_sha}"
  merge_failed=false
  original_conflicts=''
  if ! git merge --squash "${upstream_sha}"; then
    merge_failed=true
    original_conflicts="$(git diff --name-only --diff-filter=U)"
  fi
  git restore --source="${base_sha}" --staged --worktree -- .github/workflows
  remaining_conflicts="$(git diff --name-only --diff-filter=U)"
  if [[ "${merge_failed}" == true ]] &&
     { [[ -z "${original_conflicts}" ]] || [[ -n "${remaining_conflicts}" ]]; }; then
    if [[ -n "${tracked_pr}" ]]; then
      pr_comment_once \
        "$(jq -r '.number' <<<"${tracked_pr}")" \
        "conflict:${upstream_sha}" \
        "Upstream ${upstream_sha} conflicts with base ${base_sha}. The PR and branch were preserved."
    fi
    output state conflict
    output has_changes false
    exit 1
  fi

  git diff --cached --quiet && close_obsolete_pr

  git commit \
    -m "chore: sync ${parent}@${upstream_sha}" \
    -m 'Sync-Upstream-Automation: true' \
    -m "Sync-Upstream-Source: ${parent}@${upstream_sha}" \
    -m "Sync-Upstream-Base: ${base_sha}"
  local_sha="$(git rev-parse HEAD)"

  git fetch --no-tags origin "${BASE_BRANCH}"
  git fetch --no-tags upstream "${upstream_branch}"
  if [[ "${base_sha}" != "$(git rev-parse "origin/${BASE_BRANCH}")" ||
        "${upstream_sha}" != "$(git rev-parse "upstream/${upstream_branch}")" ]]; then
    continue
  fi

  require_gate
  fetch_remote_tip
  [[ "${remote_sha}" == "${expected_remote_sha}" ]] || continue
  assert_remote_owned "${base_sha}"

  if [[ -n "${remote_sha}" ]] && git diff --quiet "${remote_sha}" "${local_sha}"; then
    published_sha="${remote_sha}"
  else
    reference_pr="${tracked_pr:-${latest_pr}}"
    if [[ -n "${reference_pr}" ]]; then
      set_pending_tip "$(jq -r '.number' <<<"${reference_pr}")" "${local_sha}"
    fi
    if [[ -n "${remote_sha}" ]]; then
      git push \
        --force-with-lease="refs/heads/${SYNC_BRANCH}:${remote_sha}" \
        origin "${local_sha}:refs/heads/${SYNC_BRANCH}"
    else
      git push \
        --force-with-lease="refs/heads/${SYNC_BRANCH}:" \
        origin "${local_sha}:refs/heads/${SYNC_BRANCH}"
    fi
    published_sha="${local_sha}"
  fi

  fetch_remote_tip
  [[ "${remote_sha}" == "${published_sha}" ]] || exit 1
  publish_pr
  output state published
  output has_changes true
  output synced_sha "${published_sha}"
  exit 0
done

output state unstable
output has_changes false
echo 'Base, upstream, or synchronization branch moved during all rebuild attempts.' >&2
exit 1
