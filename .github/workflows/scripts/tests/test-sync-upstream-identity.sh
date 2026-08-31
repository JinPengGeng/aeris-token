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
  local name="$1" login="$2" comment_id="$3" expected_posts="${4:-0}" exercise_pr_comment="${5:-false}"
  local root="${RUN_ROOT}/${name}" fake_bin="${RUN_ROOT}/${name}/bin"
  local calls="${RUN_ROOT}/${name}/gh-calls" harness="${RUN_ROOT}/${name}/harness.sh"
  mkdir -p "${fake_bin}"
  : >"${calls}"

  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
case "$*" in
  *'api --method GET repos/example/repo/issues/42/comments'*'-f page=1'*)
    case "${AERIS_TEST_CHANNEL:-}" in
      issue)
        [[ "${GH_TOKEN}" == test-issues-token ]] || {
          printf 'ordinary issue comment lookup used the wrong token channel\n' >&2
          exit 1
        }
        ;;
      writer)
        [[ "${GH_TOKEN}" == test-writer-token ]] || {
          printf 'PR comment lookup used the wrong token channel\n' >&2
          exit 1
        }
        ;;
      *)
        printf 'comment lookup did not declare its channel\n' >&2
        exit 1
        ;;
    esac
    printf '[{"id":1,"user":{"login":"%s"},"body":"<!-- upstream-sync-once -->"}' \
      "${COMMENT_LOGIN}"
    if [[ -n "${COMMENT_ID}" ]]; then
      printf ',{"id":%s,"user":{"login":"%s"},"body":"<!-- upstream-sync-pending-tip:old -->"}' \
        "${COMMENT_ID}" "${COMMENT_LOGIN}"
    fi
    printf ']\n'
    ;;
  *'--method PATCH repos/example/repo/issues/comments/'*)
    [[ "${GH_TOKEN}" == test-writer-token ]] || {
      printf 'pending-tip update used the wrong token channel\n' >&2
      exit 1
    }
    ;;
  *'--method POST repos/example/repo/issues/42/comments'*)
    [[ "${GH_TOKEN}" == test-writer-token ]] || {
      printf 'pending-tip creation used the wrong token channel\n' >&2
      exit 1
    }
    if [[ "$*" == *'upstream-sync-pending-tip:'* ]]; then
      [[ "${EXPECT_PENDING_POST}" == true ]] || {
        printf 'unexpected pending-tip REST comment creation\n' >&2
        exit 1
      }
    else
      [[ "${EXPECT_PR_POST}" == true ]] || {
        printf 'unexpected PR REST comment creation\n' >&2
        exit 1
      }
    fi
    ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${fake_bin}/gh"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/github-autonomy.sh" >"${root}/github-autonomy.sh"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/bounded-git-fetch.sh" >"${root}/bounded-git-fetch.sh"
  sed '/^mapfile -t sync_identity /,$d' \
    "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
cat >>"${harness}" <<'EOF'
AERIS_TEST_CHANNEL=issue issue_comment_once 42 once 'must not duplicate'
AERIS_TEST_CHANNEL=writer set_pending_tip 42 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
if [[ "${EXERCISE_PR_COMMENT:-false}" == true ]]; then
  AERIS_TEST_CHANNEL=writer pr_comment_once 42 pr-once 'writer PR comment'
fi
EOF

  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" COMMENT_ID="${comment_id}" COMMENT_LOGIN="${login}" \
    EXPECT_PENDING_POST="$([[ "${expected_posts}" -ge 1 ]] && printf true || printf false)" \
    EXPECT_PR_POST="${exercise_pr_comment}" EXERCISE_PR_COMMENT="${exercise_pr_comment}" GH_TOKEN=test-writer-token \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_ISSUES_GH_TOKEN=test-issues-token AERIS_WRITER_APP_SLUG=aeris-writer \
    bash "${harness}"

  assert_eq "${expected_posts}" "$(grep -c -- '--method POST repos/example/repo/issues/42/comments' "${calls}" || true)" \
    "${name} comment writes must use REST"
  assert_eq 0 "$(grep -c -- '^pr comment ' "${calls}" || true)" \
    "${name} PR comments must not use GraphQL CLI"
  if [[ -n "${comment_id}" ]]; then
    assert_eq 1 "$(grep -c -- "--method PATCH repos/example/repo/issues/comments/${comment_id}" "${calls}" || true)" \
      "${name} pending-tip comment must be updated"
  fi
  grep -q 'api --method GET repos/example/repo/issues/42/comments' "${calls}" ||
    fail "${name} omitted the bounded comments read"
  ! grep -q -- '--paginate' "${calls}" || fail "${name} used unbounded pagination"
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

  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/github-autonomy.sh" >"${root}/github-autonomy.sh"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/bounded-git-fetch.sh" >"${root}/bounded-git-fetch.sh"
  sed '/^mapfile -t sync_identity /,$d' \
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
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
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

  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/github-autonomy.sh" >"${root}/github-autonomy.sh"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/bounded-git-fetch.sh" >"${root}/bounded-git-fetch.sh"
  sed '/^mapfile -t sync_identity /,$d' \
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
    AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token AERIS_SYNC_TEST_MODE=true AERIS_SYNC_TEST_FIXTURE=true \
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
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
    AERIS_SYNC_TEST_MODE=true AERIS_SYNC_TEST_FIXTURE=true \
    AERIS_SYNC_BEFORE_SUCCESS_REF_FENCE_HOOK="${head_hook}" \
    STABLE_SHA="${stable}" EXPECTED_BODY="${body}" bash "${harness}"
}

run_authoritative_identity_json_cases() {
  local root="${RUN_ROOT}/authoritative-json" fake_bin="${RUN_ROOT}/authoritative-json/bin"
  local harness="${RUN_ROOT}/authoritative-json/harness.sh"
  local repository_json="${RUN_ROOT}/authoritative-json/repository.json"
  local upstream_json="${RUN_ROOT}/authoritative-json/upstream.json"
  local bot_json="${RUN_ROOT}/authoritative-json/bot.json" status
  mkdir -p "${root}" "${fake_bin}"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/github-autonomy.sh" >"${root}/github-autonomy.sh"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/bounded-git-fetch.sh" >"${root}/bounded-git-fetch.sh"
  sed '/^mapfile -t sync_identity /,$d' "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  cat >>"${harness}" <<'EOF'
read_sync_identity >/dev/null
EOF
  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]] || exit 2
case "$2" in
  repos/example/repo) cat "${REPOSITORY_JSON}" ;;
  repos/example/Upstream) cat "${UPSTREAM_JSON}" ;;
  users/aeris-sync%5Bbot%5D) cat "${BOT_JSON}" ;;
  *) exit 2 ;;
esac
EOF
  chmod +x "${fake_bin}/gh"
  printf '%s\n' '{"parent":{"full_name":"example/Upstream","default_branch":"main"}}' >"${repository_json}"
  printf '%s\n' '{"full_name":"example/Upstream","default_branch":"main"}' >"${upstream_json}"
  printf '%s\n' '{"login":"aeris-sync[bot]","id":987654,"type":"Bot"}' >"${bot_json}"

  for invalid in '{' '[]' 'null'; do
    printf '%s' "${invalid}" >"${repository_json}"
    set +e
    PATH="${fake_bin}:${PATH}" REPOSITORY_JSON="${repository_json}" UPSTREAM_JSON="${upstream_json}" BOT_JSON="${bot_json}" \
      GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
      AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
      bash "${harness}" >/dev/null 2>&1
    status=$?
    set -e
    [[ ${status} -ne 0 ]] || fail 'malformed or non-object fork repository identity was accepted'
  done
  printf '%s\n' '{"parent":{"full_name":"example/Upstream","default_branch":"main"}}' >"${repository_json}"
  for invalid in '{' '[]' 'null'; do
    printf '%s' "${invalid}" >"${bot_json}"
    set +e
    PATH="${fake_bin}:${PATH}" REPOSITORY_JSON="${repository_json}" UPSTREAM_JSON="${upstream_json}" BOT_JSON="${bot_json}" \
      GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
      AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
      bash "${harness}" >/dev/null 2>&1
    status=$?
    set -e
    [[ ${status} -ne 0 ]] || fail 'malformed or non-object Sync App identity was accepted'
  done
  node -e "process.stdout.write('x'.repeat(2097153))" >"${bot_json}"
  set +e
  PATH="${fake_bin}:${PATH}" REPOSITORY_JSON="${repository_json}" UPSTREAM_JSON="${upstream_json}" BOT_JSON="${bot_json}" \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
    bash "${harness}" >/dev/null 2>&1
  status=$?
  set -e
  [[ ${status} -ne 0 ]] || fail 'oversized Sync App identity was accepted'
}

run_bounded_api_pagination_cases() {
  local root="${RUN_ROOT}/bounded-pagination" fake_bin="${RUN_ROOT}/bounded-pagination/bin"
  local harness="${RUN_ROOT}/bounded-pagination/harness.sh"
  local calls="${RUN_ROOT}/bounded-pagination/gh-calls" output_json status
  mkdir -p "${root}" "${fake_bin}"
  : >"${calls}"
  output_json="${root}/combined.json"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/github-autonomy.sh" >"${root}/github-autonomy.sh"
  awk '{ sub(/\r$/, ""); print }' "${SCRIPT_ROOT}/bounded-git-fetch.sh" >"${root}/bounded-git-fetch.sh"
  sed '/^mapfile -t sync_identity /,$d' "${SCRIPT_ROOT}/sync-upstream.sh" >"${harness}"
  cat >>"${harness}" <<'EOF'
aeris_read_bounded_api_array_pages aeris_bounded_gh \
  repos/example/repo/items "${OUTPUT_JSON}" 'pagination fixture'
EOF
  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
page=''
while (($#)); do
  if [[ "$1" == -f && "${2:-}" == page=* ]]; then
    page="${2#page=}"
    break
  fi
  shift
done
[[ "${page}" =~ ^[1-9][0-9]*$ ]] || exit 2
if [[ "${GH_MODE}" == overflow ]]; then
  if ((page <= 10)); then
    jq -nc --argjson page "${page}" '[range(1;101) | {id: (($page - 1) * 100 + .)}]'
  elif ((page == 11)); then
    printf '%s\n' '[{"id":1001}]'
  else
    printf '%s\n' '[]'
  fi
elif ((page == 1)); then
  jq -nc '[range(1;101) | {id:.}]'
elif ((page == 2)); then
  printf '%s\n' '[{"id":101}]'
else
  printf '%s\n' '[]'
fi
EOF
  chmod +x "${fake_bin}/gh"

  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" GH_MODE=normal OUTPUT_JSON="${output_json}" \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
    bash "${harness}"
  assert_eq 101 "$(jq 'length' "${output_json}")" \
    'bounded pagination must aggregate every proven page'
  assert_eq 101 "$(jq '.[-1].id' "${output_json}")" \
    'bounded pagination must preserve page order'

  set +e
  PATH="${fake_bin}:${PATH}" GH_CALLS="${calls}" GH_MODE=overflow OUTPUT_JSON="${output_json}" \
    GITHUB_OUTPUT="${root}/output" GITHUB_REPOSITORY=example/repo \
    AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_WRITER_APP_SLUG=aeris-sync AERIS_ISSUES_GH_TOKEN=test-issues-token \
    bash "${harness}" >/dev/null 2>&1
  status=$?
  set -e
  [[ ${status} -ne 0 ]] || fail 'bounded pagination accepted an eleventh non-empty page'
  grep -q -- '-f page=11' "${calls}" || fail 'bounded pagination did not prove its terminal page'
  ! grep -q -- '--paginate' "${calls}" || fail 'bounded pagination delegated to gh --paginate'
}

run_identity_case app 'aeris-writer[bot]' 102
run_identity_case legacy 'github-actions[bot]' 202
run_identity_case rest-create 'aeris-writer[bot]' '' 1
run_identity_case pr-comment 'aeris-writer[bot]' 102 1 true
run_publication_fence_drift_case
run_post_publish_pr_fence_cases
run_authoritative_identity_json_cases
run_bounded_api_pagination_cases
printf 'PASS sync upstream identity migration (%s)\n' "${RUN_ROOT}"
