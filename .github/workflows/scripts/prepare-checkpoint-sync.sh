#!/usr/bin/env bash
set -euo pipefail

to_node_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s\n' "$1"
  fi
}

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKPOINT_HELPER="${CHECKPOINT_HELPER:-${SCRIPT_ROOT}/checkpoint-merge.sh}"
BOUNDED_FETCH_HELPER="${BOUNDED_FETCH_HELPER:-${SCRIPT_ROOT}/bounded-git-fetch.sh}"
source "${BOUNDED_FETCH_HELPER}"

bounded_tree_git() {
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git "$@"
}

fork_base="${1:?fork base is required}"
upstream_tip="${2:?upstream tip is required}"
expected_repository="${3:?expected upstream repository is required}"
expected_branch="${4:?expected upstream branch is required}"
state_path="${5:-.github/upstream-sync-state.json}"
policy_path="${6:-.github/upstream-sync-policy.yml}"

fail_error() {
  printf '%s\n' "$1" >&2
  printf 'state=error\n'
  exit 3
}

valid_repo_path() {
  [[ -n "$1" && "$1" != /* && ! "$1" =~ (^|/)\.\.(/|$) ]]
}

valid_repo_path "${state_path}" || fail_error "invalid state path: ${state_path}"
valid_repo_path "${policy_path}" || fail_error "invalid policy path: ${policy_path}"

for entry in "fork:${fork_base}" "upstream:${upstream_tip}"; do
  label="${entry%%:*}"
  ref="${entry#*:}"
  bounded_tree_git rev-parse --verify "${ref}^{commit}" >/dev/null 2>&1 ||
    fail_error "invalid ${label} commit: ${ref}"
done
fork_base="$(bounded_tree_git rev-parse "${fork_base}^{commit}")"
upstream_tip="$(bounded_tree_git rev-parse "${upstream_tip}^{commit}")"

tmp_root="${AERIS_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${tmp_root}"
work_dir="$(mktemp -d "${tmp_root%/}/aeris-prepare-sync.XXXXXX")"
state_json="${work_dir}/state.json"
policy_yaml="${work_dir}/policy.yml"
changed_paths="${work_dir}/changed-paths"
filtered_index="${work_dir}/filtered.index"
final_index="${work_dir}/final.index"
updated_state="${work_dir}/updated-state.json"

cleanup() {
  printf 'Removing temporary workspace: %s\n' "${work_dir}" >&2
  rm -f -- \
    "${state_json}" "${policy_yaml}" "${changed_paths}" \
    "${filtered_index}" "${filtered_index}.lock" \
    "${final_index}" "${final_index}.lock" "${updated_state}"
  rmdir -- "${work_dir}" 2>/dev/null || true
}
trap cleanup EXIT

bounded_tree_git show "${fork_base}:${state_path}" >"${state_json}" 2>/dev/null ||
  fail_error "state file is missing from fork base: ${state_path}"
bounded_tree_git show "${fork_base}:${policy_path}" >"${policy_yaml}" 2>/dev/null ||
  fail_error "policy file is missing from fork base: ${policy_path}"

policy_version="$(awk '/^version:[[:space:]]*[0-9]+[[:space:]]*$/ {print $2; exit}' "${policy_yaml}")"
[[ "${policy_version}" =~ ^[0-9]+$ ]] || fail_error 'policy version is missing or invalid'

policy_field() {
  local section="$1" field="$2"
  awk -v section="${section}" -v field="${field}" '
    $0 == section ":" { in_section = 1; next }
    in_section && /^[^[:space:]#]/ { exit }
    in_section && $0 ~ "^[[:space:]][[:space:]]" field ":[[:space:]]*" {
      value = $0
      sub("^[[:space:]][[:space:]]" field ":[[:space:]]*", "", value)
      sub(/[[:space:]]+$/, "", value)
      if (value ~ /^".*"$/ || value ~ /^\047.*\047$/) {
        value = substr(value, 2, length(value) - 2)
      }
      print value
      exit
    }
  ' "${policy_yaml}"
}

policy_repository="$(policy_field upstream repository)"
policy_branch="$(policy_field upstream branch)"
policy_state_path="$(policy_field sync state_file)"
policy_matcher="$(policy_field matching enforced_fork_owned_subset)"
[[ "${policy_repository}" == "${expected_repository}" ]] ||
  fail_error 'policy upstream repository does not match the requested repository'
[[ "${policy_branch}" == "${expected_branch}" ]] ||
  fail_error 'policy upstream branch does not match the requested branch'
[[ "${policy_state_path}" == "${state_path}" ]] ||
  fail_error 'policy state path does not match the requested state file'
[[ "${policy_matcher}" == exact_or_directory_recursive ]] ||
  fail_error 'policy fork_owned matcher is not supported by this runtime'

set +e
checkpoint="$(node - \
  "$(to_node_path "${state_json}")" \
  "${expected_repository}" \
  "${expected_branch}" \
  "${policy_version}" <<'NODE'
const fs = require('node:fs');

const [path, expectedRepository, expectedBranch, policyVersion] = process.argv.slice(2);

try {
  const state = JSON.parse(fs.readFileSync(path, 'utf8'));
  const valid =
    state !== null &&
    !Array.isArray(state) &&
    state.schema_version === 1 &&
    state.repository === expectedRepository &&
    state.branch === expectedBranch &&
    state.policy_version === Number(policyVersion) &&
    typeof state.last_integrated_sha === 'string' &&
    /^[0-9a-f]{40}$/.test(state.last_integrated_sha);
  if (!valid) {
    throw new Error('state fields do not match the protected sync contract');
  }
  process.stdout.write(state.last_integrated_sha);
} catch (error) {
  console.error(`invalid upstream sync state: ${error.message}`);
  process.exit(1);
}
NODE
)"
state_status=$?
set -e
((state_status == 0)) || {
  printf 'state=error\n'
  exit 3
}

bounded_tree_git rev-parse --verify "${checkpoint}^{commit}" >/dev/null 2>&1 ||
  fail_error "checkpoint commit is unavailable: ${checkpoint}"

set +e
bounded_tree_git merge-base --is-ancestor "${checkpoint}" "${upstream_tip}"
ancestor_status=$?
set -e
case "${ancestor_status}" in
  0) ;;
  1)
    printf 'state=history_rewrite\n'
    exit 2
    ;;
  *) fail_error 'unable to verify upstream checkpoint ancestry' ;;
esac

mapfile -t fork_patterns < <(
  awk '
    /^fork_owned:[[:space:]]*$/ { in_section = 1; next }
    in_section && /^[^[:space:]#]/ { exit }
    in_section && /^[[:space:]]*-[[:space:]]*/ {
      value = $0
      sub(/^[[:space:]]*-[[:space:]]*/, "", value)
      sub(/[[:space:]]+$/, "", value)
      print value
    }
  ' "${policy_yaml}"
)
((${#fork_patterns[@]} > 0)) || fail_error 'fork_owned policy must contain at least one path'

contains_glob() {
  [[ "$1" == *'*'* || "$1" == *'?'* || "$1" == *'['* ]]
}

for index in "${!fork_patterns[@]}"; do
  pattern="${fork_patterns[${index}]}"
  if [[ "${pattern}" == \"*\" && "${pattern}" == *\" ]]; then
    pattern="${pattern:1:${#pattern}-2}"
  elif [[ "${pattern}" == \'*\' && "${pattern}" == *\' ]]; then
    pattern="${pattern:1:${#pattern}-2}"
  fi
  valid_repo_path "${pattern}" || fail_error "invalid fork_owned pattern: ${pattern}"
  if [[ "${pattern}" == *'/**' ]]; then
    prefix="${pattern%/**}"
    contains_glob "${prefix}" &&
      fail_error "unsupported fork_owned pattern: ${pattern}"
  elif contains_glob "${pattern}"; then
    fail_error "unsupported fork_owned pattern: ${pattern}"
  fi
  fork_patterns[${index}]="${pattern}"
done

printf 'checkpoint=%s\n' "${checkpoint}"

path_is_fork_owned() {
  local path="$1" pattern prefix
  for pattern in "${fork_patterns[@]}"; do
    if [[ "${pattern}" == *'/**' ]]; then
      prefix="${pattern%/**}"
      [[ "${path}" == "${prefix}/"* ]] && return 0
    elif [[ "${path}" == "${pattern}" ]]; then
      return 0
    fi
  done
  return 1
}

if [[ "${checkpoint}" == "${upstream_tip}" ]]; then
  printf 'state=noop\n'
  printf 'tree=%s\n' "$(bounded_tree_git rev-parse "${fork_base}^{tree}")"
  printf 'filtered_paths=0\n'
  exit 0
fi

bounded_tree_git diff --no-renames --name-only -z "${checkpoint}" "${upstream_tip}" -- >"${changed_paths}" ||
  fail_error 'unable to enumerate upstream changes'
GIT_INDEX_FILE="${filtered_index}" bounded_tree_git read-tree "${upstream_tip}"

filtered_count=0
while IFS= read -r -d '' path; do
  path_is_fork_owned "${path}" || continue
  entry="$(bounded_tree_git ls-tree "${checkpoint}" -- "${path}" | awk 'NR == 1 { print $1, $3 }')"
  if [[ -n "${entry}" ]]; then
    read -r mode object extra <<<"${entry}"
    [[ -n "${mode}" && -n "${object}" && -z "${extra:-}" ]] ||
      fail_error "unable to read checkpoint entry: ${path}"
    GIT_INDEX_FILE="${filtered_index}" bounded_tree_git update-index \
      --add --cacheinfo "${mode}" "${object}" "${path}"
  else
    GIT_INDEX_FILE="${filtered_index}" bounded_tree_git update-index --force-remove -- "${path}"
  fi
  ((filtered_count += 1))
done <"${changed_paths}"

filtered_tree="$(GIT_INDEX_FILE="${filtered_index}" bounded_tree_git write-tree)"
upstream_date="$(bounded_tree_git show -s --format=%cI "${upstream_tip}")"
synthetic_commit="$({
  printf 'Filtered upstream tree for checkpoint sync\n\n'
  printf 'Source: %s\n' "${upstream_tip}"
} | GIT_AUTHOR_NAME='aeris-sync' \
  GIT_AUTHOR_EMAIL='aeris-sync@invalid' \
  GIT_AUTHOR_DATE="${upstream_date}" \
  GIT_COMMITTER_NAME='aeris-sync' \
  GIT_COMMITTER_EMAIL='aeris-sync@invalid' \
  GIT_COMMITTER_DATE="${upstream_date}" \
  bounded_tree_git commit-tree "${filtered_tree}" -p "${checkpoint}")"

set +e
merge_output="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" "${CHECKPOINT_HELPER}" \
  "${checkpoint}" "${fork_base}" "${synthetic_commit}")"
merge_status=$?
set -e
if ((merge_status != 0)); then
  printf '%s\n' "${merge_output}"
  exit "${merge_status}"
fi

merged_tree="$(sed -n 's/^tree=//p' <<<"${merge_output}")"
bounded_tree_git rev-parse --verify "${merged_tree}^{tree}" >/dev/null 2>&1 ||
  fail_error 'checkpoint helper did not return a valid tree'

node - "$(to_node_path "${state_json}")" "$(to_node_path "${updated_state}")" "${upstream_tip}" <<'NODE'
const fs = require('node:fs');

const [source, destination, upstreamTip] = process.argv.slice(2);
const state = JSON.parse(fs.readFileSync(source, 'utf8'));
state.last_integrated_sha = upstreamTip;
fs.writeFileSync(destination, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
NODE

state_entry="$(bounded_tree_git ls-tree "${fork_base}" -- "${state_path}" | awk 'NR == 1 { print $1, $3 }')"
read -r state_mode state_object state_extra <<<"${state_entry}"
[[ "${state_mode}" == 100644 && -n "${state_object}" && -z "${state_extra:-}" ]] ||
  fail_error 'state file must be a regular non-executable file'
updated_state_object="$(bounded_tree_git hash-object -w "${updated_state}")"

GIT_INDEX_FILE="${final_index}" bounded_tree_git read-tree "${merged_tree}"
GIT_INDEX_FILE="${final_index}" bounded_tree_git update-index \
  --add --cacheinfo 100644 "${updated_state_object}" "${state_path}"
final_tree="$(GIT_INDEX_FILE="${final_index}" bounded_tree_git write-tree)"

printf 'state=clean\n'
printf 'tree=%s\n' "${final_tree}"
printf 'filtered_paths=%s\n' "${filtered_count}"
