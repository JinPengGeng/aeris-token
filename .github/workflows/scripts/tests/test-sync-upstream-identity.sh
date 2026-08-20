#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-sync-identity.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

run_identity_case() {
  local name="$1" login="$2" comment_id="$3"
  local root="${RUN_ROOT}/${name}" fake_bin="${RUN_ROOT}/${name}/bin"
  local calls="${RUN_ROOT}/${name}/gh-calls" harness="${RUN_ROOT}/${name}/harness.sh"
  mkdir -p "${fake_bin}"
  : >"${calls}"

  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
case "$*" in
  *'/issues/42/comments?per_page=100'*)
    filter=''
    while (($#)); do
      if [[ "$1" == --jq ]]; then filter="$2"; break; fi
      shift
    done
    [[ -n "${filter}" ]] || { printf 'missing jq filter\n' >&2; exit 1; }
    [[ "${filter}" == *'aeris-sync[bot]'* && "${filter}" == *'github-actions[bot]'* ]] || {
      printf 'comment filter omitted an accepted bot identity\n' >&2
      exit 1
    }
    [[ "${filter}" == *"${COMMENT_LOGIN}"* ]] || {
      printf 'configured comment author was not accepted\n' >&2
      exit 1
    }
    if [[ "${filter}" == *'startswith('* ]]; then
      printf '%s\n' "${COMMENT_ID}"
    else
      printf '%s\n' '<!-- upstream-sync-once -->'
    fi
    ;;
  *'--method PATCH repos/example/repo/issues/comments/'*) ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${fake_bin}/gh"
  cp "${SCRIPT_ROOT}/github-autonomy.sh" "${root}/github-autonomy.sh"
  cp "${SCRIPT_ROOT}/bounded-git-fetch.sh" "${root}/bounded-git-fetch.sh"
  sed '/^parent="$(aeris_gh api "repos\/\${GITHUB_REPOSITORY}"/,$d' \
    "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  cat >>"${harness}" <<'EOF'
issue_comment_once 42 once 'must not duplicate'
set_pending_tip 42 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
EOF

  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" COMMENT_ID="${comment_id}" COMMENT_LOGIN="${login}" \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_SYNC_APP_SLUG=aeris-sync \
    bash "${harness}"

  assert_eq 0 "$(grep -Ec -- '--method POST|^pr comment ' "${calls}" || true)" \
    "${name} comments must not be duplicated"
  assert_eq 1 "$(grep -c -- "--method PATCH repos/example/repo/issues/comments/${comment_id}" "${calls}" || true)" \
    "${name} pending-tip comment must be updated"
  grep -q 'aeris-sync\[bot\]' "${calls}" || fail "${name} query omitted the Sync App bot"
  grep -q 'github-actions\[bot\]' "${calls}" || fail "${name} query omitted the legacy bot"
}

run_publication_fence_drift_case() {
  local root="${RUN_ROOT}/publication-fence" source="${RUN_ROOT}/publication-fence/source"
  local origin="${RUN_ROOT}/publication-fence/origin.git"
  local upstream="${RUN_ROOT}/publication-fence/upstream.git"
  local harness="${RUN_ROOT}/publication-fence/harness.sh"
  local hook="${RUN_ROOT}/publication-fence/drift.sh" stable drift
  mkdir -p "${root}"
  git init -q --bare "${origin}"
  git init -q --bare "${upstream}"
  git init -q "${source}"
  git -C "${source}" config user.name 'Publication Fence Fixture'
  git -C "${source}" config user.email 'publication-fence@example.com'
  printf 'stable\n' >"${source}/file.txt"
  git -C "${source}" add file.txt
  git -C "${source}" commit -qm stable
  stable="$(git -C "${source}" rev-parse HEAD)"
  printf 'drift\n' >"${source}/file.txt"
  git -C "${source}" commit -qam drift
  drift="$(git -C "${source}" rev-parse HEAD)"
  git -C "${source}" remote add origin "${origin}"
  git -C "${source}" remote add upstream "${upstream}"
  git -C "${source}" push -q origin \
    "${stable}:refs/heads/main" "${stable}:refs/heads/automation/sync-upstream"
  git -C "${source}" push -q upstream "${stable}:refs/heads/main"

  cp "${SCRIPT_ROOT}/github-autonomy.sh" "${root}/github-autonomy.sh"
  cp "${SCRIPT_ROOT}/bounded-git-fetch.sh" "${root}/bounded-git-fetch.sh"
  sed '/^parent="$(aeris_gh api "repos\/\${GITHUB_REPOSITORY}"/,$d' \
    "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  cat >>"${harness}" <<'EOF'
upstream_branch=main
if aeris_assert_publication_refs_exact \
  "${EXPECTED_BASE}" "${EXPECTED_UPSTREAM}" "${EXPECTED_HEAD}"; then
  printf 'publication fence accepted drift\n' >&2
  exit 99
fi
EOF
  cat >"${hook}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
git push -q --force upstream "${DRIFT_SHA}:refs/heads/main"
EOF
  chmod +x "${hook}"
  cd "${source}"
  GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_SYNC_APP_SLUG=aeris-sync \
    AERIS_SYNC_TEST_MODE=true AERIS_SYNC_TEST_FIXTURE=true \
    AERIS_SYNC_BEFORE_FINAL_REF_FENCE_HOOK="${hook}" DRIFT_SHA="${drift}" \
    EXPECTED_BASE="${stable}" EXPECTED_UPSTREAM="${stable}" EXPECTED_HEAD="${stable}" \
    bash "${harness}"
}

run_post_publish_pr_fence_cases() {
  local root="${RUN_ROOT}/post-publish" source="${RUN_ROOT}/post-publish/source"
  local origin="${RUN_ROOT}/post-publish/origin.git" upstream="${RUN_ROOT}/post-publish/upstream.git"
  local harness="${RUN_ROOT}/post-publish/harness.sh" fake_bin="${RUN_ROOT}/post-publish/bin"
  local pr_json="${RUN_ROOT}/post-publish/pr.json" metadata_hook head_hook stable drift body
  mkdir -p "${root}" "${fake_bin}"
  git init -q --bare "${origin}"
  git init -q --bare "${upstream}"
  git init -q "${source}"
  git -C "${source}" config user.name 'Post Publish Fixture'
  git -C "${source}" config user.email 'post-publish@example.com'
  printf 'stable\n' >"${source}/file.txt"
  git -C "${source}" add file.txt
  git -C "${source}" commit -qm stable
  stable="$(git -C "${source}" rev-parse HEAD)"
  printf 'drift\n' >"${source}/file.txt"
  git -C "${source}" commit -qam drift
  drift="$(git -C "${source}" rev-parse HEAD)"
  git -C "${source}" remote add origin "${origin}"
  git -C "${source}" remote add upstream "${upstream}"
  git -C "${source}" push -q origin \
    "${stable}:refs/heads/main" "${stable}:refs/heads/automation/sync-upstream"
  git -C "${source}" push -q upstream "${stable}:refs/heads/main"

  body="<!-- upstream-sync-managed -->
<!-- upstream-sync-owned-tip:${stable} -->
<!-- upstream-sync-source:example/Upstream@${stable} -->"
  PR_BODY="${body}" PR_SHA="${stable}" node - "${pr_json}" <<'NODE'
const fs = require('node:fs');
fs.writeFileSync(process.argv[2], JSON.stringify({
  number: 42, state: 'open', draft: false, body: process.env.PR_BODY,
  user: { login: 'aeris-sync[bot]', id: 987654, type: 'Bot' },
  base: { ref: 'main', sha: process.env.PR_SHA, repo: { full_name: 'example/repo' } },
  head: { ref: 'automation/sync-upstream', sha: process.env.PR_SHA,
    repo: { full_name: 'example/repo' } },
}));
NODE
  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == 'api repos/example/repo/pulls/42' ]] || exit 2
cat "${PR_JSON}"
EOF
  chmod +x "${fake_bin}/gh"
  metadata_hook="${root}/metadata-drift.sh"
  cat >"${metadata_hook}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
node - "${PR_JSON}" <<'NODE'
const fs = require('node:fs'); const path = process.argv[2];
const pr = JSON.parse(fs.readFileSync(path, 'utf8')); pr.draft = true;
fs.writeFileSync(path, JSON.stringify(pr));
NODE
EOF
  head_hook="${root}/head-drift.sh"
  cat >"${head_hook}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
git push -q --force origin "${DRIFT_SHA}:refs/heads/automation/sync-upstream"
EOF
  chmod +x "${metadata_hook}" "${head_hook}"

  cp "${SCRIPT_ROOT}/github-autonomy.sh" "${root}/github-autonomy.sh"
  cp "${SCRIPT_ROOT}/bounded-git-fetch.sh" "${root}/bounded-git-fetch.sh"
  sed '/^parent="$(aeris_gh api "repos\/\${GITHUB_REPOSITORY}"/,$d' \
    "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  cat >>"${harness}" <<'EOF'
upstream_branch=main
base_sha="${STABLE_SHA}"
upstream_sha="${STABLE_SHA}"
published_sha="${STABLE_SHA}"
published_pr_number=42
published_pr_url=https://github.com/example/repo/pull/42
published_pr_body="${EXPECTED_BODY}"
sync_app_bot_id=987654
sync_app_bot_type=Bot
if aeris_post_publish_fence; then
  printf 'post-publish fence accepted PR metadata drift\n' >&2
  exit 97
fi
EOF
  cd "${source}"
  PATH="${fake_bin}:${PATH}" PR_JSON="${pr_json}" GITHUB_OUTPUT="${root}/output" \
    GITHUB_REPOSITORY=example/repo AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z \
    AERIS_SYNC_APP_SLUG=aeris-sync AERIS_SYNC_TEST_MODE=true AERIS_SYNC_TEST_FIXTURE=true \
    AERIS_SYNC_AFTER_PUBLISH_REF_FENCE_HOOK="${metadata_hook}" \
    STABLE_SHA="${stable}" EXPECTED_BODY="${body}" bash "${harness}"

  PR_BODY="${body}" PR_SHA="${stable}" node - "${pr_json}" <<'NODE'
const fs = require('node:fs');
fs.writeFileSync(process.argv[2], JSON.stringify({
  number: 42, state: 'open', draft: false, body: process.env.PR_BODY,
  user: { login: 'aeris-sync[bot]', id: 987654, type: 'Bot' },
  base: { ref: 'main', sha: process.env.PR_SHA, repo: { full_name: 'example/repo' } },
  head: { ref: 'automation/sync-upstream', sha: process.env.PR_SHA,
    repo: { full_name: 'example/repo' } },
}));
NODE
  PATH="${fake_bin}:${PATH}" PR_JSON="${pr_json}" DRIFT_SHA="${drift}" \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_SYNC_APP_SLUG=aeris-sync \
    AERIS_SYNC_TEST_MODE=true AERIS_SYNC_TEST_FIXTURE=true \
    AERIS_SYNC_BEFORE_SUCCESS_REF_FENCE_HOOK="${head_hook}" \
    STABLE_SHA="${stable}" EXPECTED_BODY="${body}" bash "${harness}"
}

run_identity_case app 'aeris-sync[bot]' 102
run_identity_case legacy 'github-actions[bot]' 202
run_publication_fence_drift_case
run_post_publish_pr_fence_cases
printf 'PASS sync upstream identity migration (%s)\n' "${RUN_ROOT}"
