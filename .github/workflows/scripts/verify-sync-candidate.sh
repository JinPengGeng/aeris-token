#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREPARE_HELPER="${PREPARE_HELPER:-${SCRIPT_DIR}/prepare-checkpoint-sync.sh}"
METADATA_HELPER="${METADATA_HELPER:-${SCRIPT_DIR}/validate-sync-candidate-metadata.cjs}"
BOUNDED_FETCH_HELPER="${BOUNDED_FETCH_HELPER:-${SCRIPT_DIR}/bounded-git-fetch.sh}"
source "${BOUNDED_FETCH_HELPER}"

bounded_tree_git() {
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git "$@"
}
STATE_PATH='.github/upstream-sync-state.json'
POLICY_PATH='.github/upstream-sync-policy.yml'
MAX_PR_BYTES=2097152

to_node_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s\n' "$1"
  fi
}

fail() {
  printf 'error: sync candidate verification failed: %s\n' "$1" >&2
  exit 1
}

usage() {
  printf '%s\n' 'usage: verify-sync-candidate.sh <owner/repo> <pr-number|pr-url> <expected-head-sha>' >&2
  exit 64
}

valid_repository() {
  [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$ ]]
}

parse_pr_number() {
  local value="$1"
  if [[ "${value}" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s\n' "${value}"
    return
  fi
  if [[ "${value}" =~ ^https://github\.com/([^/]+)/([^/]+)/pull/([1-9][0-9]*)/?$ ]] &&
     [[ "${BASH_REMATCH[1]}/${BASH_REMATCH[2]}" == "${REPOSITORY}" ]]; then
    printf '%s\n' "${BASH_REMATCH[3]}"
    return
  fi
  usage
}

[[ $# -eq 3 ]] || usage
REPOSITORY="$1"
valid_repository "${REPOSITORY}" || usage
PR_NUMBER="$(parse_pr_number "$2")"
EXPECTED_HEAD="${3,,}"
[[ "${EXPECTED_HEAD}" =~ ^[0-9a-f]{40}$ ]] || usage
: "${AERIS_SYNC_APP_SLUG:?AERIS_SYNC_APP_SLUG is required}"
[[ "${AERIS_SYNC_APP_SLUG}" =~ ^[a-z0-9][a-z0-9-]{0,99}$ ]] || usage
SYNC_APP_BOT_LOGIN="${AERIS_SYNC_APP_SLUG}[bot]"

tmp_root="${AERIS_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${tmp_root}"
work_dir="$(mktemp -d "${tmp_root%/}/aeris-verify-sync.XXXXXX")"
pr_initial="${work_dir}/pr-initial.json"
pr_final="${work_dir}/pr-final.json"
pr_authoritative="${work_dir}/pr-authoritative.json"
bot_identity="${work_dir}/bot-identity.json"
message_file="${work_dir}/message"
metadata_file="${work_dir}/metadata.json"
policy_file="${work_dir}/policy.yml"
changed_paths="${work_dir}/changed-paths"

cleanup() {
  rm -f -- "${pr_initial}" "${pr_final}" "${pr_authoritative}" "${bot_identity}" "${message_file}" "${metadata_file}" \
    "${policy_file}" "${changed_paths}"
  rmdir -- "${work_dir}" 2>/dev/null || true
}
trap cleanup EXIT

fetch_ref_sha() {
  local remote="$1" ref="$2" expected="$3" label="$4" destination
  destination="refs/aeris/verify/$(printf '%s' "${label}" | bounded_tree_git hash-object --stdin)"
  aeris_bounded_fetch_ref "${remote}" "${ref}" "${expected}" "${destination}" "${label}" ||
    fail "unable to fetch ${label} through the bounded receiver"
}

read_exact_remote_ref() {
  local remote="$1" ref="$2" label="$3"
  aeris_bounded_read_remote_ref "${remote}" "${ref}" "${label}" ||
    fail "unable to read ${label} through the bounded receiver"
  printf '%s\n' "${AERIS_BOUNDED_REMOTE_SHA}"
}

read_public_json() {
  local url="$1" destination="$2" label="$3" status size
  status="$(curl --silent --show-error \
    --proto '=https' --tlsv1.2 \
    --connect-timeout 10 --max-time 30 --max-filesize "${MAX_PR_BYTES}" \
    --header 'Accept: application/vnd.github+json' \
    --header 'X-GitHub-Api-Version: 2022-11-28' \
    --output "${destination}" --write-out '%{http_code}' \
    "${url}")" || fail "unable to read the public ${label} resource"
  [[ "${status}" == 200 ]] || fail "public ${label} resource returned HTTP ${status}"
  size="$(wc -c <"${destination}")"
  [[ "${size}" =~ ^[0-9]+$ && ${size} -gt 0 && ${size} -le ${MAX_PR_BYTES} ]] ||
    fail "${label} response is empty or exceeds the resource bound"
}

read_pr() {
  read_public_json "https://api.github.com/repos/${REPOSITORY}/pulls/${PR_NUMBER}" \
    "$1" 'pull request'
}

read_policy_field() {
  local section="$1" field="$2"
  awk -v section="${section}" -v field="${field}" '
    $0 == section ":" { in_section = 1; next }
    in_section && /^[^[:space:]#]/ { exit }
    in_section && $0 ~ "^[[:space:]][[:space:]]" field ":[[:space:]]*" {
      value = $0
      sub("^[[:space:]][[:space:]]" field ":[[:space:]]*", "", value)
      sub(/[[:space:]]+$/, "", value)
      if (value ~ /^".*"$/ || value ~ /^\047.*\047$/) value = substr(value, 2, length(value) - 2)
      print value
      exit
    }
  ' "${policy_file}"
}

read_pr_coordinates() {
  node - "$(to_node_path "$1")" <<'NODE'
const fs = require('node:fs');
const pr = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
for (const value of [pr.base?.sha, pr.head?.sha, pr.base?.ref, pr.head?.ref]) {
  if (typeof value !== 'string' || value.length === 0 || value.includes('\n')) process.exit(1);
  process.stdout.write(`${value}\n`);
}
NODE
}

validate_authoritative_pr() {
  node - "$(to_node_path "$1")" "$(to_node_path "${pr_initial}")" \
    "${PR_NUMBER}" "${REPOSITORY}" "${base_ref}" "${base_sha}" \
    "${head_ref}" "${head_sha}" "${SYNC_APP_BOT_LOGIN}" \
    "${trusted_author_id}" "${trusted_author_type}" <<'NODE'
const fs = require('node:fs');
const [currentPath, initialPath, number, repository, baseRef, baseSha, headRef, headSha,
  authorLogin, authorId, authorType] = process.argv.slice(2);
const current = JSON.parse(fs.readFileSync(currentPath, 'utf8'));
const initial = JSON.parse(fs.readFileSync(initialPath, 'utf8'));
const body = current.body;
const valid = current && !Array.isArray(current) &&
  current.number === Number(number) && current.state === 'open' && current.draft === false &&
  current.user?.login === authorLogin && current.user?.id === Number(authorId) &&
  current.user?.type === authorType &&
  current.base?.repo?.full_name === repository && current.base?.ref === baseRef &&
  current.base?.sha === baseSha && current.head?.repo?.full_name === repository &&
  current.head?.ref === headRef && current.head?.sha === headSha &&
  typeof body === 'string' && body === initial.body &&
  body.includes('<!-- upstream-sync-managed -->') &&
  body.includes(`<!-- upstream-sync-owned-tip:${headSha} -->`);
if (!valid) process.exit(1);
NODE
}

read_pr "${pr_initial}"
read_public_json "https://api.github.com/users/${AERIS_SYNC_APP_SLUG}%5Bbot%5D" \
  "${bot_identity}" 'Sync App bot identity'

mapfile -t trusted_author < <(node - "$(to_node_path "${bot_identity}")" \
  "${AERIS_SYNC_APP_SLUG}[bot]" <<'NODE'
const fs = require('node:fs');
const [path, expectedLogin] = process.argv.slice(2);
const user = JSON.parse(fs.readFileSync(path, 'utf8'));
if (user === null || Array.isArray(user) || typeof user !== 'object' ||
    user.login !== expectedLogin || !Number.isSafeInteger(user.id) || user.id < 1 || user.type !== 'Bot') {
  process.exit(1);
}
process.stdout.write(`${user.id}\n${user.type}\n`);
NODE
)
[[ ${#trusted_author[@]} -eq 2 ]] || fail 'trusted Sync App bot identity is invalid'
trusted_author_id="${trusted_author[0]}"
trusted_author_type="${trusted_author[1]}"

mapfile -t pr_coordinates < <(read_pr_coordinates "${pr_initial}")
[[ ${#pr_coordinates[@]} -eq 4 ]] || fail 'pull request base or head coordinates are missing'
base_sha="${pr_coordinates[0]}"
head_sha="${pr_coordinates[1]}"
base_ref="${pr_coordinates[2]}"
head_ref="${pr_coordinates[3]}"
[[ "${head_sha}" == "${EXPECTED_HEAD}" ]] || fail 'expected head no longer matches the pull request'

bounded_tree_git show "${base_sha}:${POLICY_PATH}" >"${policy_file}" 2>/dev/null ||
  fail 'sync policy is missing from the protected base tree'
AERIS_BOUNDED_FETCH_CREDENTIALLESS=true
aeris_bounded_fetch_init "${policy_file}" || fail 'bounded Git fetch contract is invalid'

fork_remote="https://github.com/${REPOSITORY}.git"
fetch_ref_sha "${fork_remote}" "refs/heads/${base_ref}" "${base_sha}" 'protected base branch'
fetch_ref_sha "${fork_remote}" "refs/heads/${head_ref}" "${head_sha}" 'managed head branch'

policy_version="$(awk '/^version:[[:space:]]*[0-9]+[[:space:]]*$/ { print $2; exit }' "${policy_file}")"
[[ "${policy_version}" == 1 ]] || fail 'protected sync policy version is unsupported'
policy_base_branch="$(read_policy_field sync base_branch)"
policy_sync_branch="$(read_policy_field sync branch)"
policy_state_path="$(read_policy_field sync state_file)"
[[ "${policy_base_branch}" == "${base_ref}" && "${policy_sync_branch}" == "${head_ref}" ]] ||
  fail 'pull request branches do not match the protected sync policy'
[[ "${policy_state_path}" == "${STATE_PATH}" ]] || fail 'protected policy uses an unsupported state path'

bounded_tree_git show -s --format=%B "${head_sha}" >"${message_file}" || fail 'unable to read candidate message'
commit_author="$(bounded_tree_git show -s --format=%ae "${head_sha}")"
commit_committer="$(bounded_tree_git show -s --format=%ce "${head_sha}")"
parent_line="$(bounded_tree_git rev-list --parents -n1 "${head_sha}")"
read -r -a parents <<<"${parent_line}"
parent_count="$((${#parents[@]} - 1))"
actual_parent="${parents[1]:-0000000000000000000000000000000000000000}"

node "${METADATA_HELPER}" \
  "${pr_initial}" "${message_file}" "${REPOSITORY}" "${base_ref}" "${head_ref}" \
  "${EXPECTED_HEAD}" "${AERIS_SYNC_APP_SLUG}" "${trusted_author_id}" "${trusted_author_type}" \
  "${commit_author}" "${commit_committer}" \
  "${parent_count}" "${actual_parent}" >"${metadata_file}" || fail 'managed metadata validation failed'

mapfile -t candidate_coordinates < <(node - "$(to_node_path "${metadata_file}")" <<'NODE'
const fs = require('node:fs');
const metadata = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
for (const key of ['checkpoint', 'upstreamTip', 'sourceRepository']) {
  const value = metadata[key];
  if (typeof value !== 'string' || value.length === 0 || value.includes('\n')) process.exit(1);
  process.stdout.write(`${value}\n`);
}
NODE
)
[[ ${#candidate_coordinates[@]} -eq 3 ]] || fail 'validated candidate coordinates are missing'
checkpoint="${candidate_coordinates[0]}"
upstream_tip="${candidate_coordinates[1]}"
upstream_repository="${candidate_coordinates[2]}"
policy_repository="$(read_policy_field upstream repository)"
policy_branch="$(read_policy_field upstream branch)"
[[ "${upstream_repository}" == "${policy_repository}" ]] ||
  fail 'source trailer does not match the protected policy repository'
[[ -n "${policy_branch}" ]] || fail 'protected policy upstream branch is missing'

upstream_remote="https://github.com/${policy_repository}.git"
upstream_current="$(read_exact_remote_ref "${upstream_remote}" \
  "refs/heads/${policy_branch}" 'upstream ref')"
fetch_ref_sha "${upstream_remote}" "refs/heads/${policy_branch}" \
  "${upstream_current}" 'upstream branch'
git cat-file -e "${upstream_tip}^{commit}" 2>/dev/null || fail 'advertised U1 is unavailable from upstream'
bounded_tree_git merge-base --is-ancestor "${checkpoint}" "${upstream_tip}" ||
  fail 'U0 is not an ancestor of U1'
[[ "${upstream_tip}" == "${upstream_current}" ]] ||
  fail 'advertised U1 is not the current protected upstream branch tip'
aeris_enforce_change_bounds "${checkpoint}" "${upstream_tip}" 'upstream source delta' ||
  fail 'upstream source delta exceeds the protected resource bounds'

prepare_output="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" bash "${PREPARE_HELPER}" \
  "${base_sha}" "${upstream_tip}" "${policy_repository}" "${policy_branch}" \
  "${STATE_PATH}" "${POLICY_PATH}")" || fail 'deterministic candidate regeneration failed'
[[ "$(sed -n 's/^state=//p' <<<"${prepare_output}" | tail -n1)" == clean ]] ||
  fail 'candidate does not represent an advancing clean integration'
[[ "$(sed -n 's/^checkpoint=//p' <<<"${prepare_output}" | tail -n1)" == "${checkpoint}" ]] ||
  fail 'commit checkpoint does not match protected base state U0'
expected_tree="$(sed -n 's/^tree=//p' <<<"${prepare_output}" | tail -n1)"
actual_tree="$(git rev-parse "${head_sha}^{tree}")"
[[ "${expected_tree}" == "${actual_tree}" ]] || fail 'candidate tree differs from deterministic integration result'

aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" node - \
  "${base_sha}" "${head_sha}" "${STATE_PATH}" "${checkpoint}" "${upstream_tip}" \
  "${policy_repository}" "${policy_branch}" <<'NODE' || fail 'checkpoint state transition is invalid'
const { execFileSync } = require('node:child_process');
const [base, head, path, checkpoint, upstreamTip, repository, branch] = process.argv.slice(2);
const read = (revision) => JSON.parse(execFileSync('git', ['show', `${revision}:${path}`], { encoding: 'utf8' }));
const before = read(base);
const after = read(head);
const common = (state) => state && !Array.isArray(state) && state.schema_version === 1 &&
  state.repository === repository && state.branch === branch && Number.isInteger(state.policy_version);
if (!common(before) || !common(after) || before.policy_version !== after.policy_version ||
    before.last_integrated_sha !== checkpoint || after.last_integrated_sha !== upstreamTip ||
    checkpoint === upstreamTip) process.exit(1);
NODE

bounded_tree_git diff --no-renames --name-only -z "${base_sha}" "${head_sha}" -- >"${changed_paths}" ||
  fail 'unable to enumerate candidate changes'
mapfile -t fork_patterns < <(awk '
  /^fork_owned:[[:space:]]*$/ { in_section = 1; next }
  in_section && /^[^[:space:]#]/ { exit }
  in_section && /^[[:space:]]*-[[:space:]]*/ {
    value = $0; sub(/^[[:space:]]*-[[:space:]]*/, "", value); sub(/[[:space:]]+$/, "", value)
    if (value ~ /^".*"$/ || value ~ /^\047.*\047$/) value = substr(value, 2, length(value) - 2)
    print value
  }
' "${policy_file}")
((${#fork_patterns[@]} > 0)) || fail 'protected policy has no fork-owned paths'
while IFS= read -r -d '' path; do
  [[ "${path}" == "${STATE_PATH}" ]] && continue
  for pattern in "${fork_patterns[@]}"; do
    if [[ "${pattern}" == *'/**' && "${path}" == "${pattern%/**}/"* ]] || [[ "${path}" == "${pattern}" ]]; then
      fail "fork-owned path differs from the protected base: ${path}"
    fi
  done
done <"${changed_paths}"

read_pr "${pr_final}"
if ! node - "$(to_node_path "${pr_initial}")" "$(to_node_path "${pr_final}")" <<'NODE'
const fs = require('node:fs');
const project = (pr) => ({
  number: pr.number, state: pr.state, draft: pr.draft,
  user: { login: pr.user?.login, id: pr.user?.id, type: pr.user?.type }, body: pr.body,
  base: { ref: pr.base?.ref, sha: pr.base?.sha, repo: { full_name: pr.base?.repo?.full_name } },
  head: { ref: pr.head?.ref, sha: pr.head?.sha, repo: { full_name: pr.head?.repo?.full_name } },
});
const [before, after] = process.argv.slice(2).map((path) => project(JSON.parse(fs.readFileSync(path, 'utf8'))));
if (JSON.stringify(before) !== JSON.stringify(after)) process.exit(1);
NODE
then
  fail 'pull request metadata drifted during verification'
fi
if [[ "${AERIS_VERIFY_TEST_MODE:-false}" == true &&
      "${AERIS_VERIFY_TEST_FIXTURE:-false}" == true &&
      -n "${AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK:-}" ]]; then
  "${AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK}" ||
    fail 'verification final-fence test hook failed'
fi
fetch_ref_sha "${fork_remote}" "refs/heads/${base_ref}" "${base_sha}" 'protected base branch'
fetch_ref_sha "${fork_remote}" "refs/heads/${head_ref}" "${head_sha}" 'managed head branch'
upstream_final="$(read_exact_remote_ref "${upstream_remote}" \
  "refs/heads/${policy_branch}" 'upstream ref')"
[[ "${upstream_final}" == "${upstream_current}" ]] ||
  fail 'upstream branch drifted during verification'

if [[ "${AERIS_VERIFY_TEST_MODE:-false}" == true &&
      "${AERIS_VERIFY_TEST_FIXTURE:-false}" == true &&
      -n "${AERIS_VERIFY_AFTER_REF_FENCE_HOOK:-}" ]]; then
  "${AERIS_VERIFY_AFTER_REF_FENCE_HOOK}" ||
    fail 'verification post-ref-fence test hook failed'
fi
read_pr "${pr_authoritative}"
validate_authoritative_pr "${pr_authoritative}" ||
  fail 'authoritative pull request metadata changed after the ref fence'

if [[ "${AERIS_VERIFY_TEST_MODE:-false}" == true &&
      "${AERIS_VERIFY_TEST_FIXTURE:-false}" == true &&
      -n "${AERIS_VERIFY_BEFORE_SUCCESS_REF_FENCE_HOOK:-}" ]]; then
  "${AERIS_VERIFY_BEFORE_SUCCESS_REF_FENCE_HOOK}" ||
    fail 'verification pre-success ref-fence test hook failed'
fi
fetch_ref_sha "${fork_remote}" "refs/heads/${base_ref}" "${base_sha}" 'protected base branch'
fetch_ref_sha "${fork_remote}" "refs/heads/${head_ref}" "${head_sha}" 'managed head branch'
upstream_final="$(read_exact_remote_ref "${upstream_remote}" \
  "refs/heads/${policy_branch}" 'upstream ref')"
[[ "${upstream_final}" == "${upstream_current}" ]] ||
  fail 'upstream branch drifted immediately before verification success'

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  verified_body_sha256="$(node - "${pr_authoritative}" <<'NODE'
const fs = require('node:fs'); const crypto = require('node:crypto');
const body = JSON.parse(fs.readFileSync(process.argv[2], 'utf8')).body;
process.stdout.write(crypto.createHash('sha256').update(body, 'utf8').digest('hex'));
NODE
)"
  printf 'verified=true\n' >>"${GITHUB_OUTPUT}"
  printf 'verified_pr_number=%s\n' "${PR_NUMBER}" >>"${GITHUB_OUTPUT}"
  printf 'verified_base_ref=%s\n' "${base_ref}" >>"${GITHUB_OUTPUT}"
  printf 'verified_base=%s\n' "${base_sha}" >>"${GITHUB_OUTPUT}"
  printf 'verified_head_ref=%s\n' "${head_ref}" >>"${GITHUB_OUTPUT}"
  printf 'verified_head=%s\n' "${head_sha}" >>"${GITHUB_OUTPUT}"
  printf 'verified_upstream=%s\n' "${upstream_tip}" >>"${GITHUB_OUTPUT}"
  printf 'verified_author_login=%s\n' "${SYNC_APP_BOT_LOGIN}" >>"${GITHUB_OUTPUT}"
  printf 'verified_author_id=%s\n' "${trusted_author_id}" >>"${GITHUB_OUTPUT}"
  printf 'verified_author_type=%s\n' "${trusted_author_type}" >>"${GITHUB_OUTPUT}"
  printf 'verified_body_sha256=%s\n' "${verified_body_sha256}" >>"${GITHUB_OUTPUT}"
fi
printf 'verified sync candidate PR #%s: %s -> %s at %s\n' \
  "${PR_NUMBER}" "${checkpoint}" "${upstream_tip}" "${head_sha}"
