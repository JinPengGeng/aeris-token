#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="${SCRIPT_ROOT}/prepare-checkpoint-sync.sh"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-prepare-sync-test.XXXXXX")"
mkdir -p "${RUN_ROOT}/tmp"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

assert_file_missing() {
  local tree="$1" path="$2"
  if git cat-file -e "${tree}:${path}" 2>/dev/null; then
    fail "${path} should not exist in ${tree}"
  fi
}

new_repo() {
  local path="$1"
  mkdir -p "${path}"
  git -C "${path}" init -q
  git -C "${path}" config user.name 'Prepare Sync Test'
  git -C "${path}" config user.email 'prepare-sync@example.com'
  git -C "${path}" config core.autocrlf false
}

write_state() {
  local sha="$1" repository="${2:-example/Upstream}" branch="${3:-main}"
  mkdir -p .github
  printf '{"schema_version":1,"repository":"%s","branch":"%s","last_integrated_sha":"%s","policy_version":1}\n' \
    "${repository}" "${branch}" "${sha}" >.github/upstream-sync-state.json
}

write_policy() {
  mkdir -p .github
  cat >.github/upstream-sync-policy.yml <<'YAML'
version: 1
upstream:
  repository: example/Upstream
  branch: main
sync:
  state_file: .github/upstream-sync-state.json
  fail_closed: true
  autonomous_merge: eligible
matching:
  syntax: aeris-glob-v1
  enforced_fork_owned_subset: exact_or_directory_recursive
  precedence:
    - sensitive
    - review_required
    - fork_owned
    - generated
    - upstream_owned
  default: review_required
fork_owned:
  - .github/upstream-sync-policy.yml
  - .github/upstream-sync-state.json
  - .github/automation-policy.yml
  - .github/workflows/**
review_required:
  - .github/**
sensitive:
  - .gitmodules
  - "**/*.pem"
  - "**/*.key"
  - "**/*.p12"
generated: []
upstream_owned:
  - "**"
conflicts:
  overwrite_unknown_tip: false
  create_or_update_alert: true
  preserve_existing_branch_and_pr: true
  require_explicit_adoption_of_resolution: true
  ai_resolution:
    enabled: true
    profile: aeris-sync-conflict-v1
    required_pre_conflict_verdict: eligible
    allowed_type: modify_modify_utf8_text
    allowed_mode: "100644"
    maximum_files: 4
    maximum_bytes_per_file: 16384
    maximum_total_input_bytes: 65536
    resolver_model_variable: AERIS_AI_MODEL_CONFLICT_RESOLVER
    reviewer_model_variable: AERIS_AI_MODEL_CONFLICT_REVIEWER
    require_distinct_model_ids: true
    require_complete_resolution: true
    require_independent_review_pass: true
    allow_non_conflict_edits: false
    allow_sensitive_or_review_required_paths: false
    allow_binary_rename_delete_mode_or_case_ambiguity: false
YAML
}

write_policy_with_fork_owned() {
  local fork_owned="$1"
  mkdir -p .github
  cat >.github/upstream-sync-policy.yml <<YAML
version: 1
upstream:
  repository: example/Upstream
  branch: main
sync:
  state_file: .github/upstream-sync-state.json
  fail_closed: true
  autonomous_merge: eligible
matching:
  syntax: aeris-glob-v1
  enforced_fork_owned_subset: exact_or_directory_recursive
  precedence:
    - sensitive
    - review_required
    - fork_owned
    - generated
    - upstream_owned
  default: review_required
fork_owned:
  - ${fork_owned}
review_required:
  - .github/**
sensitive:
  - .gitmodules
  - "**/*.pem"
  - "**/*.key"
  - "**/*.p12"
generated: []
upstream_owned:
  - "**"
conflicts:
  overwrite_unknown_tip: false
  create_or_update_alert: true
  preserve_existing_branch_and_pr: true
  require_explicit_adoption_of_resolution: true
  ai_resolution:
    enabled: true
    profile: aeris-sync-conflict-v1
    required_pre_conflict_verdict: eligible
    allowed_type: modify_modify_utf8_text
    allowed_mode: "100644"
    maximum_files: 4
    maximum_bytes_per_file: 16384
    maximum_total_input_bytes: 65536
    resolver_model_variable: AERIS_AI_MODEL_CONFLICT_RESOLVER
    reviewer_model_variable: AERIS_AI_MODEL_CONFLICT_REVIEWER
    require_distinct_model_ids: true
    require_complete_resolution: true
    require_independent_review_pass: true
    allow_non_conflict_edits: false
    allow_sensitive_or_review_required_paths: false
    allow_binary_rename_delete_mode_or_case_ambiguity: false
YAML
}

prepare() {
  AERIS_TMP_ROOT="${RUN_ROOT}/tmp" "${HELPER}" \
    "$1" "$2" example/Upstream main
}

tree_from_output() {
  sed -n 's/^tree=//p' <<<"$1"
}

test_squash_checkpoint_noop() {
  local repo="${RUN_ROOT}/noop" root checkpoint main output tree
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >app.txt
  git add app.txt
  git commit -qm 'base'
  root="$(git rev-parse HEAD)"

  git switch -qc upstream
  printf 'upstream checkpoint\n' >app.txt
  git commit -qam 'upstream checkpoint'
  checkpoint="$(git rev-parse HEAD)"

  git switch -qc main "${root}"
  printf 'upstream checkpoint\n' >app.txt
  write_state "${checkpoint}"
  write_policy
  git add .
  git commit -qm 'squash checkpoint and add state'
  main="$(git rev-parse HEAD)"

  if git merge-base --is-ancestor "${checkpoint}" "${main}"; then
    fail 'test setup must not retain upstream ancestry'
  fi
  output="$(prepare "${main}" upstream)"
  assert_eq noop "$(sed -n 's/^state=//p' <<<"${output}")" 'checkpoint no-op state'
  tree="$(tree_from_output "${output}")"
  assert_eq "$(git rev-parse "${main}^{tree}")" "${tree}" 'checkpoint no-op tree'
}

test_fork_owned_filter_and_state_advance() {
  local repo="${RUN_ROOT}/filter" root checkpoint upstream_tip main output tree state_sha
  new_repo "${repo}"
  cd "${repo}"

  mkdir -p .github/workflows
  printf 'base\n' >app.txt
  printf 'upstream workflow v0\n' >.github/workflows/release.yml
  git add .
  git commit -qm 'base'
  root="$(git rev-parse HEAD)"

  git switch -qc upstream
  printf 'checkpoint\n' >app.txt
  git commit -qam 'upstream checkpoint'
  checkpoint="$(git rev-parse HEAD)"

  git switch -qc main "${root}"
  printf 'checkpoint\n' >app.txt
  printf 'fork workflow\n' >.github/workflows/release.yml
  printf 'fork policy\n' >.github/automation-policy.yml
  write_state "${checkpoint}"
  write_policy
  git add .
  git commit -qm 'squash checkpoint with fork policy'
  main="$(git rev-parse HEAD)"

  git switch -q upstream
  printf 'upstream next\n' >app.txt
  printf 'upstream workflow v1\n' >.github/workflows/release.yml
  printf 'upstream policy\n' >.github/automation-policy.yml
  printf 'new upstream workflow\n' >.github/workflows/new.yml
  git add .
  git commit -qm 'upstream next'
  upstream_tip="$(git rev-parse HEAD)"

  output="$(prepare "${main}" "${upstream_tip}")"
  assert_eq clean "$(sed -n 's/^state=//p' <<<"${output}")" 'filtered merge state'
  assert_eq false "$(sed -n 's/^autonomous_eligible=//p' <<<"${output}")" \
    'review-required backlog autonomous eligibility'
  assert_eq manual_review "$(sed -n 's/^policy_verdict=//p' <<<"${output}")" \
    'review-required backlog verdict'
  assert_eq 3 "$(sed -n 's/^filtered_paths=//p' <<<"${output}")" 'filtered path count'
  tree="$(tree_from_output "${output}")"
  assert_eq 'upstream next' "$(git show "${tree}:app.txt")" 'upstream app change'
  assert_eq 'fork workflow' "$(git show "${tree}:.github/workflows/release.yml")" \
    'fork workflow preservation'
  assert_eq 'fork policy' "$(git show "${tree}:.github/automation-policy.yml")" \
    'fork policy preservation'
  assert_file_missing "${tree}" .github/workflows/new.yml
  state_sha="$(git show "${tree}:.github/upstream-sync-state.json" |
    node -e "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>process.stdout.write(JSON.parse(d).last_integrated_sha))")"
  assert_eq "${upstream_tip}" "${state_sha}" 'result tree checkpoint advance'
  assert_eq "${checkpoint}" "$(git show "${main}:.github/upstream-sync-state.json" |
    node -e "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>process.stdout.write(JSON.parse(d).last_integrated_sha))")" \
    'fork base checkpoint must not advance'
}

test_exact_path_and_recursive_directory_filter() {
  local case_name repo root checkpoint upstream_tip main output tree expected_filtered
  local expected_foo expected_nested expected_eligible expected_verdict
  for case_name in exact recursive policy-manual; do
    repo="${RUN_ROOT}/${case_name}-path-filter"
    new_repo "${repo}"
    cd "${repo}"

    mkdir -p docs/nested
    printf 'foo v0\n' >docs/foo.md
    printf 'nested v0\n' >docs/nested/bar.md
    git add .
    git commit -qm 'base'
    root="$(git rev-parse HEAD)"

    git switch -qc upstream
    git commit --allow-empty -qm 'checkpoint'
    checkpoint="$(git rev-parse HEAD)"
    printf 'foo v1\n' >docs/foo.md
    printf 'nested v1\n' >docs/nested/bar.md
    git add docs
    git commit -qm 'upstream docs changes'
    upstream_tip="$(git rev-parse HEAD)"

    git switch -qc main "${root}"
    write_state "${checkpoint}"
    if [[ "${case_name}" == exact || "${case_name}" == policy-manual ]]; then
      write_policy_with_fork_owned 'docs/foo.md'
      expected_filtered=1
      expected_foo='foo v0'
      expected_nested='nested v1'
      if [[ "${case_name}" == policy-manual ]]; then
        sed -i 's/autonomous_merge: eligible/autonomous_merge: manual/' .github/upstream-sync-policy.yml
        expected_eligible=false
        expected_verdict=manual_review
      else
        expected_eligible=true
        expected_verdict=eligible
      fi
    else
      write_policy_with_fork_owned 'docs/**'
      expected_filtered=2
      expected_foo='foo v0'
      expected_nested='nested v0'
      expected_eligible=true
      expected_verdict=eligible
    fi
    git add .github
    git commit -qm "${case_name} fork-owned policy"
    main="$(git rev-parse HEAD)"

    output="$(prepare "${main}" "${upstream_tip}")"
    assert_eq clean "$(sed -n 's/^state=//p' <<<"${output}")" \
      "${case_name} path filter state"
    assert_eq "${expected_eligible}" "$(sed -n 's/^autonomous_eligible=//p' <<<"${output}")" \
      "${case_name} low-risk eligibility"
    assert_eq "${expected_verdict}" "$(sed -n 's/^policy_verdict=//p' <<<"${output}")" \
      "${case_name} low-risk verdict"
    assert_eq "${expected_filtered}" "$(sed -n 's/^filtered_paths=//p' <<<"${output}")" \
      "${case_name} path filter count"
    tree="$(tree_from_output "${output}")"
    assert_eq "${expected_foo}" "$(git show "${tree}:docs/foo.md")" \
      "${case_name} foo preservation"
    assert_eq "${expected_nested}" "$(git show "${tree}:docs/nested/bar.md")" \
      "${case_name} nested preservation"
  done
}

test_non_fork_conflict() {
  local repo="${RUN_ROOT}/conflict" root checkpoint upstream_tip main output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >shared.txt
  git add shared.txt
  git commit -qm 'base'
  root="$(git rev-parse HEAD)"

  git switch -qc upstream
  git commit --allow-empty -qm 'checkpoint'
  checkpoint="$(git rev-parse HEAD)"
  printf 'upstream\n' >shared.txt
  git commit -qam 'upstream conflict'
  upstream_tip="$(git rev-parse HEAD)"

  git switch -qc main "${root}"
  printf 'fork\n' >shared.txt
  write_state "${checkpoint}"
  write_policy
  git add .
  git commit -qm 'fork conflict'
  main="$(git rev-parse HEAD)"

  set +e
  output="$(prepare "${main}" "${upstream_tip}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 1 "${status}" 'non-fork conflict exit code'
  assert_eq conflict "$(sed -n 's/^state=//p' <<<"${output}")" 'non-fork conflict state'
}

test_ai_resolution_policy_controls_conflict_bundle() {
  local repo="${RUN_ROOT}/ai-policy" root checkpoint upstream_tip main output status bundle
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >shared.txt
  git add shared.txt
  git commit -qm 'base'
  root="$(git rev-parse HEAD)"

  git switch -qc upstream
  git commit --allow-empty -qm 'checkpoint'
  checkpoint="$(git rev-parse HEAD)"
  printf 'upstream\n' >shared.txt
  git commit -qam 'upstream conflict'
  upstream_tip="$(git rev-parse HEAD)"

  git switch -qc main "${root}"
  printf 'fork\n' >shared.txt
  write_state "${checkpoint}"
  write_policy
  git add .
  git commit -qm 'fork conflict with AI policy'
  main="$(git rev-parse HEAD)"
  bundle="${RUN_ROOT}/ai-policy-artifacts/bundle.json"

  set +e
  output="$(
    AERIS_TMP_ROOT="${RUN_ROOT}/tmp" \
    AERIS_ARTIFACT_ROOT="${RUN_ROOT}" \
    AERIS_CONFLICT_BUNDLE_PATH="${bundle}" \
    AERIS_AI_MODEL_CONFLICT_RESOLVER=resolver-model \
    AERIS_AI_MODEL_CONFLICT_REVIEWER=reviewer-model \
    GITHUB_REPOSITORY=example/Fork GITHUB_REPOSITORY_ID=1 \
      "${HELPER}" "${main}" "${upstream_tip}" example/Upstream main 2>/dev/null
  )"
  status=$?
  set -e
  assert_eq 1 "${status}" 'enabled AI conflict policy exit code'
  assert_eq conflict "$(sed -n 's/^state=//p' <<<"${output}")" 'enabled AI conflict policy state'
  [[ -f "${bundle}" ]] || fail 'enabled AI conflict policy did not produce a bundle'
  assert_eq sync_conflict_bundle "$(node -e "const fs=require('fs');process.stdout.write(JSON.parse(fs.readFileSync(process.argv[1],'utf8')).artifact_type)" "${bundle}")" \
    'enabled AI conflict policy artifact type'

  sed -i 's/    enabled: true/    enabled: false/' .github/upstream-sync-policy.yml
  git add .github/upstream-sync-policy.yml
  git commit -qm 'disable AI conflict policy'
  main="$(git rev-parse HEAD)"
  rm -f -- "${bundle}"
  set +e
  output="$(
    AERIS_TMP_ROOT="${RUN_ROOT}/tmp" \
    AERIS_ARTIFACT_ROOT="${RUN_ROOT}" \
    AERIS_CONFLICT_BUNDLE_PATH="${bundle}" \
    AERIS_AI_MODEL_CONFLICT_RESOLVER=resolver-model \
    AERIS_AI_MODEL_CONFLICT_REVIEWER=reviewer-model \
    GITHUB_REPOSITORY=example/Fork GITHUB_REPOSITORY_ID=1 \
      "${HELPER}" "${main}" "${upstream_tip}" example/Upstream main 2>/dev/null
  )"
  status=$?
  set -e
  assert_eq 3 "${status}" 'disabled AI conflict policy exit code'
  assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" 'disabled AI conflict policy state'
  [[ ! -e "${bundle}" ]] || fail 'disabled AI conflict policy produced a bundle'

  sed -i 's/    enabled: false/    enabled: true/' .github/upstream-sync-policy.yml
  sed -i 's/    maximum_files: 4/    maximum_files: 5/' .github/upstream-sync-policy.yml
  git add .github/upstream-sync-policy.yml
  git commit -qm 'widen unsupported AI conflict policy'
  main="$(git rev-parse HEAD)"
  set +e
  output="$(
    AERIS_TMP_ROOT="${RUN_ROOT}/tmp" \
    AERIS_ARTIFACT_ROOT="${RUN_ROOT}" \
    AERIS_CONFLICT_BUNDLE_PATH="${bundle}" \
    AERIS_AI_MODEL_CONFLICT_RESOLVER=resolver-model \
    AERIS_AI_MODEL_CONFLICT_REVIEWER=reviewer-model \
    GITHUB_REPOSITORY=example/Fork GITHUB_REPOSITORY_ID=1 \
      "${HELPER}" "${main}" "${upstream_tip}" example/Upstream main 2>/dev/null
  )"
  status=$?
  set -e
  assert_eq 3 "${status}" 'unsupported AI conflict policy limit exit code'
  assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" 'unsupported AI conflict policy limit state'
  [[ ! -e "${bundle}" ]] || fail 'unsupported AI conflict policy limit produced a bundle'
}

test_invalid_state_and_history_rewrite() {
  local repo="${RUN_ROOT}/invalid" checkpoint main rewritten output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >app.txt
  git add app.txt
  git commit -qm 'checkpoint'
  checkpoint="$(git rev-parse HEAD)"
  write_state "${checkpoint}" wrong/Repository
  write_policy
  git add .
  git commit -qm 'invalid state identity'
  main="$(git rev-parse HEAD)"

  set +e
  output="$(prepare "${main}" "${checkpoint}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 3 "${status}" 'invalid state exit code'
  assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" 'invalid state status'

  git switch -q --orphan rewritten
  printf 'rewritten\n' >app.txt
  git add app.txt
  git commit -qm 'rewritten upstream'
  rewritten="$(git rev-parse HEAD)"

  git switch -q --detach "${main}"
  write_state "${checkpoint}"
  git add .github/upstream-sync-state.json
  git commit -qm 'repair state identity'
  main="$(git rev-parse HEAD)"

  set +e
  output="$(prepare "${main}" "${rewritten}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 2 "${status}" 'history rewrite exit code'
  assert_eq history_rewrite "$(sed -n 's/^state=//p' <<<"${output}")" \
    'history rewrite state'
}

test_unsupported_policy_pattern() {
  local repo="${RUN_ROOT}/unsupported-policy" checkpoint main output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >app.txt
  git add app.txt
  git commit -qm 'checkpoint'
  checkpoint="$(git rev-parse HEAD)"
  write_state "${checkpoint}"
  mkdir -p .github
  cat >.github/upstream-sync-policy.yml <<'YAML'
version: 1
upstream:
  repository: example/Upstream
  branch: main
sync:
  state_file: .github/upstream-sync-state.json
matching:
  enforced_fork_owned_subset: exact_or_directory_recursive
fork_owned:
  - "**/*.yml"
YAML
  git add .
  git commit -qm 'unsupported policy'
  main="$(git rev-parse HEAD)"

  set +e
  output="$(prepare "${main}" "${checkpoint}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 3 "${status}" 'unsupported policy exit code'
  assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" 'unsupported policy state'
}

test_policy_identity_mismatch() {
  local repo="${RUN_ROOT}/policy-identity" checkpoint main output status
  new_repo "${repo}"
  cd "${repo}"

  printf 'base\n' >app.txt
  git add app.txt
  git commit -qm 'checkpoint'
  checkpoint="$(git rev-parse HEAD)"
  write_state "${checkpoint}"
  write_policy
  sed -i 's#repository: example/Upstream#repository: other/Upstream#' \
    .github/upstream-sync-policy.yml
  git add .
  git commit -qm 'mismatched policy identity'
  main="$(git rev-parse HEAD)"

  set +e
  output="$(prepare "${main}" "${checkpoint}" 2>/dev/null)"
  status=$?
  set -e
  assert_eq 3 "${status}" 'policy identity mismatch exit code'
  assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" \
    'policy identity mismatch state'
}

test_sensitive_paths_fail_closed() {
  local case_name repo checkpoint upstream_tip main output status path
  for case_name in sensitive; do
    repo="${RUN_ROOT}/${case_name}-policy"
    new_repo "${repo}"
    cd "${repo}"

    printf 'base\n' >app.txt
    git add app.txt
    git commit -qm 'checkpoint'
    checkpoint="$(git rev-parse HEAD)"

    git switch -qc upstream
    if [[ "${case_name}" == sensitive ]]; then
      path='crates/core/token.pem'
    fi
    mkdir -p "$(dirname "${path}")"
    printf 'unsafe backlog\n' >"${path}"
    git add .
    git commit -qm "${case_name} upstream path"
    upstream_tip="$(git rev-parse HEAD)"

    git switch -qc main "${checkpoint}"
    write_state "${checkpoint}"
    write_policy
    git add .github
    git commit -qm "${case_name} policy fixture"
    main="$(git rev-parse HEAD)"

    set +e
    output="$(prepare "${main}" "${upstream_tip}" 2>/dev/null)"
    status=$?
    set -e
    assert_eq 3 "${status}" "${case_name} policy exit code"
    assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" \
      "${case_name} policy state"
  done
}

test_rejected_aeris_glob_syntax() {
  local pattern repo checkpoint main output status
  for pattern in '!docs/**' '/docs/**' 'docs\\**' 'docs/[a-z].md' '' 'docs/'; do
    repo="${RUN_ROOT}/rejected-pattern-${RANDOM}"
    new_repo "${repo}"; cd "${repo}"
    printf 'base\n' >app.txt; git add app.txt; git commit -qm base; checkpoint="$(git rev-parse HEAD)"
    write_state "${checkpoint}"
    cat >.github/upstream-sync-policy.yml <<YAML
version: 1
upstream:
  repository: example/Upstream
  branch: main
sync:
  state_file: .github/upstream-sync-state.json
  fail_closed: true
  autonomous_merge: eligible
matching:
  syntax: aeris-glob-v1
  enforced_fork_owned_subset: exact_or_directory_recursive
  precedence:
    - sensitive
    - review_required
    - fork_owned
    - generated
    - upstream_owned
  default: review_required
fork_owned:
  - '$pattern'
review_required:
  - .github/**
sensitive:
  - .gitmodules
  - "**/*.pem"
  - "**/*.key"
  - "**/*.p12"
generated: []
upstream_owned:
  - apps/**
YAML
    git add .github; git commit -qm policy; main="$(git rev-parse HEAD)"
    set +e; output="$(prepare "${main}" "${checkpoint}" 2>/dev/null)"; status=$?; set -e
    assert_eq 3 "${status}" "rejected syntax ${pattern} exit code"
    assert_eq error "$(sed -n 's/^state=//p' <<<"${output}")" "rejected syntax ${pattern} state"
  done
}

test_unknown_paths_are_manual_review() {
  local repo="${RUN_ROOT}/unknown-policy" checkpoint upstream_tip main output
  new_repo "${repo}"
  cd "${repo}"
  printf 'base\n' >app.txt; git add app.txt; git commit -qm checkpoint
  checkpoint="$(git rev-parse HEAD)"
  git switch -qc upstream
  printf 'unknown\n' >unclassified.txt; git add .; git commit -qm unknown
  upstream_tip="$(git rev-parse HEAD)"
  git switch -qc main "${checkpoint}"; write_state "${checkpoint}"; write_policy
  sed -i 's#  - "\*\*"#  - apps/**#' .github/upstream-sync-policy.yml
  git add .github; git commit -qm policy; main="$(git rev-parse HEAD)"
  output="$(prepare "${main}" "${upstream_tip}")"
  assert_eq clean "$(sed -n 's/^state=//p' <<<"${output}")" 'unknown path state'
  assert_eq false "$(sed -n 's/^autonomous_eligible=//p' <<<"${output}")" 'unknown path eligibility'
  assert_eq manual_review "$(sed -n 's/^policy_verdict=//p' <<<"${output}")" 'unknown path verdict'
}

test_squash_checkpoint_noop
test_fork_owned_filter_and_state_advance
test_exact_path_and_recursive_directory_filter
test_non_fork_conflict
test_ai_resolution_policy_controls_conflict_bundle
test_invalid_state_and_history_rewrite
test_unsupported_policy_pattern
test_policy_identity_mismatch
test_sensitive_paths_fail_closed
test_unknown_paths_are_manual_review
test_rejected_aeris_glob_syntax

printf 'PASS prepare checkpoint sync (%s)\n' "${RUN_ROOT}"
