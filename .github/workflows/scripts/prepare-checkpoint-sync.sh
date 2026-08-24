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
CONFLICT_RUNTIME="${CONFLICT_RUNTIME:-${SCRIPT_ROOT}/../../automation/src/sync-conflict-review.mjs}"

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
  git rev-parse --verify "${ref}^{commit}" >/dev/null 2>&1 ||
    fail_error "invalid ${label} commit: ${ref}"
done
fork_base="$(git rev-parse "${fork_base}^{commit}")"
upstream_tip="$(git rev-parse "${upstream_tip}^{commit}")"

tmp_root="${AERIS_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${tmp_root}"
work_dir="$(mktemp -d "${tmp_root%/}/aeris-prepare-sync.XXXXXX")"
state_json="${work_dir}/state.json"
policy_yaml="${work_dir}/policy.yml"
changed_paths="${work_dir}/changed-paths"
filtered_index="${work_dir}/filtered.index"
final_index="${work_dir}/final.index"
updated_state="${work_dir}/updated-state.json"
review_patterns_file="${work_dir}/review-required"
sensitive_patterns_file="${work_dir}/sensitive"
generated_patterns_file="${work_dir}/generated"
upstream_patterns_file="${work_dir}/upstream-owned"
fork_patterns_file="${work_dir}/fork-owned"

cleanup() {
  printf 'Removing temporary workspace: %s\n' "${work_dir}" >&2
  rm -f -- \
    "${state_json}" "${policy_yaml}" "${changed_paths}" \
    "${filtered_index}" "${filtered_index}.lock" \
    "${final_index}" "${final_index}.lock" "${updated_state}" \
    "${review_patterns_file}" "${sensitive_patterns_file}" \
    "${generated_patterns_file}" "${upstream_patterns_file}" \
    "${fork_patterns_file}"
  rmdir -- "${work_dir}" 2>/dev/null || true
}
trap cleanup EXIT

git show "${fork_base}:${state_path}" >"${state_json}" 2>/dev/null ||
  fail_error "state file is missing from fork base: ${state_path}"
git show "${fork_base}:${policy_path}" >"${policy_yaml}" 2>/dev/null ||
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

policy_nested_field() {
  local section="$1" subsection="$2" field="$3"
  awk -v section="${section}" -v subsection="${subsection}" -v field="${field}" '
    $0 == section ":" { in_section = 1; next }
    in_section && /^[^[:space:]#]/ { exit }
    in_section && $0 == "  " subsection ":" { in_subsection = 1; next }
    in_subsection && /^  [^[:space:]#]/ { exit }
    in_subsection && $0 ~ "^[[:space:]][[:space:]][[:space:]][[:space:]]" field ":[[:space:]]*" {
      value = $0
      sub("^[[:space:]][[:space:]][[:space:]][[:space:]]" field ":[[:space:]]*", "", value)
      sub(/[[:space:]]+$/, "", value)
      if (value ~ /^".*"$/ || value ~ /^\047.*\047$/) {
        value = substr(value, 2, length(value) - 2)
      }
      print value
      exit
    }
  ' "${policy_yaml}"
}

require_ai_resolution_policy() {
  local enabled profile pre_conflict_verdict allowed_type allowed_mode maximum_files
  local maximum_bytes_per_file maximum_total_input_bytes resolver_model_variable
  local reviewer_model_variable distinct_models complete_resolution independent_review
  local non_conflict_edits sensitive_paths ambiguous_changes

  enabled="$(policy_nested_field conflicts ai_resolution enabled)"
  profile="$(policy_nested_field conflicts ai_resolution profile)"
  pre_conflict_verdict="$(policy_nested_field conflicts ai_resolution required_pre_conflict_verdict)"
  allowed_type="$(policy_nested_field conflicts ai_resolution allowed_type)"
  allowed_mode="$(policy_nested_field conflicts ai_resolution allowed_mode)"
  maximum_files="$(policy_nested_field conflicts ai_resolution maximum_files)"
  maximum_bytes_per_file="$(policy_nested_field conflicts ai_resolution maximum_bytes_per_file)"
  maximum_total_input_bytes="$(policy_nested_field conflicts ai_resolution maximum_total_input_bytes)"
  resolver_model_variable="$(policy_nested_field conflicts ai_resolution resolver_model_variable)"
  reviewer_model_variable="$(policy_nested_field conflicts ai_resolution reviewer_model_variable)"
  distinct_models="$(policy_nested_field conflicts ai_resolution require_distinct_model_ids)"
  complete_resolution="$(policy_nested_field conflicts ai_resolution require_complete_resolution)"
  independent_review="$(policy_nested_field conflicts ai_resolution require_independent_review_pass)"
  non_conflict_edits="$(policy_nested_field conflicts ai_resolution allow_non_conflict_edits)"
  sensitive_paths="$(policy_nested_field conflicts ai_resolution allow_sensitive_or_review_required_paths)"
  ambiguous_changes="$(policy_nested_field conflicts ai_resolution allow_binary_rename_delete_mode_or_case_ambiguity)"

  [[ "${enabled}" == true && "${profile}" == aeris-sync-conflict-v1 &&
     "${pre_conflict_verdict}" == eligible && "${allowed_type}" == modify_modify_utf8_text &&
     "${allowed_mode}" == 100644 && "${maximum_files}" == 4 &&
     "${maximum_bytes_per_file}" == 16384 && "${maximum_total_input_bytes}" == 65536 &&
     "${resolver_model_variable}" == AERIS_AI_MODEL_CONFLICT_RESOLVER &&
     "${reviewer_model_variable}" == AERIS_AI_MODEL_CONFLICT_REVIEWER &&
     "${distinct_models}" == true && "${complete_resolution}" == true &&
     "${independent_review}" == true && "${non_conflict_edits}" == false &&
     "${sensitive_paths}" == false && "${ambiguous_changes}" == false ]] ||
    fail_error 'AI conflict resolution policy is disabled, incomplete, or unsupported by this runtime'
}

policy_repository="$(policy_field upstream repository)"
policy_branch="$(policy_field upstream branch)"
policy_state_path="$(policy_field sync state_file)"
policy_fail_closed="$(policy_field sync fail_closed)"
policy_autonomous_merge="$(policy_field sync autonomous_merge)"
policy_matcher="$(policy_field matching enforced_fork_owned_subset)"
policy_syntax="$(policy_field matching syntax)"
policy_default="$(policy_field matching default)"
[[ "${policy_repository}" == "${expected_repository}" ]] ||
  fail_error 'policy upstream repository does not match the requested repository'
[[ "${policy_branch}" == "${expected_branch}" ]] ||
  fail_error 'policy upstream branch does not match the requested branch'
[[ "${policy_state_path}" == "${state_path}" ]] ||
  fail_error 'policy state path does not match the requested state file'
[[ "${policy_fail_closed}" == true ]] || fail_error 'sync policy must fail closed'
[[ "${policy_autonomous_merge}" == eligible || "${policy_autonomous_merge}" == manual ]] ||
  fail_error 'sync autonomous_merge policy must be eligible or manual'
[[ "${policy_matcher}" == exact_or_directory_recursive ]] ||
  fail_error 'policy fork_owned matcher is not supported by this runtime'
[[ "${policy_syntax}" == aeris-glob-v1 && "${policy_default}" == review_required ]] ||
  fail_error 'sync policy matching contract is not fail closed'
mapfile -t policy_precedence < <(
  awk '
    $0 == "  precedence:" { in_section = 1; next }
    in_section && /^  [^[:space:]-]/ { exit }
    in_section && /^[[:space:]]*-[[:space:]]*/ {
      value = $0
      sub(/^[[:space:]]*-[[:space:]]*/, "", value)
      print value
    }
  ' "${policy_yaml}"
)
[[ "${policy_precedence[*]}" == 'sensitive review_required fork_owned generated upstream_owned' ]] ||
  fail_error 'sync policy precedence is not supported by this runtime'

write_policy_patterns() {
  local section="$1" destination="$2"
  awk -v section="${section}" '
    $0 == section ":" { in_section = 1; next }
    in_section && /^[^[:space:]#]/ { exit }
    in_section && /^[[:space:]]*-[[:space:]]*/ {
      value = $0
      sub(/^[[:space:]]*-[[:space:]]*/, "", value)
      sub(/[[:space:]]+$/, "", value)
      if (value ~ /^".*"$/ || value ~ /^\047.*\047$/) value = substr(value, 2, length(value) - 2)
      print value
    }
  ' "${policy_yaml}" >"${destination}"
}
write_policy_patterns review_required "${review_patterns_file}"
write_policy_patterns sensitive "${sensitive_patterns_file}"
write_policy_patterns generated "${generated_patterns_file}"
write_policy_patterns upstream_owned "${upstream_patterns_file}"
write_policy_patterns fork_owned "${fork_patterns_file}"
[[ -s "${review_patterns_file}" ]] || fail_error 'review_required policy must contain at least one path'
[[ -s "${sensitive_patterns_file}" ]] || fail_error 'sensitive policy must contain at least one path'
[[ -s "${upstream_patterns_file}" ]] || fail_error 'upstream_owned policy must contain at least one path'

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

git rev-parse --verify "${checkpoint}^{commit}" >/dev/null 2>&1 ||
  fail_error "checkpoint commit is unavailable: ${checkpoint}"

set +e
git merge-base --is-ancestor "${checkpoint}" "${upstream_tip}"
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
  if [[ -z "${pattern}" || "${pattern}" == */ || "${pattern}" == /* || "${pattern}" == *'\\'* || "${pattern}" == *'['* || "${pattern}" == *']'* || "${pattern}" == '!'* ]]; then
    fail_error "unsupported policy pattern: ${pattern}"
  elif [[ "${pattern}" == *'/**' ]]; then
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
  printf 'tree=%s\n' "$(git rev-parse "${fork_base}^{tree}")"
  printf 'filtered_paths=0\n'
  printf 'autonomous_eligible=false\n'
  printf 'policy_verdict=noop\n'
  exit 0
fi

git diff --no-renames --name-only -z "${checkpoint}" "${upstream_tip}" -- >"${changed_paths}" ||
  fail_error 'unable to enumerate upstream changes'

# Classify the complete upstream backlog before fork-owned filtering. A review
# verdict can publish for humans; sensitive and unknown paths cannot publish.
policy_result="$(node - \
  "$(to_node_path "${changed_paths}")" \
  "$(to_node_path "${review_patterns_file}")" \
  "$(to_node_path "${sensitive_patterns_file}")" \
  "$(to_node_path "${generated_patterns_file}")" \
  "$(to_node_path "${upstream_patterns_file}")" \
  "$(to_node_path "${fork_patterns_file}")" <<'NODE'
const fs = require('node:fs');
const [changed, reviewFile, sensitiveFile, generatedFile, upstreamFile, forkFile] = process.argv.slice(2);
const readLines = (file) => fs.readFileSync(file, 'utf8').split(/\r?\n/).filter(Boolean);
const paths = fs.readFileSync(changed).toString('utf8').split('\0').filter(Boolean);
const patterns = {
  review: readLines(reviewFile), sensitive: readLines(sensitiveFile),
  generated: readLines(generatedFile), upstream: readLines(upstreamFile), fork: readLines(forkFile),
};
function regex(pattern) {
  if (!pattern || pattern.startsWith('!') || pattern.startsWith('/') || pattern.endsWith('/') || pattern.includes('\\') || pattern.includes('[') || pattern.includes(']')) throw new Error(`unsupported policy pattern: ${pattern}`);
  let source = '';
  for (let i = 0; i < pattern.length; i += 1) {
    const c = pattern[i];
    if (c === '*' && pattern[i + 1] === '*' && pattern[i + 2] === '/') { source += '(?:.*/)?'; i += 2; }
    else if (c === '*' && pattern[i + 1] === '*') { source += '.*'; i += 1; }
    else if (c === '*') source += '[^/]*';
    else if (c === '?') source += '[^/]';
    else if ('\\.^$+{}()|[]'.includes(c)) source += `\\${c}`;
    else source += c;
  }
  if (!pattern.includes('/')) source = `(?:.*/)?${source}`;
  return new RegExp(`^${source}(?:/.*)?$`);
}
const compiled = Object.fromEntries(Object.entries(patterns).map(([key, values]) => [key, values.map(regex)]));
const matches = (kind, path) => compiled[kind].some((matcher) => matcher.test(path));
let manual = false;
for (const path of paths) {
  if (matches('sensitive', path)) throw new Error(`sensitive upstream path: ${path}`);
  if (matches('review', path)) manual = true;
  if (!matches('fork', path) && !matches('review', path) && !matches('generated', path) && !matches('upstream', path)) {
    manual = true;
  }
}
const verdict = manual ? 'manual_review' : 'eligible';
process.stdout.write(verdict);
NODE
)" || fail_error 'upstream policy classification failed closed'
[[ "${policy_result}" == eligible || "${policy_result}" == manual_review ]] ||
  fail_error 'upstream policy classification returned an invalid verdict'
if [[ "${policy_autonomous_merge}" == manual ]]; then
  policy_result=manual_review
fi
GIT_INDEX_FILE="${filtered_index}" git read-tree "${upstream_tip}"

filtered_count=0
while IFS= read -r -d '' path; do
  path_is_fork_owned "${path}" || continue
  entry="$(git ls-tree "${checkpoint}" -- "${path}" | awk 'NR == 1 { print $1, $3 }')"
  if [[ -n "${entry}" ]]; then
    read -r mode object extra <<<"${entry}"
    [[ -n "${mode}" && -n "${object}" && -z "${extra:-}" ]] ||
      fail_error "unable to read checkpoint entry: ${path}"
    GIT_INDEX_FILE="${filtered_index}" git update-index \
      --add --cacheinfo "${mode}" "${object}" "${path}"
  else
    GIT_INDEX_FILE="${filtered_index}" git update-index --force-remove -- "${path}"
  fi
  ((filtered_count += 1))
done <"${changed_paths}"

filtered_tree="$(GIT_INDEX_FILE="${filtered_index}" git write-tree)"
upstream_date="$(git show -s --format=%cI "${upstream_tip}")"
synthetic_commit="$({
  printf 'Filtered upstream tree for checkpoint sync\n\n'
  printf 'Source: %s\n' "${upstream_tip}"
} | GIT_AUTHOR_NAME='aeris-sync' \
  GIT_AUTHOR_EMAIL='aeris-sync@invalid' \
  GIT_AUTHOR_DATE="${upstream_date}" \
  GIT_COMMITTER_NAME='aeris-sync' \
  GIT_COMMITTER_EMAIL='aeris-sync@invalid' \
  GIT_COMMITTER_DATE="${upstream_date}" \
  git commit-tree "${filtered_tree}" -p "${checkpoint}")"

set +e
merge_output="$("${CHECKPOINT_HELPER}" \
  "${checkpoint}" "${fork_base}" "${synthetic_commit}")"
merge_status=$?
set -e
conflict_resolved=false
conflict_bundle_sha=''
conflict_candidate_sha=''
conflict_generation_sha=''
conflict_resolution_sha=''
conflict_resolved_tree=''
conflict_resolver_model_sha=''
if ((merge_status != 0)); then
  merge_state="$(sed -n 's/^state=//p' <<<"${merge_output}" | tail -n1)"
  if [[ "${merge_status}:${merge_state}" != 1:conflict ]]; then
    printf '%s\n' "${merge_output}"
    exit "${merge_status}"
  fi
  if [[ "${policy_result}" != eligible ]]; then
    printf '%s\n' "${merge_output}"
    exit 1
  fi

  conflict_bundle_path="${AERIS_CONFLICT_BUNDLE_PATH:-}"
  conflict_candidate_path="${AERIS_CONFLICT_CANDIDATE_PATH:-}"
  if [[ -n "${conflict_candidate_path}" && -z "${conflict_bundle_path}" ]]; then
    fail_error 'a conflict candidate requires its exact conflict bundle'
  fi
  if [[ -z "${conflict_bundle_path}" ]]; then
    printf '%s\n' "${merge_output}"
    exit 1
  fi

  require_ai_resolution_policy

  conflict_environment=(
    GITHUB_OUTPUT=
    GITHUB_REPOSITORY="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required for conflict resolution}"
    GITHUB_REPOSITORY_ID="${GITHUB_REPOSITORY_ID:?GITHUB_REPOSITORY_ID is required for conflict resolution}"
    BASE_BRANCH="${BASE_BRANCH:-main}"
    SYNC_BRANCH="${SYNC_BRANCH:-automation/sync-upstream}"
    AERIS_CONFLICT_BASE_SHA="${fork_base}"
    AERIS_CONFLICT_CHECKPOINT_SHA="${checkpoint}"
    AERIS_CONFLICT_UPSTREAM_REPOSITORY="${expected_repository}"
    AERIS_CONFLICT_UPSTREAM_REF="${expected_branch}"
    AERIS_CONFLICT_UPSTREAM_SHA="${upstream_tip}"
    AERIS_CONFLICT_SYNTHETIC_COMMIT_SHA="${synthetic_commit}"
    AERIS_CONFLICT_POLICY_PATH="${policy_path}"
    AERIS_CONFLICT_STATE_PATH="${state_path}"
    AERIS_SYNC_POLICY_VERDICT="${policy_result}"
  )

  if [[ -z "${conflict_candidate_path}" ]]; then
    set +e
    conflict_output="$(env "${conflict_environment[@]}" \
      AERIS_CONFLICT_OUTPUT_PATH="${conflict_bundle_path}" \
      node "${CONFLICT_RUNTIME}" prepare)"
    conflict_status=$?
    set -e
    ((conflict_status == 0)) || fail_error 'trusted conflict bundle generation failed'
    conflict_bundle_sha="$(sed -n 's/^conflict_bundle_sha=//p' <<<"${conflict_output}" | tail -n1)"
    conflict_generation_sha="$(sed -n 's/^conflict_generation_sha=//p' <<<"${conflict_output}" | tail -n1)"
    [[ "${conflict_bundle_sha}" =~ ^[0-9a-f]{64}$ && "${conflict_generation_sha}" =~ ^[0-9a-f]{64}$ ]] ||
      fail_error 'trusted conflict bundle hashes are invalid'
    printf 'policy_verdict=eligible\n'
    printf 'autonomous_eligible=false\n'
    printf 'conflict_bundle_sha=%s\n' "${conflict_bundle_sha}"
    printf 'conflict_generation_sha=%s\n' "${conflict_generation_sha}"
    printf 'state=conflict\n'
    exit 1
  fi

  set +e
  conflict_output="$(env "${conflict_environment[@]}" \
    AERIS_CONFLICT_BUNDLE_PATH="${conflict_bundle_path}" \
    AERIS_CONFLICT_CANDIDATE_PATH="${conflict_candidate_path}" \
    node "${CONFLICT_RUNTIME}" materialize)"
  conflict_status=$?
  set -e
  ((conflict_status == 0)) || fail_error 'conflict resolution candidate failed trusted replay'
  conflict_bundle_sha="$(sed -n 's/^bundle_sha=//p' <<<"${conflict_output}" | tail -n1)"
  conflict_candidate_sha="$(sed -n 's/^candidate_sha=//p' <<<"${conflict_output}" | tail -n1)"
  conflict_generation_sha="$(sed -n 's/^generation_sha=//p' <<<"${conflict_output}" | tail -n1)"
  conflict_resolution_sha="$(sed -n 's/^resolution_sha=//p' <<<"${conflict_output}" | tail -n1)"
  conflict_resolved_tree="$(sed -n 's/^resolved_merge_tree_sha=//p' <<<"${conflict_output}" | tail -n1)"
  conflict_resolver_model_sha="$(sed -n 's/^resolver_model_sha=//p' <<<"${conflict_output}" | tail -n1)"
  for value in "${conflict_bundle_sha}" "${conflict_candidate_sha}" "${conflict_generation_sha}" \
    "${conflict_resolution_sha}" "${conflict_resolver_model_sha}"; do
    [[ "${value}" =~ ^[0-9a-f]{64}$ ]] || fail_error 'trusted conflict resolution hash is invalid'
  done
  git rev-parse --verify "${conflict_resolved_tree}^{tree}" >/dev/null 2>&1 ||
    fail_error 'trusted conflict resolution tree is invalid'
  merged_tree="${conflict_resolved_tree}"
  policy_result=conflict_ai_review
  conflict_resolved=true
else
  merged_tree="$(sed -n 's/^tree=//p' <<<"${merge_output}")"
  git rev-parse --verify "${merged_tree}^{tree}" >/dev/null 2>&1 ||
    fail_error 'checkpoint helper did not return a valid tree'
fi

node - "$(to_node_path "${state_json}")" "$(to_node_path "${updated_state}")" "${upstream_tip}" <<'NODE'
const fs = require('node:fs');

const [source, destination, upstreamTip] = process.argv.slice(2);
const state = JSON.parse(fs.readFileSync(source, 'utf8'));
state.last_integrated_sha = upstreamTip;
fs.writeFileSync(destination, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
NODE

state_entry="$(git ls-tree "${fork_base}" -- "${state_path}" | awk 'NR == 1 { print $1, $3 }')"
read -r state_mode state_object state_extra <<<"${state_entry}"
[[ "${state_mode}" == 100644 && -n "${state_object}" && -z "${state_extra:-}" ]] ||
  fail_error 'state file must be a regular non-executable file'
updated_state_object="$(git hash-object -w "${updated_state}")"

GIT_INDEX_FILE="${final_index}" git read-tree "${merged_tree}"
GIT_INDEX_FILE="${final_index}" git update-index \
  --add --cacheinfo 100644 "${updated_state_object}" "${state_path}"
final_tree="$(GIT_INDEX_FILE="${final_index}" git write-tree)"

printf 'policy_verdict=%s\n' "${policy_result}"
if [[ "${policy_result}" == eligible || "${policy_result}" == conflict_ai_review ]]; then
  printf 'autonomous_eligible=true\n'
else
  printf 'autonomous_eligible=false\n'
fi

if [[ "${conflict_resolved}" == true ]]; then
  printf 'conflict_bundle_sha=%s\n' "${conflict_bundle_sha}"
  printf 'conflict_candidate_sha=%s\n' "${conflict_candidate_sha}"
  printf 'conflict_generation_sha=%s\n' "${conflict_generation_sha}"
  printf 'conflict_resolution_sha=%s\n' "${conflict_resolution_sha}"
  printf 'conflict_resolved_tree=%s\n' "${conflict_resolved_tree}"
  printf 'conflict_resolver_model_sha=%s\n' "${conflict_resolver_model_sha}"
fi

printf 'state=clean\n'
printf 'tree=%s\n' "${final_tree}"
printf 'filtered_paths=%s\n' "${filtered_count}"
