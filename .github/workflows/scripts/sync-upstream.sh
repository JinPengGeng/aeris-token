#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Every authenticated GitHub operation rechecks the time-bounded authorization.
source "${SCRIPT_DIR}/github-autonomy.sh"
BOUNDED_FETCH_HELPER="${BOUNDED_FETCH_HELPER:-${SCRIPT_DIR}/bounded-git-fetch.sh}"
source "${BOUNDED_FETCH_HELPER}"

bounded_tree_git() {
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git "$@"
}

bounded_tree_blob_to_file() {
  local revision="$1" path="$2" destination="$3"
  bounded_tree_git show "${revision}:${path}" >"${destination}"
}

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
: "${AERIS_ISSUES_GH_TOKEN:?AERIS_ISSUES_GH_TOKEN is required}"
: "${AERIS_WRITER_APP_SLUG:?AERIS_WRITER_APP_SLUG is required}"
[[ "${AERIS_WRITER_APP_SLUG}" =~ ^[a-z0-9][a-z0-9-]{0,99}$ ]] || {
  echo 'AERIS_WRITER_APP_SLUG must be a lowercase GitHub App slug.' >&2
  exit 78
}

BASE_BRANCH="${BASE_BRANCH:-main}"
SYNC_BRANCH="${SYNC_BRANCH:-automation/sync-upstream}"
RESUME="${RESUME:-false}"
STATE_FILE="${STATE_FILE:-.github/upstream-sync-state.json}"
SYNC_POLICY_FILE="${SYNC_POLICY_FILE:-.github/upstream-sync-policy.yml}"
SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREPARE_HELPER="${PREPARE_HELPER:-${SCRIPT_ROOT}/prepare-checkpoint-sync.sh}"
AUTOMERGE_HELPER="${AUTOMERGE_HELPER:-${SCRIPT_ROOT}/manage-sync-automerge.sh}"

MANAGED_MARKER='<!-- upstream-sync-managed -->'
AUTO_CLOSED_MARKER='<!-- upstream-sync-auto-closed -->'
WRITER_APP_BOT_LOGIN="${AERIS_WRITER_APP_SLUG}[bot]"
LEGACY_BOT_LOGIN='github-actions[bot]'
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
checkpoint_sha=''
prepared_checkpoint_sha=''
fetched_base_sha=''
fetched_upstream_sha=''
sync_app_bot_id=''
sync_app_bot_type=''
published_pr_number=''
published_pr_url=''
published_pr_body=''
GITHUB_API_PAGE_BYTES=2097152
GITHUB_API_MAX_PAGES=10
GITHUB_API_TOTAL_BYTES=$((GITHUB_API_PAGE_BYTES * GITHUB_API_MAX_PAGES))
autonomous_eligible='false'
policy_verdict=''

output() {
  printf '%s=%s\n' "$1" "$2" >>"${GITHUB_OUTPUT}"
}

aeris_bounded_gh() {
  aeris_require_active_autonomy_window || return
  # gh is a Go binary: the deadline runner keeps the hard timeout and file
  # bounds without the virtual-memory ceiling its runtime cannot start under.
  aeris_bounded_run_deadline "${GITHUB_API_PAGE_BYTES}" gh "$@"
}

# Writer App credentials are restricted to branch, PR, and pending-tip comment
# publication. Issue inventory and ordinary issue comments use the workflow
# token through this isolated channel.
aeris_issues_gh() {
  aeris_require_active_autonomy_window || return
  GH_TOKEN="${AERIS_ISSUES_GH_TOKEN}" command gh "$@"
}

aeris_bounded_issues_gh() {
  aeris_require_active_autonomy_window || return
  GH_TOKEN="${AERIS_ISSUES_GH_TOKEN}" aeris_bounded_run_deadline "${GITHUB_API_PAGE_BYTES}" gh "$@"
}

# Read an authoritative JSON array through explicit, resource-bounded pages.
# The channel argument selects the credential scope for the endpoint read.
aeris_read_bounded_api_array_pages() {
  local channel="$1" endpoint="$2" destination="$3" label="$4"
  shift 4
  local page_dir page_file count size total_bytes=0 page status terminal_proven=false
  local -a query_args=("$@") page_files=()
  page_dir="$(mktemp -d)" || return 1
  for ((page = 1; page <= GITHUB_API_MAX_PAGES + 1; page += 1)); do
    page_file="${page_dir}/page-${page}.json"
    if ! "${channel}" api --method GET "${endpoint}" \
      "${query_args[@]}" -f per_page=100 -f "page=${page}" >"${page_file}"; then
      rm -f -- "${page_dir}"/page-*.json
      rmdir -- "${page_dir}" 2>/dev/null || true
      echo "Unable to read authoritative ${label} page ${page}." >&2
      return 1
    fi
    size="$(wc -c <"${page_file}")"
    count="$(aeris_bounded_run 1024 jq -er \
      'if type == "array" and length <= 100 then length else error("invalid bounded page") end' \
      "${page_file}")" || {
      rm -f -- "${page_dir}"/page-*.json
      rmdir -- "${page_dir}" 2>/dev/null || true
      echo "Authoritative ${label} page ${page} is not a bounded JSON array." >&2
      return 1
    }
    if [[ ! "${size}" =~ ^[0-9]+$ || ${size} -le 0 || ${size} -gt ${GITHUB_API_PAGE_BYTES} ||
          ! "${count}" =~ ^[0-9]+$ ]]; then
      rm -f -- "${page_dir}"/page-*.json
      rmdir -- "${page_dir}" 2>/dev/null || true
      echo "Authoritative ${label} page ${page} exceeds its resource bound." >&2
      return 1
    fi
    if ((page > GITHUB_API_MAX_PAGES)); then
      if ((count != 0)); then
        rm -f -- "${page_dir}"/page-*.json
        rmdir -- "${page_dir}" 2>/dev/null || true
        echo "Authoritative ${label} exceeds ${GITHUB_API_MAX_PAGES} pages." >&2
        return 1
      fi
      rm -f -- "${page_file}"
      break
    fi
    total_bytes=$((total_bytes + size))
    if ((total_bytes > GITHUB_API_TOTAL_BYTES)); then
      rm -f -- "${page_dir}"/page-*.json
      rmdir -- "${page_dir}" 2>/dev/null || true
      echo "Authoritative ${label} exceeds its aggregate resource bound." >&2
      return 1
    fi
    page_files+=("${page_file}")
    if ((count < 100)); then
      terminal_proven=true
      break
    fi
  done
  if [[ "${terminal_proven}" != true && ${page} -le ${GITHUB_API_MAX_PAGES} ]]; then
    rm -f -- "${page_dir}"/page-*.json
    rmdir -- "${page_dir}" 2>/dev/null || true
    echo "Authoritative ${label} pagination did not prove a terminal page." >&2
    return 1
  fi
  if ((${#page_files[@]} == 0)); then
    printf '[]\n' >"${destination}"
    status=0
  else
    set +e
    aeris_bounded_run "${GITHUB_API_TOTAL_BYTES}" jq -cs 'add' \
      "${page_files[@]}" >"${destination}"
    status=$?
    set -e
  fi
  rm -f -- "${page_dir}"/page-*.json
  rmdir -- "${page_dir}" 2>/dev/null || true
  return "${status}"
}

aeris_writer_git_push() {
  local temp_root askpass_dir askpass status=0
  : "${AERIS_WRITER_TOKEN:?AERIS_WRITER_TOKEN is required for Writer Git publication}"
  temp_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  askpass_dir="$(mktemp -d "${temp_root%/}/aeris-writer-askpass.XXXXXX")"
  askpass="${askpass_dir}/askpass.sh"
  if ! (
    umask 077
    cat >"${askpass}" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${AERIS_WRITER_TOKEN:?}" ;;
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
    aeris_git_network -c credential.helper= -c http.https://github.com/.extraheader= "$@" || status=$?
  rm -f -- "${askpass}" || status=1
  rmdir -- "${askpass_dir}" || status=1
  return "${status}"
}

list_sync_prs() {
  local response status
  response="$(mktemp)" || return 1
  aeris_read_bounded_api_array_pages aeris_bounded_gh \
    "repos/${GITHUB_REPOSITORY}/pulls" "${response}" 'synchronization pull requests' \
    -f state=all -f "base=${BASE_BRANCH}" -f "head=${repo_owner}:${SYNC_BRANCH}" \
    -f sort=updated -f direction=desc || {
    rm -f -- "${response}"
    return 1
  }
  set +e
  aeris_bounded_run "${GITHUB_API_TOTAL_BYTES}" jq -c '[.[] | {
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
    }]' "${response}"
  status=$?
  set -e
  rm -f -- "${response}"
  return "${status}"
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

issue_bot_comments() {
  local response status
  response="$(mktemp)" || return 1
  aeris_read_bounded_api_array_pages aeris_bounded_issues_gh \
    "repos/${GITHUB_REPOSITORY}/issues/$1/comments" "${response}" \
    'ordinary issue comments' || {
    rm -f -- "${response}"
    return 1
  }
  set +e
  aeris_bounded_run "${GITHUB_API_TOTAL_BYTES}" jq -r \
    --arg sync "${WRITER_APP_BOT_LOGIN}" --arg legacy "${LEGACY_BOT_LOGIN}" \
    '.[] | select(.user.login == $sync or .user.login == $legacy) | .body' "${response}"
  status=$?
  set -e
  rm -f -- "${response}"
  return "${status}"
}

pr_bot_comments() {
  local response status
  response="$(mktemp)" || return 1
  aeris_read_bounded_api_array_pages aeris_bounded_gh \
    "repos/${GITHUB_REPOSITORY}/issues/$1/comments" "${response}" \
    'synchronization pull request comments' || {
    rm -f -- "${response}"
    return 1
  }
  set +e
  aeris_bounded_run "${GITHUB_API_TOTAL_BYTES}" jq -r \
    --arg sync "${WRITER_APP_BOT_LOGIN}" --arg legacy "${LEGACY_BOT_LOGIN}" \
    '.[] | select(.user.login == $sync or .user.login == $legacy) | .body' "${response}"
  status=$?
  set -e
  rm -f -- "${response}"
  return "${status}"
}

is_sync_automation_login() {
  [[ "$1" == "${WRITER_APP_BOT_LOGIN}" || "$1" == "${LEGACY_BOT_LOGIN}" || "$1" == app/github-actions ]]
}

issue_comment_once() {
  local number="$1" key="$2" message="$3" marker comments
  marker="<!-- upstream-sync-${key} -->"
  comments="$(issue_bot_comments "${number}")"
  if [[ "${comments}" != *"${marker}"* ]]; then
    aeris_issues_gh api --method POST \
      "repos/${GITHUB_REPOSITORY}/issues/${number}/comments" \
      -f body="${marker}
${message}" >/dev/null
  fi
}

pr_comment_once() {
  local number="$1" key="$2" message="$3" marker comments
  marker="<!-- upstream-sync-${key} -->"
  comments="$(pr_bot_comments "${number}")"
  if [[ "${comments}" != *"${marker}"* ]]; then
    aeris_bounded_gh api --method POST \
      "repos/${GITHUB_REPOSITORY}/issues/${number}/comments" \
      -f body="${marker}
${message}" >/dev/null
  fi
}

# Authenticate the planned SHA before push so an interrupted publication can
# recover without treating commit author fields as an ownership boundary.
set_pending_tip() {
  local number="$1" sha="$2" comment_id body comments_file
  body="<!-- upstream-sync-pending-tip:${sha} -->
Prepared automation branch tip ${sha}."
  comments_file="$(mktemp)" || return 1
  aeris_read_bounded_api_array_pages aeris_bounded_gh \
    "repos/${GITHUB_REPOSITORY}/issues/${number}/comments" "${comments_file}" \
    'synchronization pull request comments' || {
    rm -f -- "${comments_file}"
    return 1
  }
  comment_id="$(aeris_bounded_run 1024 jq -r \
    --arg sync "${WRITER_APP_BOT_LOGIN}" --arg legacy "${LEGACY_BOT_LOGIN}" \
    '[.[] | select((.user.login == $sync or .user.login == $legacy) and
      ((.body // "") | startswith("<!-- upstream-sync-pending-tip:"))) | .id] | last // empty' \
    "${comments_file}")" || {
    rm -f -- "${comments_file}"
    return 1
  }
  rm -f -- "${comments_file}"
  if [[ -n "${comment_id}" ]]; then
    # GitHub authorizes a PR comment mutation with pull_requests:write even
    # when the REST route is under /issues. Use the bounded Writer App token;
    # the workflow token intentionally has only pull-requests:read.
    aeris_bounded_gh api \
      --method PATCH \
      "repos/${GITHUB_REPOSITORY}/issues/comments/${comment_id}" \
      -f body="${body}" >/dev/null
  else
    # Keep this on the REST issue-comments endpoint so the mutation is
    # idempotent and does not rely on the GraphQL CLI fallback.
    aeris_bounded_gh api --method POST \
      "repos/${GITHUB_REPOSITORY}/issues/${number}/comments" \
      -f body="${body}" >/dev/null
  fi
}

latest_close_actor() {
  local response status
  response="$(mktemp)" || return 1
  aeris_read_bounded_api_array_pages aeris_bounded_issues_gh \
    "repos/${GITHUB_REPOSITORY}/issues/$1/events" "${response}" \
    'synchronization pull request events' || {
    rm -f -- "${response}"
    return 1
  }
  set +e
  aeris_bounded_run 1024 jq -r \
    '[.[] | select(.event == "closed") | .actor.login] | last // empty' "${response}"
  status=$?
  set -e
  rm -f -- "${response}"
  return "${status}"
}

pr_was_auto_closed() {
  local number="$1"
  is_sync_automation_login "$(latest_close_actor "${number}" || true)" || return 1
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
  is_sync_automation_login "${author}" || return 1
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
  local subject body author committer source automation base_trailer actual_parent parent_count
  subject="$(bounded_tree_git show -s --format=%s "${sha}")"
  body="$(bounded_tree_git show -s --format=%B "${sha}")"
  author="$(bounded_tree_git show -s --format=%ae "${sha}")"
  committer="$(bounded_tree_git show -s --format=%ce "${sha}")"
  source="$(sed -n 's/^Sync-Upstream-Source: //p' <<<"${body}" | tail -n1)"
  automation="$(sed -n 's/^Sync-Upstream-Automation: //p' <<<"${body}" | tail -n1)"
  base_trailer="$(sed -n 's/^Sync-Upstream-Base: //p' <<<"${body}" | tail -n1)"
  # Historical synchronization commits have exactly one parent; current ones
  # additionally link the advertised upstream tip as their second parent.
  parent_count="$(bounded_tree_git rev-list --parents -n1 "${sha}" | wc -w)"
  [[ "${parent_count}" -eq 2 || "${parent_count}" -eq 3 ]] || return 1
  actual_parent="$(bounded_tree_git rev-parse "${sha}^")"
  [[ "${author}" == "${BOT_EMAIL}" && "${committer}" == "${BOT_EMAIL}" ]] || return 1
  [[ "${automation}" == true && "${source}" == "${parent}@"* ]] || return 1
  [[ "${source##*@}" =~ ^[0-9a-f]{40}$ ]] || return 1
  [[ "${subject}" == "chore: sync ${source}" ]] || return 1
  [[ "${base_trailer}" == "${actual_parent}" ]] || return 1
  if [[ "${parent_count}" -eq 3 ]]; then
    [[ "$(bounded_tree_git rev-parse "${sha}^2")" == "${source##*@}" ]] || return 1
  fi
  bounded_tree_git merge-base --is-ancestor "${actual_parent}" "${current_base}"
}

is_legacy_tip() {
  local sha="$1" current_base="$2" pr_json="$3"
  local source source_sha subject author committer actual_parent
  [[ -n "${pr_json}" && "$(jq -r '.headRefOid' <<<"${pr_json}")" == "${sha}" ]] || return 1
  source="$(source_from_pr "${pr_json}")"
  [[ "${source}" == "${parent}@"* && "${source##*@}" =~ ^[0-9a-f]{40}$ ]] || return 1
  source_sha="${source##*@}"
  subject="$(bounded_tree_git show -s --format=%s "${sha}")"
  author="$(bounded_tree_git show -s --format=%ae "${sha}")"
  committer="$(bounded_tree_git show -s --format=%ce "${sha}")"
  [[ "$(bounded_tree_git rev-list --parents -n1 "${sha}" | wc -w)" -eq 2 ]] || return 1
  actual_parent="$(bounded_tree_git rev-parse "${sha}^")"
  [[ "${author}" == "${BOT_EMAIL}" && "${committer}" == "${BOT_EMAIL}" ]] || return 1
  [[ "${subject}" == "chore: sync ${parent}@${source_sha}" ||
     "${subject}" == "chore: sync ${parent}@${source_sha:0:12}" ]] || return 1
  bounded_tree_git merge-base --is-ancestor "${actual_parent}" "${current_base}"
}

fetch_remote_tip() {
  aeris_bounded_read_remote_ref origin "refs/heads/${SYNC_BRANCH}" \
    'synchronization branch' true
  remote_sha="${AERIS_BOUNDED_REMOTE_SHA}"
  if [[ -n "${remote_sha}" ]]; then
    aeris_bounded_fetch_ref origin "refs/heads/${SYNC_BRANCH}" "${remote_sha}" \
      "refs/remotes/origin/${SYNC_BRANCH}" 'synchronization branch'
  fi
}

fetch_source_refs() {
  aeris_bounded_read_remote_ref origin "refs/heads/${BASE_BRANCH}" 'protected base branch'
  fetched_base_sha="${AERIS_BOUNDED_REMOTE_SHA}"
  aeris_bounded_fetch_ref origin "refs/heads/${BASE_BRANCH}" "${fetched_base_sha}" \
    "refs/remotes/origin/${BASE_BRANCH}" 'protected base branch'

  aeris_bounded_read_remote_ref upstream "refs/heads/${upstream_branch}" 'upstream branch'
  fetched_upstream_sha="${AERIS_BOUNDED_REMOTE_SHA}"
  aeris_bounded_fetch_ref upstream "refs/heads/${upstream_branch}" "${fetched_upstream_sha}" \
    "refs/remotes/upstream/${upstream_branch}" 'upstream branch'
}

aeris_assert_publication_refs_exact() {
  local expected_base="$1" expected_upstream="$2" expected_head="$3"
  local current_base current_upstream current_head
  if [[ "${AERIS_SYNC_TEST_MODE:-false}" == true &&
        "${AERIS_SYNC_TEST_FIXTURE:-false}" == true &&
        -n "${AERIS_SYNC_BEFORE_FINAL_REF_FENCE_HOOK:-}" ]]; then
    "${AERIS_SYNC_BEFORE_FINAL_REF_FENCE_HOOK}" || return 1
  fi
  aeris_bounded_read_remote_ref origin "refs/heads/${BASE_BRANCH}" 'protected base branch' || return
  current_base="${AERIS_BOUNDED_REMOTE_SHA}"
  aeris_bounded_read_remote_ref upstream "refs/heads/${upstream_branch}" 'upstream branch' || return
  current_upstream="${AERIS_BOUNDED_REMOTE_SHA}"
  aeris_bounded_read_remote_ref origin "refs/heads/${SYNC_BRANCH}" \
    'synchronization branch' true || return
  current_head="${AERIS_BOUNDED_REMOTE_SHA}"
  [[ "${current_base}" == "${expected_base}" &&
     "${current_upstream}" == "${expected_upstream}" &&
     "${current_head}" == "${expected_head}" ]]
}

aeris_validate_authoritative_published_pr() {
  local pr_file pr_size status
  [[ "${published_pr_number}" =~ ^[1-9][0-9]*$ && -n "${published_pr_body}" ]] || return 1
  pr_file="$(mktemp)" || return 1
  aeris_require_active_autonomy_window || {
    rm -f -- "${pr_file}"
    return 1
  }
  if ! aeris_bounded_run_deadline 2097152 gh api \
    "repos/${GITHUB_REPOSITORY}/pulls/${published_pr_number}" >"${pr_file}"; then
    rm -f -- "${pr_file}"
    return 1
  fi
  pr_size="$(wc -c <"${pr_file}")"
  if [[ ! "${pr_size}" =~ ^[0-9]+$ || ${pr_size} -le 0 || ${pr_size} -gt 2097152 ]]; then
    rm -f -- "${pr_file}"
    return 1
  fi
  set +e
  AERIS_EXPECTED_PR_BODY="${published_pr_body}" node - "${pr_file}" \
    "${published_pr_number}" "${GITHUB_REPOSITORY}" "${BASE_BRANCH}" "${base_sha}" \
    "${SYNC_BRANCH}" "${published_sha}" "${WRITER_APP_BOT_LOGIN}" \
    "${sync_app_bot_id}" "${sync_app_bot_type}" <<'NODE'
const fs = require('node:fs');
const [path, number, repository, baseRef, baseSha, headRef, headSha,
  authorLogin, authorId, authorType] = process.argv.slice(2);
const pr = JSON.parse(fs.readFileSync(path, 'utf8'));
const body = pr.body;
const valid = pr && !Array.isArray(pr) && pr.number === Number(number) &&
  pr.state === 'open' && pr.draft === false &&
  pr.user?.login === authorLogin && pr.user?.id === Number(authorId) &&
  pr.user?.type === authorType &&
  pr.base?.repo?.full_name === repository && pr.base?.ref === baseRef &&
  pr.base?.sha === baseSha && pr.head?.repo?.full_name === repository &&
  pr.head?.ref === headRef && pr.head?.sha === headSha &&
  typeof body === 'string' && body === process.env.AERIS_EXPECTED_PR_BODY &&
  body.includes('<!-- upstream-sync-managed -->') &&
  body.includes(`<!-- upstream-sync-owned-tip:${headSha} -->`);
process.exit(valid ? 0 : 1);
NODE
  status=$?
  set -e
  rm -f -- "${pr_file}"
  return "${status}"
}

aeris_post_publish_fence() {
  aeris_assert_publication_refs_exact \
    "${base_sha}" "${upstream_sha}" "${published_sha}" || return
  if [[ "${AERIS_SYNC_TEST_MODE:-false}" == true &&
        "${AERIS_SYNC_TEST_FIXTURE:-false}" == true &&
        -n "${AERIS_SYNC_AFTER_PUBLISH_REF_FENCE_HOOK:-}" ]]; then
    "${AERIS_SYNC_AFTER_PUBLISH_REF_FENCE_HOOK}" || return 1
  fi
  aeris_validate_authoritative_published_pr || return
  if [[ "${AERIS_SYNC_TEST_MODE:-false}" == true &&
        "${AERIS_SYNC_TEST_FIXTURE:-false}" == true &&
        -n "${AERIS_SYNC_BEFORE_SUCCESS_REF_FENCE_HOOK:-}" ]]; then
    "${AERIS_SYNC_BEFORE_SUCCESS_REF_FENCE_HOOK}" || return 1
  fi
  aeris_assert_publication_refs_exact \
    "${base_sha}" "${upstream_sha}" "${published_sha}"
}

read_checkpoint_from_base() {
  local base="$1" state_file status
  state_file="$(mktemp)" || return 1
  bounded_tree_blob_to_file "${base}" "${STATE_FILE}" "${state_file}" 2>/dev/null || {
    rm -f -- "${state_file}"
    return 1
  }
  set +e
  node - "${state_file}" "${parent}" "${upstream_branch}" <<'NODE'
const fs = require('node:fs');
const [path, repository, branch] = process.argv.slice(2);
let state;
try {
  state = JSON.parse(fs.readFileSync(path, 'utf8'));
} catch {
  process.exit(1);
}
if (state === null || Array.isArray(state) || typeof state !== 'object' ||
    state.schema_version !== 1 || state.policy_version !== 1 ||
    state.repository !== repository || state.branch !== branch ||
    typeof state.last_integrated_sha !== 'string' ||
    !/^[0-9a-f]{40}$/.test(state.last_integrated_sha)) process.exit(1);
process.stdout.write(state.last_integrated_sha);
NODE
  status=$?
  set -e
  rm -f -- "${state_file}"
  return "${status}"
}

aeris_read_bounded_api_json() {
  local endpoint="$1" destination="$2" label="$3" size
  aeris_require_active_autonomy_window || return 1
  if ! aeris_bounded_run_deadline 2097152 gh api "${endpoint}" >"${destination}"; then
    echo "Unable to read authoritative ${label}." >&2
    return 1
  fi
  size="$(wc -c <"${destination}")"
  if [[ ! "${size}" =~ ^[0-9]+$ || ${size} -le 0 || ${size} -gt 2097152 ]]; then
    echo "Authoritative ${label} response is empty or exceeds its bound." >&2
    return 1
  fi
}

read_sync_identity() {
  local repository_file upstream_file bot_file resolved_parent status
  repository_file="$(mktemp)" || return 1
  upstream_file="$(mktemp)" || {
    rm -f -- "${repository_file}"
    return 1
  }
  bot_file="$(mktemp)" || {
    rm -f -- "${repository_file}" "${upstream_file}"
    return 1
  }
  aeris_read_bounded_api_json "repos/${GITHUB_REPOSITORY}" "${repository_file}" \
    'fork repository identity' || {
    rm -f -- "${repository_file}" "${upstream_file}" "${bot_file}"
    return 1
  }
  resolved_parent="$(node - "${repository_file}" <<'NODE'
const fs = require('node:fs');
let repository;
try {
  repository = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
} catch {
  process.exit(1);
}
const parent = repository?.parent;
if (repository === null || Array.isArray(repository) || typeof repository !== 'object' ||
    parent === null || Array.isArray(parent) || typeof parent !== 'object' ||
    typeof parent.full_name !== 'string' ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/.test(parent.full_name)) {
  process.exit(1);
}
process.stdout.write(parent.full_name);
NODE
)" || {
    rm -f -- "${repository_file}" "${upstream_file}" "${bot_file}"
    return 1
  }
  aeris_read_bounded_api_json "repos/${resolved_parent}" "${upstream_file}" \
    'upstream repository identity' || {
    rm -f -- "${repository_file}" "${upstream_file}" "${bot_file}"
    return 1
  }
  aeris_read_bounded_api_json "users/${AERIS_WRITER_APP_SLUG}%5Bbot%5D" "${bot_file}" \
    'Writer App bot identity' || {
    rm -f -- "${repository_file}" "${upstream_file}" "${bot_file}"
    return 1
  }
  set +e
  node - "${repository_file}" "${upstream_file}" "${bot_file}" \
    "${resolved_parent}" "${WRITER_APP_BOT_LOGIN}" <<'NODE'
const fs = require('node:fs');
const [repositoryPath, upstreamPath, botPath, expectedParent, expectedLogin] = process.argv.slice(2);
let repository;
let upstream;
let bot;
try {
  repository = JSON.parse(fs.readFileSync(repositoryPath, 'utf8'));
  upstream = JSON.parse(fs.readFileSync(upstreamPath, 'utf8'));
  bot = JSON.parse(fs.readFileSync(botPath, 'utf8'));
} catch {
  process.exit(1);
}
const parent = repository?.parent;
if (repository === null || Array.isArray(repository) || typeof repository !== 'object' ||
    parent === null || Array.isArray(parent) || typeof parent !== 'object' ||
    parent.full_name !== expectedParent ||
    upstream === null || Array.isArray(upstream) || typeof upstream !== 'object' ||
    upstream.full_name !== expectedParent ||
    typeof upstream.default_branch !== 'string' || upstream.default_branch.length === 0 ||
    upstream.default_branch.includes('\n') || upstream.default_branch.includes('\r') ||
    bot === null || Array.isArray(bot) || typeof bot !== 'object' ||
    bot.login !== expectedLogin || !Number.isSafeInteger(bot.id) || bot.id < 1 || bot.type !== 'Bot') {
  process.exit(1);
}
process.stdout.write(`${expectedParent}\n${upstream.default_branch}\n${bot.id}\n${bot.type}\n`);
NODE
  status=$?
  set -e
  rm -f -- "${repository_file}" "${upstream_file}" "${bot_file}"
  return "${status}"
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
    if aeris_bounded_gh pr reopen --repo "${GITHUB_REPOSITORY}" "${number}" >/dev/null 2>&1; then
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

disarm_tracked_pr() {
  [[ -n "${tracked_pr}" ]] || return 0
  bash "${AUTOMERGE_HELPER}" \
    disarm \
    "${GITHUB_REPOSITORY}" \
    "$(jq -r '.url' <<<"${tracked_pr}")"
}

report_workflow_drift() {
  local current_tree changed title existing
  [[ -n "${checkpoint_sha}" && "${checkpoint_sha}" != "${upstream_sha}" ]] || return 0
  current_tree="$(bounded_tree_git rev-parse "${upstream_sha}:.github/workflows" 2>/dev/null || printf absent)"
  changed="$(bounded_tree_git diff --name-only "${checkpoint_sha}" "${upstream_sha}" -- .github/workflows || true)"
  [[ -n "${changed}" ]] || return 0

  title="[sync-upstream] Review upstream workflow tree ${current_tree:0:12}"
  existing="$(aeris_bounded_issues_gh issue list \
    --repo "${GITHUB_REPOSITORY}" \
    --state all \
    --limit 100 \
    --search "\"${title}\" in:title" \
    --json title \
    --jq ".[] | select(.title == \"${title}\") | .title" | head -n1)"
  if [[ -z "${existing}" ]]; then
    aeris_bounded_issues_gh issue create \
      --repo "${GITHUB_REPOSITORY}" \
      --title "${title}" \
      --body "<!-- upstream-sync-workflow-tree:${current_tree} -->
Upstream changed fork-owned workflow files. Review these paths manually:

${changed}" >/dev/null
  fi
  echo "::warning title=Upstream workflow drift::${changed//$'\n'/, }"
}

report_sync_alert() {
  local kind="$1" key="$2" message="$3" title existing number marker
  title="[sync-upstream] ${kind}: ${key}"
  marker="<!-- upstream-sync-alert:${kind}:${key} -->"
  existing="$(aeris_bounded_issues_gh issue list \
    --repo "${GITHUB_REPOSITORY}" \
    --state open \
    --limit 100 \
    --search "\"${title}\" in:title" \
    --json number,title,body \
    --jq ".[] | select(.title == \"${title}\" and ((.body // \"\") | contains(\"${marker}\"))) | .number" | head -n1)"
  if [[ -z "${existing}" ]]; then
    aeris_bounded_issues_gh issue create \
      --repo "${GITHUB_REPOSITORY}" \
      --title "${title}" \
      --body "${marker}
${message}" >/dev/null
    return 0
  fi
  number="${existing}"
  issue_comment_once "${number}" "alert:${kind}:${key}" "${message}"
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
    aeris_bounded_gh pr close --repo "${GITHUB_REPOSITORY}" "${number}" >/dev/null
  fi
  output state noop
  output has_changes false
  exit 0
}

wait_for_pr_head() {
  local pr="$1" expected_sha="$2" attempt view_data
  for attempt in 1 2 3 4 5 6; do
    if view_data="$(aeris_bounded_gh pr view --repo "${GITHUB_REPOSITORY}" "${pr}" --json state,headRefOid)" &&
       [[ "$(jq -r '.state' <<<"${view_data}")" == OPEN &&
          "$(jq -r '.headRefOid' <<<"${view_data}")" == "${expected_sha}" ]]; then
      printf '%s\n' "${view_data}"
      return 0
    fi
    ((attempt == 6)) || sleep 2
  done
  echo "PR ${pr} did not expose synchronization tip ${expected_sha} in time." >&2
  return 1
}

publish_pr() {
  local body merge_behavior number pr_url create_error create_output view_data trusted_view
  require_gate
  fetch_remote_tip
  [[ "${remote_sha}" == "${published_sha}" ]] || {
    echo 'Synchronization branch moved before PR publication.' >&2
    return 1
  }

  if [[ "${autonomous_eligible}:${policy_verdict}" == true:eligible ||
        "${autonomous_eligible}:${policy_verdict}" == true:conflict_ai_review ]]; then
    merge_behavior='This pull request is merged once after protected branch checks pass; no persistent auto-merge is configured.'
  else
    merge_behavior='This pull request requires maintainer review and is not directly merged by synchronization automation.'
  fi
  body="${MANAGED_MARKER}
<!-- upstream-sync-owned-tip:${published_sha} -->
<!-- upstream-sync-source:${parent}@${upstream_sha} -->
Automated synchronization from ${parent}:${upstream_branch} at ${upstream_sha}.
Checkpoint advanced from ${checkpoint_sha} to ${upstream_sha}.

${merge_behavior}
Configured fork-owned paths are preserved; upstream workflow changes are reviewed separately."

  if [[ -n "${tracked_pr}" ]]; then
    number="$(jq -r '.number' <<<"${tracked_pr}")"
    pr_url="$(jq -r '.url' <<<"${tracked_pr}")"
    view_data="$(wait_for_pr_head "${number}" "${published_sha}")" || return 1
  else
    create_error="$(mktemp)"
    if create_output="$(aeris_bounded_gh pr create \
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
    view_data="$(wait_for_pr_head "${pr_url}" "${published_sha}")" || return 1
  fi

  aeris_bounded_gh pr edit \
    --repo "${GITHUB_REPOSITORY}" \
    "${pr_url}" \
    --title 'chore: sync upstream' \
    --body "${body}"
  refresh_prs
  [[ "$(jq 'length' <<<"${open_prs}")" -eq 1 &&
     "$(jq -r '.headRefOid' <<<"${open_pr}")" == "${published_sha}" ]] || return 1
  published_pr_number="$(jq -r '.number' <<<"${open_pr}")"
  published_pr_url="${pr_url}"
  published_pr_body="${body}"
  trusted_view="$(aeris_bounded_gh pr view "${pr_url}" --repo "${GITHUB_REPOSITORY}" \
    --json state,isDraft,headRefOid,headRefName,headRepository,baseRefName,baseRefOid,autoMergeRequest)"
  jq -e --arg head_sha "${published_sha}" --arg head_branch "${SYNC_BRANCH}" \
    --arg repository "${GITHUB_REPOSITORY}" --arg base_branch "${BASE_BRANCH}" '
    type == "object" and .state == "OPEN" and .isDraft == false and
    .headRefOid == $head_sha and .headRefName == $head_branch and
    .headRepository.nameWithOwner == $repository and .baseRefName == $base_branch and
    (.baseRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and
    .autoMergeRequest == null
  ' <<<"${trusted_view}" >/dev/null || return 1
  output pr_url "${pr_url}"
  output pr_number "${published_pr_number}"
  output expected_base_sha "$(jq -r '.baseRefOid' <<<"${trusted_view}")"
}

mapfile -t sync_identity < <(read_sync_identity)
[[ ${#sync_identity[@]} -eq 4 ]] || {
  echo 'Unable to resolve the authoritative Writer App bot identity.' >&2
  exit 1
}
parent="${sync_identity[0]}"
upstream_branch="${sync_identity[1]}"
sync_app_bot_id="${sync_identity[2]}"
sync_app_bot_type="${sync_identity[3]}"
output parent "${parent}"
output upstream_branch "${upstream_branch}"

bounded_tree_git remote remove upstream >/dev/null 2>&1 || true
bounded_tree_git remote add upstream "https://github.com/${parent}.git"
bounded_tree_git config user.name 'github-actions[bot]'
bounded_tree_git config user.email "${BOT_EMAIL}"
AERIS_BOUNDED_FETCH_PREFLIGHT=aeris_require_active_autonomy_window
AERIS_BOUNDED_FETCH_CREDENTIALLESS=true
aeris_bounded_fetch_init "${SYNC_POLICY_FILE}"

if pause_or_resume; then
  :
else
  rc=$?
  ((rc == 20)) && exit 0
  exit "${rc}"
fi
disarm_tracked_pr

for attempt in 1 2 3; do
  fetch_source_refs
  base_sha="${fetched_base_sha}"
  upstream_sha="${fetched_upstream_sha}"
  [[ "$(bounded_tree_git rev-parse HEAD)" == "${base_sha}" ]] || {
    echo 'Trusted checkout HEAD no longer equals the fetched base SHA.' >&2
    output state error
    output has_changes false
    exit 1
  }
  output upstream_sha "${upstream_sha}"

  refresh_prs
  fetch_remote_tip
  expected_remote_sha="${remote_sha}"
  assert_remote_owned "${base_sha}"

  checkpoint_sha="$(read_checkpoint_from_base "${base_sha}")" || {
    message="Protected checkpoint state is invalid at base ${base_sha}. Synchronization stopped before processing the upstream tree."
    report_sync_alert invalid-state "${base_sha:0:12}" "${message}"
    output state error
    output has_changes false
    exit 1
  }
  if ! bounded_tree_git merge-base --is-ancestor "${checkpoint_sha}" "${upstream_sha}"; then
    message="Checkpoint ${checkpoint_sha} is not an ancestor of upstream ${upstream_sha}. Synchronization stopped without changing the branch, PR, or checkpoint."
    report_sync_alert history-rewrite "${upstream_sha:0:12}" "${message}"
    output state history_rewrite
    output has_changes false
    exit 1
  fi
  if ! aeris_enforce_change_bounds "${checkpoint_sha}" "${upstream_sha}" \
    'upstream source delta'; then
    message="Upstream ${upstream_sha} exceeds the protected Git resource bounds. Synchronization stopped before processing its tree."
    report_sync_alert resource-limit "${upstream_sha:0:12}" "${message}"
    output state resource_limit
    output has_changes false
    exit 1
  fi

  set +e
  prepare_output="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    env "NODE_OPTIONS=--jitless --max-old-space-size=192" "${PREPARE_HELPER}" \
    "${base_sha}" \
    "${upstream_sha}" \
    "${parent}" \
    "${upstream_branch}" \
    "${STATE_FILE}" \
    "${SYNC_POLICY_FILE}")"
  prepare_status=$?
  set -e
  prepared_checkpoint_sha="$(sed -n 's/^checkpoint=//p' <<<"${prepare_output}" | tail -n1)"
  prepare_state="$(sed -n 's/^state=//p' <<<"${prepare_output}" | tail -n1)"

  if ((prepare_status != 0)); then
    alert_key="${upstream_sha:0:12}"
    case "${prepare_status}:${prepare_state}" in
      1:conflict)
        conflict_bundle_sha="$(sed -n 's/^conflict_bundle_sha=//p' <<<"${prepare_output}" | tail -n1)"
        conflict_generation_sha="$(sed -n 's/^conflict_generation_sha=//p' <<<"${prepare_output}" | tail -n1)"
        message="Upstream ${upstream_sha} conflicts with base ${base_sha} from checkpoint ${checkpoint_sha:-unknown}. The existing PR and branch were preserved."
        if [[ -n "${tracked_pr}" ]]; then
          pr_comment_once \
            "$(jq -r '.number' <<<"${tracked_pr}")" \
            "conflict:${upstream_sha}" \
            "${message}"
        fi
        report_sync_alert conflict "${alert_key}" "${message}"
        if [[ -n "${AERIS_CONFLICT_CANDIDATE_PATH:-}" ]]; then
          output state conflict_resolution_rejected
        elif [[ "${conflict_bundle_sha}" =~ ^[0-9a-f]{64}$ &&
                "${conflict_generation_sha}" =~ ^[0-9a-f]{64}$ &&
                -n "${AERIS_CONFLICT_BUNDLE_PATH:-}" ]]; then
          output state conflict_pending
          output conflict_pending true
          output conflict_bundle_sha "${conflict_bundle_sha}"
          output conflict_generation_sha "${conflict_generation_sha}"
          output checkpoint_sha "${checkpoint_sha}"
          output expected_base_sha "${base_sha}"
          output policy_verdict eligible
          output autonomous_eligible false
          output has_changes false
          exit 0
        else
          output state conflict
        fi
        ;;
      2:history_rewrite)
        message="Checkpoint ${checkpoint_sha:-unknown} is not an ancestor of upstream ${upstream_sha}. Synchronization stopped without changing the branch, PR, or checkpoint."
        report_sync_alert history-rewrite "${alert_key}" "${message}"
        output state history_rewrite
        ;;
      *)
        message="Checkpoint state or policy validation failed for base ${base_sha} and upstream ${upstream_sha}. Synchronization stopped without publication."
        report_sync_alert invalid-state "${alert_key}" "${message}"
        output state error
        ;;
    esac
    output has_changes false
    exit 1
  fi

  [[ -n "${prepared_checkpoint_sha}" && "${prepared_checkpoint_sha}" == "${checkpoint_sha}" ]] || {
    echo 'Checkpoint helper did not preserve the prevalidated checkpoint.' >&2
    exit 1
  }
  filtered_paths="$(sed -n 's/^filtered_paths=//p' <<<"${prepare_output}" | tail -n1)"
  autonomous_eligible="$(sed -n 's/^autonomous_eligible=//p' <<<"${prepare_output}" | tail -n1)"
  policy_verdict="$(sed -n 's/^policy_verdict=//p' <<<"${prepare_output}" | tail -n1)"
  [[ "${autonomous_eligible}:${policy_verdict}" == true:eligible ||
     "${autonomous_eligible}:${policy_verdict}" == true:conflict_ai_review ||
     "${autonomous_eligible}:${policy_verdict}" == false:manual_review ||
     "${autonomous_eligible}:${policy_verdict}" == false:noop ]] || {
    echo 'Checkpoint helper did not return a trusted synchronization verdict.' >&2
    exit 1
  }
  output checkpoint_sha "${checkpoint_sha}"
  output filtered_paths "${filtered_paths:-0}"
  output autonomous_eligible "${autonomous_eligible}"
  output policy_verdict "${policy_verdict}"
  conflict_bundle_sha="$(sed -n 's/^conflict_bundle_sha=//p' <<<"${prepare_output}" | tail -n1)"
  conflict_candidate_sha="$(sed -n 's/^conflict_candidate_sha=//p' <<<"${prepare_output}" | tail -n1)"
  conflict_generation_sha="$(sed -n 's/^conflict_generation_sha=//p' <<<"${prepare_output}" | tail -n1)"
  conflict_resolution_sha="$(sed -n 's/^conflict_resolution_sha=//p' <<<"${prepare_output}" | tail -n1)"
  conflict_resolved_tree="$(sed -n 's/^conflict_resolved_tree=//p' <<<"${prepare_output}" | tail -n1)"
  conflict_resolver_model_sha="$(sed -n 's/^conflict_resolver_model_sha=//p' <<<"${prepare_output}" | tail -n1)"
  if [[ "${policy_verdict}" == conflict_ai_review ]]; then
    for value in "${conflict_bundle_sha}" "${conflict_candidate_sha}" "${conflict_generation_sha}" \
      "${conflict_resolution_sha}" "${conflict_resolver_model_sha}"; do
      [[ "${value}" =~ ^[0-9a-f]{64}$ ]] || {
        echo 'Checkpoint helper returned an invalid conflict-resolution hash.' >&2
        exit 1
      }
    done
    [[ "${conflict_resolved_tree}" =~ ^[0-9a-f]{40}$ ]] || {
      echo 'Checkpoint helper returned an invalid resolved merge tree.' >&2
      exit 1
    }
    output conflict_bundle_sha "${conflict_bundle_sha}"
    output conflict_candidate_sha "${conflict_candidate_sha}"
    output conflict_generation_sha "${conflict_generation_sha}"
    output conflict_resolution_sha "${conflict_resolution_sha}"
    output conflict_resolved_tree "${conflict_resolved_tree}"
    output conflict_resolver_model_sha "${conflict_resolver_model_sha}"
  fi
  if [[ "${prepare_state}" == noop ]]; then
    close_obsolete_pr
  fi
  [[ "${prepare_state}" == clean ]] || {
    echo "Unexpected checkpoint preparation state: ${prepare_state:-missing}" >&2
    exit 1
  }

  prepared_tree="$(sed -n 's/^tree=//p' <<<"${prepare_output}" | tail -n1)"
  bounded_tree_git rev-parse --verify "${prepared_tree}^{tree}" >/dev/null
  report_workflow_drift

  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git switch --force-create "${SYNC_BRANCH}" "${base_sha}"
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git read-tree --reset -u "${prepared_tree}"

  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git diff --cached --quiet && close_obsolete_pr

  actual_prepared_tree="$(bounded_tree_git write-tree)"
  [[ "${actual_prepared_tree}" == "${prepared_tree}" ]] || {
    echo 'Prepared candidate tree changed before commit.' >&2
    exit 1
  }
  commit_arguments=(
    -m "chore: sync ${parent}@${upstream_sha}"
    -m 'Sync-Upstream-Automation: true'
    -m "Sync-Upstream-Source: ${parent}@${upstream_sha}"
    -m "Sync-Upstream-Checkpoint: ${checkpoint_sha}->${upstream_sha}"
    -m "Sync-Upstream-Base: ${base_sha}"
    -m "Sync-Upstream-Policy-Verdict: ${policy_verdict}"
  )
  if [[ "${policy_verdict}" == conflict_ai_review ]]; then
    commit_arguments+=(
      -m 'Sync-Upstream-Conflict-Profile: aeris-sync-conflict-v2'
      -m "Sync-Upstream-Conflict-Generation: ${conflict_generation_sha}"
      -m "Sync-Upstream-Conflict-Bundle: ${conflict_bundle_sha}"
      -m "Sync-Upstream-Resolution-Candidate: ${conflict_candidate_sha}"
      -m "Sync-Upstream-Resolution-SHA: ${conflict_resolution_sha}"
      -m "Sync-Upstream-Resolved-Merge-Tree: ${conflict_resolved_tree}"
      -m "Sync-Upstream-Prepared-Tree: ${prepared_tree}"
      -m "Sync-Upstream-Resolver-Model-SHA: ${conflict_resolver_model_sha}"
    )
  fi
  # Link the exact upstream tip as a second parent so synchronization history
  # stays connected to upstream ancestry. commit-tree leaves HEAD, the index,
  # and the working tree untouched, so re-point the branch explicitly.
  local_sha="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git commit-tree "${prepared_tree}" \
      -p "${base_sha}" \
      -p "${upstream_sha}" \
      "${commit_arguments[@]}")"
  [[ "${local_sha}" =~ ^[0-9a-f]{40}$ ]] || {
    echo 'Synchronization commit creation did not return an exact commit SHA.' >&2
    exit 1
  }
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git reset --hard "${local_sha}"

  fetch_source_refs
  if [[ "${base_sha}" != "${fetched_base_sha}" ||
        "${upstream_sha}" != "${fetched_upstream_sha}" ]]; then
    continue
  fi

  require_gate
  fetch_remote_tip
  [[ "${remote_sha}" == "${expected_remote_sha}" ]] || continue
  assert_remote_owned "${base_sha}"
  if ! aeris_assert_publication_refs_exact \
    "${base_sha}" "${upstream_sha}" "${expected_remote_sha}"; then
    continue
  fi

  if [[ -n "${remote_sha}" ]] && aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git diff --quiet "${remote_sha}" "${local_sha}"; then
    published_sha="${remote_sha}"
  else
    reference_pr="${tracked_pr:-${latest_pr}}"
    if [[ -n "${reference_pr}" ]]; then
      set_pending_tip "$(jq -r '.number' <<<"${reference_pr}")" "${local_sha}"
    fi
    if [[ -n "${remote_sha}" ]]; then
      aeris_writer_git_push push \
        --force-with-lease="refs/heads/${SYNC_BRANCH}:${remote_sha}" \
        "https://github.com/${GITHUB_REPOSITORY}.git" "${local_sha}:refs/heads/${SYNC_BRANCH}"
    else
      aeris_writer_git_push push \
        --force-with-lease="refs/heads/${SYNC_BRANCH}:" \
        "https://github.com/${GITHUB_REPOSITORY}.git" "${local_sha}:refs/heads/${SYNC_BRANCH}"
    fi
    published_sha="${local_sha}"
  fi

  aeris_assert_publication_refs_exact \
    "${base_sha}" "${upstream_sha}" "${published_sha}" || exit 1
  publish_pr
  aeris_post_publish_fence || exit 1
  output pr_url "${published_pr_url}"
  output state published
  output has_changes true
  output synced_sha "${published_sha}"
  output synced_tree "$(bounded_tree_git rev-parse "${published_sha}^{tree}")"
  output synced_source "${parent}@${upstream_sha}"
  exit 0
done

output state unstable
output has_changes false
echo 'Base, upstream, or synchronization branch moved during all rebuild attempts.' >&2
exit 1
