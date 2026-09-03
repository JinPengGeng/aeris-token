#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERIFIER="${SCRIPT_ROOT}/verify-sync-candidate.sh"
PREPARE="${SCRIPT_ROOT}/prepare-checkpoint-sync.sh"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-verify-sync-test.XXXXXX")"
REPO="${RUN_ROOT}/repo"
ORIGIN="${RUN_ROOT}/origin.git"
UPSTREAM="${RUN_ROOT}/upstream.git"
FAKE_BIN="${RUN_ROOT}/bin"
HELPER_ROOT="${RUN_ROOT}/helpers"
PR_JSON="${RUN_ROOT}/pr.json"
OUTPUT="${RUN_ROOT}/output"
BOT_EMAIL='41898282+github-actions[bot]@users.noreply.github.com'
SYNC_APP_BOT_ID=987654

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

expect_rejected() {
  local label="$1" expected_head="$2" status
  set +e
  run_verifier "${expected_head}" >/dev/null 2>&1
  status=$?
  set -e
  [[ ${status} -ne 0 ]] || fail "${label} was accepted"
  ! grep -qx 'verified=true' "${OUTPUT}" || fail "${label} emitted verified=true"
}

write_state() {
  local path="$1" sha="$2" version="${3:-1}"
  mkdir -p "$(dirname "${path}")"
  printf '{"schema_version":1,"repository":"example/Upstream","branch":"main","last_integrated_sha":"%s","policy_version":%s}\n' \
    "${sha}" "${version}" >"${path}"
}

write_policy() {
  mkdir -p .github
  cat >.github/upstream-sync-policy.yml <<'YAML'
version: 1
upstream:
  repository: example/Upstream
  branch: main
sync:
  base_branch: main
  branch: automation/sync-upstream
  state_file: .github/upstream-sync-state.json
  fail_closed: true
  autonomous_merge: eligible
resource_bounds:
  fetch_timeout_seconds: 90
  max_received_bytes: 268435456
  max_received_expanded_bytes: 1073741824
  max_received_objects: 250000
  max_import_bytes: 268435456
  max_import_objects: 250000
  max_process_memory_bytes: 536870912
  max_object_bytes: 33554432
  max_blob_bytes: 33554432
  max_changed_blob_bytes: 268435456
  max_changed_paths: 20000
  max_tree_entries: 500000
  max_diff_bytes: 33554432
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
    profile: aeris-sync-conflict-v2
    required_pre_conflict_verdict: eligible
    allowed_type: modify_modify_utf8_text
    allowed_mode: "100644"
    maximum_files: 16
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

tree_with_file() {
  local tree="$1" path="$2" content="$3" index blob
  index="$(mktemp "${RUN_ROOT}/index.XXXXXX")"
  rm -f "${index}"
  GIT_INDEX_FILE="${index}" git read-tree "${tree}"
  blob="$(git hash-object -w "${content}")"
  GIT_INDEX_FILE="${index}" git update-index --add --cacheinfo 100644 "${blob}" "${path}"
  GIT_INDEX_FILE="${index}" git write-tree
  rm -f "${index}" "${index}.lock"
}

make_candidate() {
  local tree="$1" base="$2" checkpoint="$3" upstream_tip="$4" duplicate="${5:-false}"
  local second_parent="${6:-${upstream_tip}}"
  local message="${RUN_ROOT}/message" duplicate_line=''
  local -a parent_args=(-p "${base}")
  [[ "${second_parent}" == none ]] || parent_args+=(-p "${second_parent}")
  [[ "${duplicate}" != true ]] || duplicate_line=$'\nSync-Upstream-Base: '"${base}"
  cat >"${message}" <<EOF
chore: sync example/Upstream@${upstream_tip}

Sync-Upstream-Automation: true

Sync-Upstream-Source: example/Upstream@${upstream_tip}

Sync-Upstream-Checkpoint: ${checkpoint}->${upstream_tip}

Sync-Upstream-Base: ${base}${duplicate_line}
EOF
  GIT_AUTHOR_NAME='github-actions[bot]' GIT_AUTHOR_EMAIL="${BOT_EMAIL}" \
    GIT_COMMITTER_NAME='github-actions[bot]' GIT_COMMITTER_EMAIL="${BOT_EMAIL}" \
    git commit-tree "${tree}" "${parent_args[@]}" -F "${message}"
}

write_pr() {
  local base="$1" head="$2" author="${3:-aeris-sync[bot]}"
  local base_ref="${4:-main}" head_ref="${5:-automation/sync-upstream}" managed="${6:-true}"
  PR_BASE="${base}" PR_HEAD="${head}" PR_AUTHOR="${author}" PR_BASE_REF="${base_ref}" \
    PR_HEAD_REF="${head_ref}" PR_MANAGED="${managed}" SYNC_APP_BOT_ID="${SYNC_APP_BOT_ID}" \
    node - "${PR_JSON}" <<'NODE'
const fs = require('node:fs');
const marker = process.env.PR_MANAGED === 'true' ? '<!-- upstream-sync-managed -->\n' : '';
const body = `${marker}<!-- upstream-sync-owned-tip:${process.env.PR_HEAD} -->
<!-- upstream-sync-source:example/Upstream@${process.env.U1} -->
Automated synchronization fixture.`;
const userId = process.env.PR_AUTHOR === 'aeris-sync[bot]' ? Number(process.env.SYNC_APP_BOT_ID) : 41898282;
fs.writeFileSync(process.argv[2], JSON.stringify({
  number: 36,
  state: 'open',
  draft: false,
  user: { login: process.env.PR_AUTHOR, id: userId, type: 'Bot' },
  base: { ref: process.env.PR_BASE_REF, sha: process.env.PR_BASE, repo: { full_name: 'example/Fork' } },
  head: { ref: process.env.PR_HEAD_REF, sha: process.env.PR_HEAD, repo: { full_name: 'example/Fork' } },
  body,
}));
NODE
}

publish_refs() {
  local base="$1" head="$2"
  git push -q --force origin "${base}:refs/heads/main" "${head}:refs/heads/automation/sync-upstream"
}

run_verifier() {
  local expected_head="$1"
  : >"${OUTPUT}"
  PATH="${FAKE_BIN}:${PATH}" PR_JSON="${PR_JSON}" SYNC_APP_BOT_ID="${SYNC_APP_BOT_ID}" \
    AERIS_AUTONOMY_EXPIRES_AT='2099-01-01T00:00:00Z' AERIS_WRITER_APP_SLUG='aeris-sync' \
    AERIS_TMP_ROOT="${RUN_ROOT}/tmp" GITHUB_OUTPUT="${OUTPUT}" \
    AUTONOMY_HELPER="${HELPER_ROOT}/github-autonomy.sh" \
    PREPARE_HELPER="${HELPER_ROOT}/prepare-checkpoint-sync.sh" \
    CHECKPOINT_HELPER="${HELPER_ROOT}/checkpoint-merge.sh" \
    AERIS_BOUNDED_FETCH_TEST_MODE=true AERIS_BOUNDED_FETCH_TEST_FIXTURE=true \
    AERIS_BOUNDED_CREDENTIALLESS_TEST_ALLOW_CONFIG=true \
    AERIS_TEST_FETCH_TIMEOUT_SECONDS=300 \
    AERIS_VERIFY_TEST_MODE="${AERIS_VERIFY_TEST_MODE:-false}" \
    AERIS_VERIFY_TEST_FIXTURE="${AERIS_VERIFY_TEST_FIXTURE:-false}" \
    AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK="${AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK:-}" \
    AERIS_VERIFY_AFTER_REF_FENCE_HOOK="${AERIS_VERIFY_AFTER_REF_FENCE_HOOK:-}" \
    AERIS_VERIFY_BEFORE_SUCCESS_REF_FENCE_HOOK="${AERIS_VERIFY_BEFORE_SUCCESS_REF_FENCE_HOOK:-}" \
    AERIS_TEST_UPSTREAM="${UPSTREAM}" AERIS_TEST_DRIFT_SHA="${AERIS_TEST_DRIFT_SHA:-}" \
    AERIS_TEST_ORIGIN="${ORIGIN}" AERIS_TEST_PR_JSON="${PR_JSON}" \
    GIT_ALLOW_PROTOCOL='file' GIT_CONFIG_COUNT=2 \
    GIT_CONFIG_KEY_0="url.file://${UPSTREAM}.insteadOf" \
    GIT_CONFIG_VALUE_0='https://github.com/example/Upstream.git' \
    GIT_CONFIG_KEY_1="url.file://${ORIGIN}.insteadOf" \
    GIT_CONFIG_VALUE_1='https://github.com/example/Fork.git' \
    bash "${VERIFIER}" example/Fork 36 "${expected_head}"
}

mkdir -p "${FAKE_BIN}" "${HELPER_ROOT}" "${RUN_ROOT}/tmp"
cat >"${FAKE_BIN}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
destination=''
url=''
while (($#)); do
  case "$1" in
    --output) destination="$2"; shift 2 ;;
    --write-out) shift 2 ;;
    https://*) url="$1"; shift ;;
    *) shift ;;
  esac
done
[[ -n "${destination}" && -n "${url}" ]] || exit 2
case "${url}" in
  https://api.github.com/repos/example/Fork/pulls/36)
    cp "${PR_JSON}" "${destination}"
    printf '%s' "${AERIS_TEST_PR_HTTP_STATUS:-200}"
    exit 0
    ;;
  https://api.github.com/users/aeris-sync%5Bbot%5D)
    if [[ -n "${AERIS_TEST_BOT_BODY_FILE:-}" ]]; then
      cp "${AERIS_TEST_BOT_BODY_FILE}" "${destination}"
    elif [[ -n "${AERIS_TEST_BOT_BODY:-}" ]]; then
      printf '%s' "${AERIS_TEST_BOT_BODY}" >"${destination}"
    else
      printf '{"login":"aeris-sync[bot]","id":%s,"type":"Bot"}\n' \
        "${SYNC_APP_BOT_ID}" >"${destination}"
    fi
    printf '%s' "${AERIS_TEST_BOT_HTTP_STATUS:-200}"
    exit 0
    ;;
  *) exit 2 ;;
esac
EOF
chmod +x "${FAKE_BIN}/curl"
for helper in github-autonomy.sh bounded-git-fetch.sh prepare-checkpoint-sync.sh checkpoint-merge.sh; do
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/${helper}" >"${HELPER_ROOT}/${helper}"
  chmod +x "${HELPER_ROOT}/${helper}"
done

git init -q --bare "${ORIGIN}"
git init -q --bare "${UPSTREAM}"
git init -q "${REPO}"
cd "${REPO}"
git config user.name 'Verifier Fixture'
git config user.email 'fixture@example.com'
git config core.autocrlf false
git remote add origin "${ORIGIN}"

mkdir -p .github/workflows
printf 'app v0\n' >app.txt
printf 'upstream workflow v0\n' >.github/workflows/release.yml
git add .
git commit -qm 'upstream checkpoint'
U0="$(git rev-parse HEAD)"

git switch -qc main
printf 'fork workflow\n' >.github/workflows/release.yml
write_state .github/upstream-sync-state.json "${U0}"
write_policy
git add .
git commit -qm 'fork base policy'
BASE="$(git rev-parse HEAD)"

git switch -qc upstream "${U0}"
printf 'app v1\n' >app.txt
printf 'upstream workflow v1\n' >.github/workflows/release.yml
printf 'new upstream content\n' >upstream.txt
git add .
git commit -qm 'upstream one'
U1="$(git rev-parse HEAD)"
export U1
git push -q "${UPSTREAM}" "${U1}:refs/heads/main"

prepare_output="$(AERIS_TMP_ROOT="${RUN_ROOT}/tmp" \
  bash "${HELPER_ROOT}/prepare-checkpoint-sync.sh" \
  "${BASE}" "${U1}" example/Upstream main)"
EXPECTED_TREE="$(sed -n 's/^tree=//p' <<<"${prepare_output}")"
VALID="$(make_candidate "${EXPECTED_TREE}" "${BASE}" "${U0}" "${U1}")"
publish_refs "${BASE}" "${VALID}"
write_pr "${BASE}" "${VALID}"

valid_output="$(run_verifier "${VALID}")"
[[ "${valid_output}" == *"verified sync candidate PR #36"* ]] || fail 'valid PR #36-like case was not verified'
grep -qx 'verified=true' "${OUTPUT}" || fail 'valid verification output is missing'
run_verifier "${VALID}" >/dev/null || fail 'deterministic replay of the same candidate failed'

cp "${PR_JSON}" "${RUN_ROOT}/valid-pr.json"
for invalid_pr in '{' '[]' 'null'; do
  printf '%s' "${invalid_pr}" >"${PR_JSON}"
  expect_rejected 'malformed or non-object pull request JSON' "${VALID}"
done
cp "${RUN_ROOT}/valid-pr.json" "${PR_JSON}"
export AERIS_TEST_PR_HTTP_STATUS=503
expect_rejected 'pull request HTTP error body' "${VALID}"
unset AERIS_TEST_PR_HTTP_STATUS
node -e "process.stdout.write('x'.repeat(2097153))" >"${PR_JSON}"
expect_rejected 'oversized pull request JSON' "${VALID}"
cp "${RUN_ROOT}/valid-pr.json" "${PR_JSON}"
for invalid_bot in '{' '[]' 'null'; do
  export AERIS_TEST_BOT_BODY="${invalid_bot}"
  expect_rejected 'malformed or non-object Sync App identity JSON' "${VALID}"
done
unset AERIS_TEST_BOT_BODY
node -e "process.stdout.write('x'.repeat(2097153))" >"${RUN_ROOT}/oversized-bot.json"
export AERIS_TEST_BOT_BODY_FILE="${RUN_ROOT}/oversized-bot.json"
expect_rejected 'oversized Sync App identity JSON' "${VALID}"
unset AERIS_TEST_BOT_BODY_FILE
export AERIS_TEST_BOT_HTTP_STATUS=502
expect_rejected 'Sync App identity HTTP error body' "${VALID}"
unset AERIS_TEST_BOT_HTTP_STATUS

PR_METADATA_HOOK="${RUN_ROOT}/drift-pr-after-ref-fence.sh"
cat >"${PR_METADATA_HOOK}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
node - "${AERIS_TEST_PR_JSON}" <<'NODE'
const fs = require('node:fs');
const path = process.argv[2];
const pr = JSON.parse(fs.readFileSync(path, 'utf8'));
pr.draft = true;
pr.body += '\nmetadata drift';
fs.writeFileSync(path, JSON.stringify(pr));
NODE
EOF
chmod +x "${PR_METADATA_HOOK}"
AERIS_VERIFY_TEST_MODE=true
AERIS_VERIFY_TEST_FIXTURE=true
AERIS_VERIFY_AFTER_REF_FENCE_HOOK="${PR_METADATA_HOOK}"
expect_rejected 'PR metadata drift after verifier ref fence' "${VALID}"
unset AERIS_VERIFY_AFTER_REF_FENCE_HOOK
write_pr "${BASE}" "${VALID}"

HEAD_DRIFT="$(git commit-tree "${EXPECTED_TREE}" -p "${BASE}" -m 'late head drift')"
HEAD_DRIFT_HOOK="${RUN_ROOT}/drift-head-before-success.sh"
cat >"${HEAD_DRIFT_HOOK}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
git push -q --force "${AERIS_TEST_ORIGIN}" \
  "${AERIS_TEST_DRIFT_SHA}:refs/heads/automation/sync-upstream"
EOF
chmod +x "${HEAD_DRIFT_HOOK}"
AERIS_VERIFY_BEFORE_SUCCESS_REF_FENCE_HOOK="${HEAD_DRIFT_HOOK}"
AERIS_TEST_DRIFT_SHA="${HEAD_DRIFT}"
expect_rejected 'head force-push after authoritative PR verification' "${VALID}"
unset AERIS_VERIFY_TEST_MODE AERIS_VERIFY_TEST_FIXTURE \
  AERIS_VERIFY_BEFORE_SUCCESS_REF_FENCE_HOOK AERIS_TEST_DRIFT_SHA
publish_refs "${BASE}" "${VALID}"
write_pr "${BASE}" "${VALID}"

git switch -q upstream
printf 'upstream drift after validation\n' >late-drift.txt
git add late-drift.txt
git commit -qm 'late upstream drift'
LATE_UPSTREAM="$(git rev-parse HEAD)"
git switch -q --detach "${VALID}"
FINAL_FENCE_HOOK="${RUN_ROOT}/drift-before-final-fence.sh"
cat >"${FINAL_FENCE_HOOK}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
git push -q --force "${AERIS_TEST_UPSTREAM}" \
  "${AERIS_TEST_DRIFT_SHA}:refs/heads/main"
EOF
chmod +x "${FINAL_FENCE_HOOK}"
AERIS_VERIFY_TEST_MODE=true
AERIS_VERIFY_TEST_FIXTURE=true
AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK="${FINAL_FENCE_HOOK}"
AERIS_TEST_DRIFT_SHA="${LATE_UPSTREAM}"
expect_rejected 'upstream drift immediately before verifier success' "${VALID}"
unset AERIS_VERIFY_TEST_MODE AERIS_VERIFY_TEST_FIXTURE \
  AERIS_VERIFY_BEFORE_FINAL_FENCE_HOOK AERIS_TEST_DRIFT_SHA
git push -q --force "${UPSTREAM}" "${U1}:refs/heads/main"

write_pr "${BASE}" "${VALID}" github-actions[bot]
expect_rejected 'legacy GitHub Actions bot author' "${VALID}"
write_pr "${BASE}" "${VALID}" app/github-actions
expect_rejected 'legacy GitHub Actions app author' "${VALID}"
write_pr "${BASE}" "${VALID}"
if [[ "${AERIS_TEST_IDENTITY_ONLY:-false}" == true ]]; then
  printf 'PASS verify exact Sync App identity fixtures (%s)\n' "${RUN_ROOT}"
  exit 0
fi
if [[ "${AERIS_TEST_VALID_ONLY:-false}" == true ]]; then
  printf 'PASS verify sync candidate valid fixture (%s)\n' "${RUN_ROOT}"
  exit 0
fi

write_state "${RUN_ROOT}/state-u1.json" "${U1}"
STATE_ONLY_TREE="$(tree_with_file "${BASE}^{tree}" .github/upstream-sync-state.json "${RUN_ROOT}/state-u1.json")"
STATE_ONLY="$(make_candidate "${STATE_ONLY_TREE}" "${BASE}" "${U0}" "${U1}")"
publish_refs "${BASE}" "${STATE_ONLY}"
write_pr "${BASE}" "${STATE_ONLY}"
expect_rejected 'tampered state-only candidate' "${STATE_ONLY}"

git show "${BASE}:.github/upstream-sync-state.json" >"${RUN_ROOT}/state-u0.json"
CONTENT_ONLY_TREE="$(tree_with_file "${EXPECTED_TREE}" .github/upstream-sync-state.json "${RUN_ROOT}/state-u0.json")"
CONTENT_ONLY="$(make_candidate "${CONTENT_ONLY_TREE}" "${BASE}" "${U0}" "${U1}")"
publish_refs "${BASE}" "${CONTENT_ONLY}"
write_pr "${BASE}" "${CONTENT_ONLY}"
expect_rejected 'tampered content-only candidate' "${CONTENT_ONLY}"

printf 'tampered workflow\n' >"${RUN_ROOT}/workflow"
EXCLUDED_TREE="$(tree_with_file "${EXPECTED_TREE}" .github/workflows/release.yml "${RUN_ROOT}/workflow")"
EXCLUDED="$(make_candidate "${EXCLUDED_TREE}" "${BASE}" "${U0}" "${U1}")"
publish_refs "${BASE}" "${EXCLUDED}"
write_pr "${BASE}" "${EXCLUDED}"
expect_rejected 'excluded fork-owned path drift' "${EXCLUDED}"

printf '%s\n' 'version: 2' >"${RUN_ROOT}/policy"
POLICY_TREE="$(tree_with_file "${EXPECTED_TREE}" .github/upstream-sync-policy.yml "${RUN_ROOT}/policy")"
POLICY_DRIFT="$(make_candidate "${POLICY_TREE}" "${BASE}" "${U0}" "${U1}")"
publish_refs "${BASE}" "${POLICY_DRIFT}"
write_pr "${BASE}" "${POLICY_DRIFT}"
expect_rejected 'candidate policy drift' "${POLICY_DRIFT}"

write_state "${RUN_ROOT}/state-v2.json" "${U1}" 2
VERSION_TREE="$(tree_with_file "${EXPECTED_TREE}" .github/upstream-sync-state.json "${RUN_ROOT}/state-v2.json")"
VERSION_DRIFT="$(make_candidate "${VERSION_TREE}" "${BASE}" "${U0}" "${U1}")"
publish_refs "${BASE}" "${VERSION_DRIFT}"
write_pr "${BASE}" "${VERSION_DRIFT}"
expect_rejected 'state policy version drift' "${VERSION_DRIFT}"

DUPLICATE="$(make_candidate "${EXPECTED_TREE}" "${BASE}" "${U0}" "${U1}" true)"
publish_refs "${BASE}" "${DUPLICATE}"
write_pr "${BASE}" "${DUPLICATE}"
expect_rejected 'duplicate commit trailer' "${DUPLICATE}"

SINGLE_PARENT="$(make_candidate "${EXPECTED_TREE}" "${BASE}" "${U0}" "${U1}" false none)"
publish_refs "${BASE}" "${SINGLE_PARENT}"
write_pr "${BASE}" "${SINGLE_PARENT}"
expect_rejected 'single-parent candidate without the upstream link' "${SINGLE_PARENT}"

WRONG_SECOND_PARENT="$(make_candidate "${EXPECTED_TREE}" "${BASE}" "${U0}" "${U1}" false "${U0}")"
publish_refs "${BASE}" "${WRONG_SECOND_PARENT}"
write_pr "${BASE}" "${WRONG_SECOND_PARENT}"
expect_rejected 'second parent that is not the advertised upstream tip' "${WRONG_SECOND_PARENT}"

publish_refs "${BASE}" "${VALID}"
write_pr "${BASE}" "${VALID}" aeris-sync[bot] main automation/sync-upstream false
expect_rejected 'non-managed PR' "${VALID}"
write_pr "${BASE}" "${VALID}" attacker
expect_rejected 'untrusted PR author' "${VALID}"
write_pr "${BASE}" "${VALID}" github-actions[bot] main attacker-branch
expect_rejected 'untrusted PR branch' "${VALID}"

write_pr "${BASE}" "${VALID}"
expect_rejected 'stale expected head' "${STATE_ONLY}"
git push -q --force origin "${STATE_ONLY}:refs/heads/automation/sync-upstream"
expect_rejected 'remote head drift' "${VALID}"

MERGED_BASE="$(GIT_AUTHOR_NAME='Verifier Fixture' GIT_AUTHOR_EMAIL='fixture@example.com' \
  GIT_COMMITTER_NAME='Verifier Fixture' GIT_COMMITTER_EMAIL='fixture@example.com' \
  git commit-tree "${EXPECTED_TREE}" -p "${BASE}" -m 'merge candidate')"
git push -q --force origin "${MERGED_BASE}:refs/heads/main" "${VALID}:refs/heads/automation/sync-upstream"
write_pr "${MERGED_BASE}" "${VALID}"
expect_rejected 'replay after checkpoint advancement' "${VALID}"

git switch -q --orphan rewritten-upstream
git rm -qrf --ignore-unmatch .
printf 'rewritten\n' >app.txt
git add app.txt
git commit -qm 'rewritten upstream'
REWRITTEN="$(git rev-parse HEAD)"
write_state "${RUN_ROOT}/state-rewritten.json" "${REWRITTEN}"
REWRITTEN_TREE="$(tree_with_file "${BASE}^{tree}" .github/upstream-sync-state.json "${RUN_ROOT}/state-rewritten.json")"
REWRITTEN_HEAD="$(make_candidate "${REWRITTEN_TREE}" "${BASE}" "${U0}" "${REWRITTEN}")"
git push -q --force "${UPSTREAM}" "${REWRITTEN}:refs/heads/main"
publish_refs "${BASE}" "${REWRITTEN_HEAD}"
export U1="${REWRITTEN}"
write_pr "${BASE}" "${REWRITTEN_HEAD}"
expect_rejected 'non-ancestor checkpoint' "${REWRITTEN_HEAD}"

git switch -q --detach "${U0}"
printf 'fork conflict\n' >app.txt
write_state .github/upstream-sync-state.json "${U0}"
write_policy
git add .
git commit -qm 'conflicting fork base'
CONFLICT_BASE="$(git rev-parse HEAD)"
git switch -q --detach "${U0}"
printf 'upstream conflict\n' >app.txt
git add app.txt
git commit -qm 'conflicting upstream'
CONFLICT_UPSTREAM="$(git rev-parse HEAD)"
write_state "${RUN_ROOT}/state-conflict.json" "${CONFLICT_UPSTREAM}"
CONFLICT_TREE="$(tree_with_file "${CONFLICT_BASE}^{tree}" .github/upstream-sync-state.json "${RUN_ROOT}/state-conflict.json")"
CONFLICT_HEAD="$(make_candidate "${CONFLICT_TREE}" "${CONFLICT_BASE}" "${U0}" "${CONFLICT_UPSTREAM}")"
git push -q --force "${UPSTREAM}" "${CONFLICT_UPSTREAM}:refs/heads/main"
publish_refs "${CONFLICT_BASE}" "${CONFLICT_HEAD}"
export U1="${CONFLICT_UPSTREAM}"
write_pr "${CONFLICT_BASE}" "${CONFLICT_HEAD}"
expect_rejected 'conflicting deterministic integration' "${CONFLICT_HEAD}"

printf 'PASS verify sync candidate (%s)\n' "${RUN_ROOT}"
