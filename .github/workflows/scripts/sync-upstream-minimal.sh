#!/usr/bin/env bash

# Minimal upstream synchronization loop — automation v2 Phase 0 (#175).
#
# One linear, fail-closed pass per run:
#   bounded fetch origin/main + upstream/main (exact refs, SHA-pinned)
#   → progress = git DAG: skip when upstream tip is already an ancestor of main
#   → three-way merge onto the fixed branch sync/upstream (rebuilt when the
#     delete-branch-on-merge setting removed it)
#   → reuse or create the single managed sync PR (reuse key: upstream SHA
#     marker in the PR body + fixed head/base; rerunning at the same upstream
#     SHA never creates a duplicate PR)
#   → dispatch the required-check workflows (GITHUB_TOKEN-created PRs emit no
#     pull_request event, so rust-ci/frontend-ci/automation-policy need an
#     explicit dispatch onto the sync branch)
#   → arm GitHub native auto-merge (merge method) on the conflict-free PR
#   → bounded wait, then verify the merge landed correctly.
#
# Merge-method discipline (patch-policy §3): the managed sync PR MUST enter
# main as a merge commit with exactly two parents — never squash or rebase.
# Squash would sever ancestor connectivity and corrupt merge-base progress.
# After the auto-merge lands, this script re-fetches origin/main and requires
# `git rev-list --count <main>..<upstream>` to be 0 and the recorded merge
# commit to have exactly two parents; any drift opens a merge-discipline
# alert and the loop stops.
#
# Every alert carries the raw failing output verbatim (#172 lesson: opaque
# alerts forced maintainers to dig through run logs). Alerts are idempotent:
# one open issue per (kind, key), repeat failures add a single guarded
# comment.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# All network git transport goes through the shared bounded, fail-closed
# helper; unbounded fetch/ls-remote/push invocations are forbidden here.
source "${SCRIPT_DIR}/bounded-git-fetch.sh"

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
: "${GH_TOKEN:?GH_TOKEN must be the workflow job GITHUB_TOKEN}"
: "${UPSTREAM_REPOSITORY:?UPSTREAM_REPOSITORY is required (owner/name)}"
[[ "${UPSTREAM_REPOSITORY}" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo 'error: UPSTREAM_REPOSITORY must be owner/name' >&2
  exit 78
}

BASE_BRANCH="${BASE_BRANCH:-main}"
SYNC_BRANCH="${SYNC_BRANCH:-sync/upstream}"
SYNC_POLICY_FILE="${SYNC_POLICY_FILE:-.github/upstream-sync-policy.yml}"
ORIGIN_URL="https://github.com/${GITHUB_REPOSITORY}.git"
UPSTREAM_URL="https://github.com/${UPSTREAM_REPOSITORY}.git"
REPO_OWNER="${GITHUB_REPOSITORY%%/*}"
MANAGED_MARKER='<!-- upstream-sync-minimal-managed -->'
WAIT_ATTEMPTS="${AERIS_SYNC_WAIT_ATTEMPTS:-50}"
WAIT_SECONDS="${AERIS_SYNC_WAIT_SECONDS:-60}"
GITHUB_API_PAGE_BYTES=2097152
REQUIRED_CONTEXTS=("Rust CI / check" "Frontend CI / check" "Automation Policy / gate")

bounded_git() {
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git "$@"
}

# gh is a Go binary: the deadline runner keeps the hard timeout and file
# bounds without the virtual-memory ceiling its runtime cannot start under.
bounded_gh() {
  aeris_bounded_run_deadline "${GITHUB_API_PAGE_BYTES}" gh "$@"
}

output() {
  printf '%s=%s\n' "$1" "$2" >>"${GITHUB_OUTPUT}"
}

summary() {
  [[ -n "${GITHUB_STEP_SUMMARY:-}" ]] || return 0
  printf '%s\n' "$1" >>"${GITHUB_STEP_SUMMARY}"
}

# Bound externally-derived text before embedding it into an alert body.
cap_text() {
  local text="$1" max="$2"
  if ((${#text} > max)); then
    printf '%s\n…(truncated)\n' "${text:0:max}"
  else
    printf '%s\n' "${text}"
  fi
}

# report_sync_alert <kind> <key> <summary> <raw>
# Idempotent: one open issue per (kind, key), identified by title plus an HTML
# marker; a repeat failure adds exactly one marker-guarded comment. <raw> is
# mandatory and must carry the original failing output / exit code.
#
# This function is the last-resort channel: a failure of its own gh calls can
# itself only surface in the run log. Every internal call therefore carries an
# explicit `--method` (gh api defaults to POST when -f fields are present — a
# missing `--method GET` turned reads into create attempts, see #180 first-run
# 422) and is guarded so the raw diagnostic always reaches stderr before the
# function fails.
report_sync_alert() {
  local kind="$1" key="$2" summary="$3" raw="$4"
  local title marker comment_marker body existing comments gh_error
  [[ -n "${raw}" ]] || {
    echo 'error: report_sync_alert requires the raw error detail' >&2
    return 1
  }
  title="[sync-upstream] ${kind}: ${key}"
  marker="<!-- upstream-sync-alert:${kind}:${key} -->"
  comment_marker="<!-- upstream-sync-alert-comment:${kind}:${key} -->"
  body="${marker}
${summary}

### Raw error

\`\`\`
$(cap_text "${raw}" 8000)
\`\`\`"
  if ! existing="$(bounded_gh issue list \
    --repo "${GITHUB_REPOSITORY}" \
    --state open \
    --limit 100 \
    --search "\"${title}\" in:title" \
    --json number,title,body \
    --jq ".[] | select(.title == \"${title}\" and ((.body // \"\") | contains(\"${marker}\"))) | .number" 2>&1)"; then
    printf 'error: alert issue inventory failed for %s\n%s\n' "'${title}'" "${existing}" >&2
    return 1
  fi
  existing="$(head -n1 <<<"${existing}")"
  if [[ -z "${existing}" ]]; then
    if ! gh_error="$(bounded_gh issue create \
      --repo "${GITHUB_REPOSITORY}" \
      --title "${title}" \
      --body "${body}" 2>&1 >/dev/null)"; then
      printf 'error: alert issue create failed for %s\n%s\n' "'${title}'" "${gh_error}" >&2
      return 1
    fi
    return 0
  fi
  if ! comments="$(bounded_gh api --method GET \
    "repos/${GITHUB_REPOSITORY}/issues/${existing}/comments" \
    -f per_page=100 --jq '.[].body' 2>&1)"; then
    printf 'error: alert comment inventory failed for issue %s\n%s\n' "${existing}" "${comments}" >&2
    return 1
  fi
  if [[ "${comments}" != *"${comment_marker}"* ]]; then
    if ! gh_error="$(bounded_gh api --method POST \
      "repos/${GITHUB_REPOSITORY}/issues/${existing}/comments" \
      -f body="${comment_marker}
${summary}

### Raw error

\`\`\`
$(cap_text "${raw}" 8000)
\`\`\`" 2>&1 >/dev/null)"; then
      printf 'error: alert comment create failed for issue %s\n%s\n' "${existing}" "${gh_error}" >&2
      return 1
    fi
  fi
}

# Push through an askpass credential helper so the token never appears on the
# command line. Only ever pushes to origin (this fork); the upstream remote is
# strictly read-only.
bounded_git_push() {
  local askpass_dir askpass status=0
  askpass_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/aeris-sync-askpass.XXXXXX")"
  askpass="${askpass_dir}/askpass.sh"
  if ! (
    umask 077
    cat >"${askpass}" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?}" ;;
  *) exit 1 ;;
esac
EOF
    chmod 700 "${askpass}"
  ); then
    rm -f -- "${askpass}"
    rmdir -- "${askpass_dir}"
    return 1
  fi
  GIT_ASKPASS="${askpass}" GIT_ASKPASS_REQUIRE=force GIT_TERMINAL_PROMPT=0 \
    aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git -c credential.helper= -c http.https://github.com/.extraheader= "$@" || status=$?
  rm -f -- "${askpass}" || status=1
  rmdir -- "${askpass_dir}" || status=1
  return "${status}"
}

marker_of() {
  sed -n 's/.*upstream-sync-minimal-upstream:\([0-9a-f]\{40\}\).*/\1/p' | head -n1
}

pr_body() {
  cat <<EOF
${MANAGED_MARKER}
<!-- upstream-sync-minimal-upstream:${upstream_sha} -->
Automated synchronization from ${UPSTREAM_REPOSITORY}:main at ${upstream_sha} onto ${BASE_BRANCH} ${base_sha}.

Progress is the git DAG (merge-base), not a state file. This PR must land as a **merge commit** — never squash or rebase it, or ancestor connectivity breaks and the loop stops with a discipline alert.

The merge was computed locally before publication and is conflict-free by construction. GitHub native auto-merge (merge method) is armed and the required checks gate the merge. Conflicts and anomalies stop the loop and open an idempotent alert issue carrying the raw error output.
EOF
}

# fetch_exact <url> <branch> <destination> <label> [optional]
# Prints the exact remote SHA on stdout; empty output means an optional ref is
# absent. Any transport or validation failure returns non-zero with the
# helper's diagnostic on stderr.
fetch_exact() {
  local url="$1" branch="$2" destination="$3" label="$4" optional="${5:-false}"
  local sha
  aeris_bounded_read_remote_ref "${url}" "refs/heads/${branch}" "${label}" "${optional}" || return 1
  sha="${AERIS_BOUNDED_REMOTE_SHA}"
  if [[ -z "${sha}" ]]; then
    printf '%s\n' ''
    return 0
  fi
  aeris_bounded_fetch_ref "${url}" "refs/heads/${branch}" "${sha}" "${destination}" "${label}" || return 1
  printf '%s\n' "${sha}"
}

# context_state <head_sha> <context> → success | pending | failed | absent
# A failed query is reported as pending (conservative: keep waiting).
context_state() {
  local head_sha="$1" context="$2" checks statuses
  checks="$(bounded_gh api "repos/${GITHUB_REPOSITORY}/commits/${head_sha}/check-runs?per_page=100" \
    --jq "[.check_runs[] | select(.name == \"${context}\")] | sort_by(.id) | last // {} |
      if .status == null then \"absent\"
      elif .status == \"completed\" and .conclusion == \"success\" then \"success\"
      elif .status == \"completed\" then \"failed\"
      else \"pending\" end" 2>/dev/null || printf 'pending')"
  statuses="$(bounded_gh api "repos/${GITHUB_REPOSITORY}/commits/${head_sha}/status" \
    --jq "[.statuses[] | select(.context == \"${context}\")] | sort_by(.created_at) | last // {} |
      if .state == null then \"absent\"
      elif .state == \"success\" then \"success\"
      elif .state == \"pending\" then \"pending\"
      else \"failed\" end" 2>/dev/null || printf 'pending')"
  if [[ "${checks}" == success || "${statuses}" == success ]]; then
    printf 'success\n'
  elif [[ "${checks}" == failed || "${statuses}" == failed ]]; then
    printf 'failed\n'
  elif [[ "${checks}" == pending || "${statuses}" == pending ]]; then
    printf 'pending\n'
  else
    printf 'absent\n'
  fi
}

# GITHUB_TOKEN-created PRs emit no pull_request event, so the required check
# contexts would never appear on their own. rust-ci.yml, frontend-ci.yml, and
# automation-policy.yml accept workflow_dispatch and publish their context onto
# the head SHA; dispatch them only when no active or successful result exists
# yet. Extra arguments are passed through to `gh workflow run` as workflow
# inputs: the policy gate pins the exact pull request number and the trusted
# policy SHA (the current main tip this run already validated), and its
# evaluation re-validates both against the live API and fails closed on drift.
ensure_check_dispatch() {
  local workflow="$1" context="$2" attempt state dispatch_output
  shift 2
  for ((attempt = 1; attempt <= 6; attempt += 1)); do
    state="$(context_state "${head_sha}" "${context}")"
    if [[ "${state}" == success || "${state}" == pending ]]; then
      echo "${context}: an active or successful result already exists"
      return 0
    fi
    if [[ "${state}" == failed ]]; then
      echo "${context}: latest result failed; dispatching a fresh run"
      break
    fi
    ((attempt == 6)) || sleep 5
  done
  if ! dispatch_output="$(bounded_gh workflow run --repo "${GITHUB_REPOSITORY}" \
      "${workflow}" --ref "${SYNC_BRANCH}" "$@" 2>&1)"; then
    summary_msg="Unable to dispatch ${workflow} for ${SYNC_BRANCH}@${head_sha}. Auto-merge cannot proceed without the required check."
    raw="context=${context}
${dispatch_output}"
    report_sync_alert check-dispatch "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  echo "${context}: workflow_dispatch created"
}

alert_key=''
base_sha=''
upstream_sha=''
head_sha=''
pr_number=''
pr_url=''

aeris_bounded_fetch_init "${SYNC_POLICY_FILE}"

[[ "${WAIT_ATTEMPTS}" =~ ^[1-9][0-9]*$ ]] || {
  echo 'error: AERIS_SYNC_WAIT_ATTEMPTS must be a positive integer' >&2
  exit 78
}
[[ "${WAIT_SECONDS}" =~ ^[0-9]+$ ]] || {
  echo 'error: AERIS_SYNC_WAIT_SECONDS must be a non-negative integer' >&2
  exit 78
}

fetch_err="$(mktemp)"
if ! base_sha="$(fetch_exact "${ORIGIN_URL}" "${BASE_BRANCH}" refs/aeris/base 'fork main' 2>"${fetch_err}")"; then
  summary_msg="Bounded fetch of fork ${BASE_BRANCH} failed; synchronization stopped before any mutation."
  raw="$(cat "${fetch_err}")"
  raw="${raw:-no diagnostic output}"
  rm -f -- "${fetch_err}"
  report_sync_alert fetch "${BASE_BRANCH}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi
[[ "${base_sha}" =~ ^[0-9a-f]{40}$ ]] || {
  echo 'error: fork main fetch did not return an exact SHA' >&2
  rm -f -- "${fetch_err}"
  exit 1
}
if ! upstream_sha="$(fetch_exact "${UPSTREAM_URL}" main refs/aeris/upstream 'upstream main' 2>"${fetch_err}")"; then
  summary_msg="Bounded fetch of upstream ${UPSTREAM_REPOSITORY}:main failed; synchronization stopped before any mutation."
  raw="$(cat "${fetch_err}")"
  raw="${raw:-no diagnostic output}"
  rm -f -- "${fetch_err}"
  report_sync_alert fetch "${UPSTREAM_REPOSITORY}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi
rm -f -- "${fetch_err}"
[[ "${upstream_sha}" =~ ^[0-9a-f]{40}$ ]] || {
  echo 'error: upstream main fetch did not return an exact SHA' >&2
  exit 1
}
alert_key="${upstream_sha:0:12}"
output base_sha "${base_sha}"
output upstream_sha "${upstream_sha}"

# The bounded bootstrap wipe deleted every ref, leaving HEAD dangling. Re-point
# the trusted checkout at the base that was just fetched and validated.
bounded_git switch --detach "${base_sha}" 2>/dev/null || bounded_git checkout --detach "${base_sha}"
[[ "$(bounded_git rev-parse HEAD)" == "${base_sha}" ]] || {
  raw="HEAD=$(bounded_git rev-parse HEAD 2>&1) expected=${base_sha}"
  summary_msg='Trusted checkout HEAD no longer equals the fetched base SHA.'
  report_sync_alert invalid-state "${base_sha:0:12}" "${summary_msg}" "${raw}"
  output state error
  exit 1
}

# Progress is the git DAG: merge-base is the synchronization state.
if bounded_git merge-base --is-ancestor "${upstream_sha}" "${base_sha}"; then
  echo "upstream ${upstream_sha} is already an ancestor of ${BASE_BRANCH}; nothing to do"
  output state up-to-date
  summary "### Upstream sync (minimal)
- State: \`up-to-date\`
- Upstream: \`${upstream_sha}\`"
  exit 0
fi

# Inventory every PR for the fixed head/base pair, newest first. `--method GET`
# is explicit: gh api otherwise defaults to POST when -f fields are present,
# which turned this read into a pull-request creation attempt (#180 first run).
if ! pulls_json="$(bounded_gh api --method GET "repos/${GITHUB_REPOSITORY}/pulls" \
    -f state=all -f "base=${BASE_BRANCH}" -f "head=${REPO_OWNER}:${SYNC_BRANCH}" \
    -f sort=updated -f direction=desc -f per_page=100 \
    --jq '[.[] | {number, state, merged_at, body, head: .head.sha, url: .html_url}]' 2>&1)" ||
   ! jq -e 'type == "array"' <<<"${pulls_json}" >/dev/null 2>&1; then
  summary_msg="Unable to inventory synchronization pull requests for ${SYNC_BRANCH} → ${BASE_BRANCH}."
  raw="${pulls_json}"
  raw="${raw:-no diagnostic output}"
  report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi

open_pr_number=''
open_pr_url=''
open_pr_head=''
open_pr_marker=''
open_pr_count=0
newest_merged_marker=''
while IFS= read -r pr_line; do
  pr_body_text="$(jq -r '.body // ""' <<<"${pr_line}")"
  [[ "${pr_body_text}" == *"${MANAGED_MARKER}"* ]] || {
    if [[ "$(jq -r '.state' <<<"${pr_line}")" == open ]]; then
      summary_msg="A non-automation PR #$(jq -r '.number' <<<"${pr_line}") occupies the reserved synchronization branch ${SYNC_BRANCH}."
      raw="pr=${pr_line}"
      report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
      output state error
      exit 1
    fi
    continue
  }
  if [[ "$(jq -r '.state' <<<"${pr_line}")" == open ]]; then
    open_pr_count=$((open_pr_count + 1))
    open_pr_number="$(jq -r '.number' <<<"${pr_line}")"
    open_pr_url="$(jq -r '.url' <<<"${pr_line}")"
    open_pr_head="$(jq -r '.head' <<<"${pr_line}")"
    open_pr_marker="$(marker_of <<<"${pr_body_text}")"
  elif [[ -z "${newest_merged_marker}" && "$(jq -r '.merged_at' <<<"${pr_line}")" != null ]]; then
    newest_merged_marker="$(marker_of <<<"${pr_body_text}")"
  fi
done < <(jq -c '.[]' <<<"${pulls_json}")

if ((open_pr_count > 1)); then
  summary_msg="Multiple open managed synchronization PRs (${open_pr_count}) exist for ${SYNC_BRANCH}; refusing to guess which one is authoritative."
  raw="${pulls_json}"
  report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi

# Discipline guard: if the newest merged managed PR already claims this
# upstream SHA but the SHA is still not an ancestor of main, the PR was
# squashed or rebased and ancestor connectivity is broken. Stop loudly.
if [[ "${newest_merged_marker}" == "${upstream_sha}" ]]; then
  summary_msg="The merged synchronization PR claims upstream ${upstream_sha}, but that commit is not an ancestor of ${BASE_BRANCH}. The PR was merged with squash or rebase instead of a merge commit. Recover by merging upstream/main into main with a true merge commit."
  raw="newest merged PR marker=${newest_merged_marker}
merge-base --is-ancestor ${upstream_sha} ${base_sha}: exit=1"
  report_sync_alert merge-discipline "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi

# Fetch the current remote sync branch tip when it exists. The branch is
# deleted automatically when its PR merges (delete_branch_on_merge), so its
# absence is the normal post-merge state and simply means "rebuild".
fetch_err="$(mktemp)"
if ! remote_sync_tip="$(fetch_exact "${ORIGIN_URL}" "${SYNC_BRANCH}" refs/aeris/sync-branch 'sync branch' true 2>"${fetch_err}")"; then
  summary_msg="Bounded fetch of the ${SYNC_BRANCH} branch tip failed."
  raw="$(cat "${fetch_err}")"
  raw="${raw:-no diagnostic output}"
  rm -f -- "${fetch_err}"
  report_sync_alert fetch "${SYNC_BRANCH}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi
rm -f -- "${fetch_err}"

if [[ -n "${open_pr_number}" ]]; then
  # An open PR's head branch must exist and match the remote tip exactly.
  if [[ -z "${remote_sync_tip}" || "${remote_sync_tip}" != "${open_pr_head}" ]]; then
    summary_msg="Open sync PR #${open_pr_number} head ${open_pr_head} does not match the remote ${SYNC_BRANCH} tip '${remote_sync_tip:-absent}'."
    raw="pr_head=${open_pr_head}
remote_tip=${remote_sync_tip:-absent}"
    report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  # Reuse the PR only when its head is exactly the clean merge of the CURRENT
  # base and upstream. strict_required_status_checks_policy demands the branch
  # stay current with main, so a stale base or an advanced upstream both force
  # a rebuild of the same fixed branch.
  if [[ "${open_pr_marker}" == "${upstream_sha}" ]] &&
     [[ "$(bounded_git cat-file -p "${open_pr_head}" | awk '/^parent / { print $2 }' | paste -sd' ' -)" == "${base_sha} ${upstream_sha}" ]]; then
    head_sha="${open_pr_head}"
    pr_number="${open_pr_number}"
    pr_url="${open_pr_url}"
    echo "reusing PR #${pr_number} at ${head_sha} (parents match current base and upstream)"
  fi
else
  # No open PR. A leftover branch is safe to overwrite only when its content
  # is already merged into main (e.g. branch deletion failed after a merge);
  # any other tip is an unknown state and stays fail-closed.
  if [[ -n "${remote_sync_tip}" ]] &&
     ! bounded_git merge-base --is-ancestor "${remote_sync_tip}" "${base_sha}"; then
    summary_msg="${SYNC_BRANCH} exists at ${remote_sync_tip} with no open managed PR and unmerged content (a maintainer likely closed the previous sync PR). Resolve the leftover branch manually; synchronization paused fail-closed."
    raw="remote_tip=${remote_sync_tip}
merge-base --is-ancestor ${remote_sync_tip} ${base_sha}: exit=1"
    report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
fi

if [[ -z "${head_sha}" ]]; then
  # Build the three-way merge locally. A conflict stops the loop before any
  # publication and opens an idempotent alert with the conflicted path list.
  bounded_git config user.name 'github-actions[bot]'
  bounded_git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
  bounded_git switch -c "${SYNC_BRANCH}" "${base_sha}"
  set +e
  merge_output="$(bounded_git merge --no-ff \
    -m "Merge ${UPSTREAM_REPOSITORY}@${upstream_sha} into ${BASE_BRANCH}" \
    "${upstream_sha}" 2>&1)"
  merge_rc=$?
  set -e
  if ((merge_rc != 0)); then
    conflict_files="$(bounded_git diff --name-only --diff-filter=U 2>/dev/null | head -n 200 || true)"
    bounded_git merge --abort >/dev/null 2>&1 || true
    summary_msg="Upstream ${upstream_sha} conflicts with ${BASE_BRANCH} ${base_sha}. Synchronization stopped; the branch, PR, and history were left unchanged. A maintainer must resolve this round manually (merge commit only, never squash).

Conflicted paths:
${conflict_files:-unavailable}"
    raw="git merge exit=${merge_rc}
${merge_output}"
    report_sync_alert conflict "${alert_key}" "${summary_msg}" "${raw}"
    output state conflict
    summary "### Upstream sync (minimal)
- State: \`conflict\`
- Upstream: \`${upstream_sha}\`"
    exit 1
  fi
  head_sha="$(bounded_git rev-parse HEAD)"

  # Publish (or republish) the fixed branch. force-with-lease pins the exact
  # remote tip we validated, so a concurrent writer fails the push instead of
  # being overwritten.
  push_extra=()
  if [[ -n "${remote_sync_tip}" ]]; then
    push_extra+=("--force-with-lease=refs/heads/${SYNC_BRANCH}:${remote_sync_tip}")
  fi
  if [[ "${remote_sync_tip}" != "${head_sha}" ]]; then
    set +e
    push_output="$(bounded_git_push push "${push_extra[@]}" "${ORIGIN_URL}" \
      "${head_sha}:refs/heads/${SYNC_BRANCH}" 2>&1)"
    push_rc=$?
    set -e
    if ((push_rc != 0)); then
      summary_msg="Unable to publish ${SYNC_BRANCH} at ${head_sha}."
      raw="bounded push exit=${push_rc}
${push_output}"
      report_sync_alert publication "${alert_key}" "${summary_msg}" "${raw}"
      output state error
      exit 1
    fi
  fi
  confirmed_err="$(mktemp)"
  if ! confirmed_tip="$(fetch_exact "${ORIGIN_URL}" "${SYNC_BRANCH}" refs/aeris/sync-branch-confirm 'sync branch' true 2>"${confirmed_err}")"; then
    summary_msg="Unable to re-read ${SYNC_BRANCH} after publishing ${head_sha}."
    raw="$(cat "${confirmed_err}")"
    raw="${raw:-no diagnostic output}"
    rm -f -- "${confirmed_err}"
    report_sync_alert publication "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  rm -f -- "${confirmed_err}"
  if [[ "${confirmed_tip}" != "${head_sha}" ]]; then
    summary_msg="${SYNC_BRANCH} moved to '${confirmed_tip:-absent}' immediately after publication of ${head_sha}."
    raw="expected=${head_sha}
observed=${confirmed_tip:-absent}"
    report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi

  if [[ -n "${open_pr_number}" ]]; then
    set +e
    edit_output="$(bounded_gh pr edit "${open_pr_number}" --repo "${GITHUB_REPOSITORY}" \
      --title "chore: sync upstream ${alert_key}" \
      --body "$(pr_body)" 2>&1)"
    edit_rc=$?
    set -e
    if ((edit_rc != 0)); then
      summary_msg="Unable to update sync PR #${open_pr_number} to ${head_sha}."
      raw="gh pr edit exit=${edit_rc}
${edit_output}"
      report_sync_alert publication "${alert_key}" "${summary_msg}" "${raw}"
      output state error
      exit 1
    fi
    pr_number="${open_pr_number}"
    pr_url="${open_pr_url}"
  else
    set +e
    create_output="$(bounded_gh pr create --repo "${GITHUB_REPOSITORY}" \
      --base "${BASE_BRANCH}" --head "${SYNC_BRANCH}" \
      --title "chore: sync upstream ${alert_key}" \
      --body "$(pr_body)" 2>&1)"
    create_rc=$?
    set -e
    if ((create_rc != 0)); then
      # A concurrent run may have created the PR; adopt it when it is the
      # single managed open PR for this head/base pair. `--method GET` is
      # explicit: gh api otherwise defaults to POST when -f fields are present.
      if ! adopt_json="$(bounded_gh api --method GET "repos/${GITHUB_REPOSITORY}/pulls" \
        -f state=open -f "base=${BASE_BRANCH}" -f "head=${REPO_OWNER}:${SYNC_BRANCH}" \
        -f per_page=100 --jq '[.[] | {number, body, url}]' 2>&1)"; then
        summary_msg="Unable to re-inventory pull requests after PR creation failed for ${head_sha}."
        raw="gh pr create exit=${create_rc}
${create_output}
adopt inventory: ${adopt_json}"
        report_sync_alert publication "${alert_key}" "${summary_msg}" "${raw}"
        output state error
        exit 1
      fi
      adopt_count="$(jq '[.[] | select((.body // "") | contains("'"${MANAGED_MARKER}"'"))] | length' <<<"${adopt_json}")"
      if [[ "${adopt_count}" == 1 ]]; then
        pr_number="$(jq -r '[.[] | select((.body // "") | contains("'"${MANAGED_MARKER}"'"))][0].number' <<<"${adopt_json}")"
        pr_url="$(jq -r '[.[] | select((.body // "") | contains("'"${MANAGED_MARKER}"'"))][0].url' <<<"${adopt_json}")"
      else
        summary_msg="Unable to create the synchronization PR for ${head_sha}."
        raw="gh pr create exit=${create_rc}
${create_output}"
        report_sync_alert publication "${alert_key}" "${summary_msg}" "${raw}"
        output state error
        exit 1
      fi
    else
      pr_url="$(tail -n1 <<<"${create_output}")"
      pr_number="${pr_url##*/}"
    fi
  fi

  # Prove the published PR identity before any merge machinery touches it.
  if ! pr_view="$(bounded_gh pr view "${pr_number}" --repo "${GITHUB_REPOSITORY}" \
    --json state,isDraft,headRefOid,headRefName,headRepository,baseRefName 2>&1)"; then
    summary_msg="Unable to re-read sync PR #${pr_number} after publication of ${head_sha}."
    raw="${pr_view}"
    raw="${raw:-no diagnostic output}"
    report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  if ! jq -e --arg head_sha "${head_sha}" --arg head_branch "${SYNC_BRANCH}" \
      --arg repository "${GITHUB_REPOSITORY}" --arg base_branch "${BASE_BRANCH}" '
      type == "object" and .state == "OPEN" and .isDraft == false and
      .headRefOid == $head_sha and .headRefName == $head_branch and
      .headRepository.nameWithOwner == $repository and .baseRefName == $base_branch
    ' <<<"${pr_view}" >/dev/null; then
    summary_msg="Published sync PR #${pr_number} identity drifted from ${SYNC_BRANCH}@${head_sha}."
    raw="${pr_view}"
    report_sync_alert invalid-state "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  echo "sync PR #${pr_number} tracks ${SYNC_BRANCH}@${head_sha}: ${pr_url}"
fi

# .github/** is fork-owned governance surface: workflows, the sync policy, and
# the automation runtime. A merge that moves those paths is reviewable by a
# human only — publish the PR but never dispatch checks (dispatch runs the
# merged workflow files) and never arm auto-merge. This gate applies to fresh
# and reused heads alike, so a drifted PR can never slip into auto-merge on a
# later run.
drifted="$(bounded_git diff --name-only "${base_sha}" "${head_sha}" -- .github/ | head -n 200 || true)"
if [[ -n "${drifted}" ]]; then
  drift_tree="$(bounded_git rev-parse "${upstream_sha}:.github" 2>/dev/null || printf 'absent')"
  summary_msg="Upstream ${upstream_sha} moves fork-owned .github/** paths in the merge into ${BASE_BRANCH}. PR #${pr_number} was published WITHOUT auto-merge and no checks were dispatched (a dispatched run would execute the merged workflow files). A maintainer must review and merge it manually — merge commit only, never squash.

Drifted paths:
${drifted}"
  raw="git diff --name-only ${base_sha} ${head_sha} -- .github/
${drifted}"
  report_sync_alert workflow-drift "${drift_tree:0:12}" "${summary_msg}" "${raw}"
  output state workflow_drift
  output pr_url "${pr_url}"
  summary "### Upstream sync (minimal)
- State: \`workflow_drift\`
- Upstream: \`${upstream_sha}\`
- Pull request (manual review, no auto-merge): ${pr_url}"
  exit 1
fi

ensure_check_dispatch rust-ci.yml "Rust CI / check"
ensure_check_dispatch frontend-ci.yml "Frontend CI / check"
# The gate evaluates the sync PR against the trusted policy at the current
# main tip; both were validated earlier in this run and are pinned as inputs.
ensure_check_dispatch automation-policy.yml "Automation Policy / gate" \
  -f "ref=${SYNC_BRANCH}" -f "pull_number=${pr_number}" -f "policy_sha=${base_sha}"

# Arm GitHub native auto-merge with the merge method. This is the only merge
# machinery in the loop; it fires only after every required check passes.
if ! pr_auto_merge="$(bounded_gh pr view "${pr_number}" --repo "${GITHUB_REPOSITORY}" \
    --json autoMergeRequest --jq '.autoMergeRequest == null' 2>&1)"; then
  summary_msg="Unable to read the auto-merge state of sync PR #${pr_number}."
  raw="${pr_auto_merge}"
  raw="${raw:-no diagnostic output}"
  report_sync_alert automerge "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi
if [[ "${pr_auto_merge}" == true ]]; then
  set +e
  automerge_output="$(bounded_gh pr merge --auto --merge "${pr_number}" \
    --repo "${GITHUB_REPOSITORY}" 2>&1)"
  automerge_rc=$?
  set -e
  if ((automerge_rc != 0)); then
    summary_msg="Unable to arm GitHub native auto-merge (merge method) on PR #${pr_number}. Common cause: repository Settings → General → Allow auto-merge is disabled."
    raw="pr merge exit=${automerge_rc}
${automerge_output}"
    report_sync_alert automerge "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
fi
echo "auto-merge (merge method) armed on PR #${pr_number}"

# Bounded wait for the auto-merge to land. A timeout leaves the armed PR in
# place; the next run reuses it and resumes waiting.
merge_commit_oid=''
gate_gap_polls=0
merged=false
for ((attempt = 1; attempt <= WAIT_ATTEMPTS; attempt += 1)); do
  # A transient API failure must not fail the run; keep waiting instead.
  if ! pr_state="$(bounded_gh pr view "${pr_number}" --repo "${GITHUB_REPOSITORY}" \
      --json state,mergeCommit,headRefOid \
      --jq '{state, merge_oid: (.mergeCommit.oid // ""), head: .headRefOid}' 2>/dev/null)"; then
    ((attempt == WAIT_ATTEMPTS)) || sleep "${WAIT_SECONDS}"
    continue
  fi
  case "$(jq -r '.state' <<<"${pr_state}")" in
    MERGED)
      [[ "$(jq -r '.head' <<<"${pr_state}")" == "${head_sha}" ]] || {
        summary_msg="Sync PR #${pr_number} merged but its head drifted from ${head_sha}."
        raw="${pr_state}"
        report_sync_alert merge-discipline "${alert_key}" "${summary_msg}" "${raw}"
        output state error
        exit 1
      }
      merge_commit_oid="$(jq -r '.merge_oid' <<<"${pr_state}")"
      merged=true
      break
      ;;
    CLOSED)
      summary_msg="Sync PR #${pr_number} was closed without merging; synchronization paused fail-closed. Reopen it or delete the ${SYNC_BRANCH} branch to let the next run rebuild it."
      raw="${pr_state}"
      report_sync_alert pr-closed "${alert_key}" "${summary_msg}" "${raw}"
      output state error
      exit 1
      ;;
  esac

  failed_context=''
  rust_state=''
  frontend_state=''
  gate_state=''
  for context in "${REQUIRED_CONTEXTS[@]}"; do
    state="$(context_state "${head_sha}" "${context}")"
    case "${context}" in
      "Rust CI / check") rust_state="${state}" ;;
      "Frontend CI / check") frontend_state="${state}" ;;
      "Automation Policy / gate") gate_state="${state}" ;;
    esac
    if [[ "${state}" == failed && -z "${failed_context}" ]]; then
      failed_context="${context}"
    fi
  done
  if [[ -n "${failed_context}" ]]; then
    summary_msg="Required check '${failed_context}' failed on sync PR #${pr_number} head ${head_sha}. Auto-merge stays blocked; a maintainer must investigate."
    raw="rust=${rust_state} frontend=${frontend_state} gate=${gate_state}
failed=${failed_context}"
    report_sync_alert check-failure "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  # The gate is dispatched onto the sync branch above, so its check normally
  # appears within one poll. Once both CI checks are green and the gate
  # context is still absent on several consecutive polls, the dispatched run
  # never published its check (a stuck queue or a skipped run): stop instead
  # of waiting forever.
  if [[ "${rust_state}" == success && "${frontend_state}" == success &&
        "${gate_state}" == absent ]]; then
    gate_gap_polls=$((gate_gap_polls + 1))
  else
    gate_gap_polls=0
  fi
  if ((gate_gap_polls >= 3)); then
    summary_msg="PR #${pr_number} is green on both CI required checks, but 'Automation Policy / gate' never appeared on head ${head_sha} even though automation-policy.yml was dispatched onto ${SYNC_BRANCH}. The dispatched gate run is stuck or was skipped: inspect the Automation Policy runs for ${SYNC_BRANCH} (or merge this PR manually with a merge commit)."
    raw="rust=${rust_state} frontend=${frontend_state} gate=${gate_state} (3 consecutive polls)"
    report_sync_alert missing-required-check "${alert_key}" "${summary_msg}" "${raw}"
    output state error
    exit 1
  fi
  ((attempt == WAIT_ATTEMPTS)) || sleep "${WAIT_SECONDS}"
done

if [[ "${merged}" != true ]]; then
  echo "auto-merge still pending after $((WAIT_ATTEMPTS * WAIT_SECONDS))s; the armed PR is reused and re-awaited by the next run"
  output state armed
  output pr_url "${pr_url}"
  summary "### Upstream sync (minimal)
- State: \`armed\` (auto-merge pending)
- Upstream: \`${upstream_sha}\`
- Pull request: ${pr_url}"
  exit 0
fi

# Post-merge discipline verification (#175 decision): the merge must have
# landed as a true merge commit and upstream/main must now be fully contained
# in main (behind == 0).
[[ "${merge_commit_oid}" =~ ^[0-9a-f]{40}$ ]] || {
  summary_msg="Merged sync PR #${pr_number} did not report a merge commit SHA."
  raw="merge_commit=${merge_commit_oid}"
  report_sync_alert merge-discipline "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
}
fetch_err="$(mktemp)"
if ! merged_base="$(fetch_exact "${ORIGIN_URL}" "${BASE_BRANCH}" refs/aeris/base-merged 'merged fork main' 2>"${fetch_err}")"; then
  summary_msg="Unable to re-fetch ${BASE_BRANCH} after the merge of PR #${pr_number}; post-merge verification could not run."
  raw="$(cat "${fetch_err}")"
  raw="${raw:-no diagnostic output}"
  rm -f -- "${fetch_err}"
  report_sync_alert merge-discipline "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi
rm -f -- "${fetch_err}"
[[ "${merged_base}" =~ ^[0-9a-f]{40}$ ]] || {
  summary_msg="Unable to re-fetch ${BASE_BRANCH} after the merge of PR #${pr_number}."
  raw="merged_base=${merged_base}"
  report_sync_alert merge-discipline "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
}
behind="$(bounded_git rev-list --count "${merged_base}..${upstream_sha}")"
merge_parents="$(bounded_git cat-file -p "${merge_commit_oid}^{commit}" 2>/dev/null | grep -c '^parent ' || true)"
if [[ "${behind}" != 0 || "${merge_parents}" != 2 ]]; then
  summary_msg="Sync PR #${pr_number} merged but ancestor connectivity is broken: git rev-list --count ${BASE_BRANCH}..upstream = ${behind} (must be 0) and merge commit ${merge_commit_oid} has ${merge_parents} parents (must be 2). The PR was merged with squash or rebase. Recover by merging upstream/main into main with a true merge commit."
  raw="behind=${behind}
merge_commit=${merge_commit_oid}
parents=${merge_parents}
base=${merged_base}
upstream=${upstream_sha}"
  report_sync_alert merge-discipline "${alert_key}" "${summary_msg}" "${raw}"
  output state error
  exit 1
fi

echo "merged: PR #${pr_number} as merge commit ${merge_commit_oid}; behind==0 verified"
output state merged
output pr_url "${pr_url}"
summary "### Upstream sync (minimal)
- State: \`merged\`
- Upstream: \`${upstream_sha}\`
- Merge commit: \`${merge_commit_oid}\` (two parents, behind==0 verified)
- Pull request: ${pr_url}"
