#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-sync-git-auth.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

fake_bin="${RUN_ROOT}/bin"
calls="${RUN_ROOT}/git-calls"
askpass_paths="${RUN_ROOT}/askpass-paths"
harness="${RUN_ROOT}/harness.sh"
mkdir -p "${fake_bin}"
: >"${calls}"
: >"${askpass_paths}"

cat >"${fake_bin}/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GIT_CALLS}"
if [[ "${1:-}" == fetch ]]; then
  [[ -z "${GIT_ASKPASS:-}" && -z "${GIT_ASKPASS_REQUIRE:-}" ]] || {
    printf 'ordinary Git reads inherited Writer askpass credentials\n' >&2
    exit 1
  }
  exit 0
fi
[[ "${GIT_ASKPASS_REQUIRE:-}" == force ]] || {
  printf 'GIT_ASKPASS_REQUIRE was not forced\n' >&2
  exit 1
}
[[ "${GIT_TERMINAL_PROMPT:-}" == 0 ]] || {
  printf 'terminal credential prompts were not disabled\n' >&2
  exit 1
}
[[ -x "${GIT_ASKPASS:-}" ]] || {
  printf 'ephemeral askpass helper was not executable\n' >&2
  exit 1
}
[[ "$(stat -c '%a' "${GIT_ASKPASS}")" == 700 ]] || {
  printf 'ephemeral askpass helper had unsafe permissions\n' >&2
  exit 1
}
[[ "$*" == '-c credential.helper= -c http.https://github.com/.extraheader= push --force-with-lease='* ]] || {
  printf 'push did not disable persistent credential helpers or preserve its lease\n' >&2
  exit 1
}
[[ "$*" != *"${AERIS_WRITER_TOKEN}"* ]] || {
  printf 'Writer token leaked into Git arguments\n' >&2
  exit 1
}
[[ "$("${GIT_ASKPASS}" "Username for 'https://github.com':")" == x-access-token ]] || {
  printf 'askpass returned the wrong username\n' >&2
  exit 1
}
[[ "$("${GIT_ASKPASS}" "Password for 'https://github.com':")" == "${AERIS_WRITER_TOKEN}" ]] || {
  printf 'askpass returned the wrong password\n' >&2
  exit 1
}
if "${GIT_ASKPASS}" 'unexpected prompt' >/dev/null 2>&1; then
  printf 'askpass accepted an unexpected prompt\n' >&2
  exit 1
fi
printf '%s\n' "${GIT_ASKPASS}" >>"${ASKPASS_PATHS}"
exit "${FAKE_GIT_EXIT:-0}"
EOF
chmod +x "${fake_bin}/git"

sed 's/\r$//' "${SCRIPT_ROOT}/github-autonomy.sh" >"${RUN_ROOT}/github-autonomy.sh"
sed '/^parent="$(aeris_gh api "repos\/\${GITHUB_REPOSITORY}"/,$d' \
  "${SCRIPT_ROOT}/sync-upstream.sh" | sed 's/\r$//' >"${harness}"
cat >>"${harness}" <<'EOF'
aeris_git_network fetch --no-tags origin main
aeris_writer_git_push push \
  --force-with-lease=refs/heads/automation/sync-upstream:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  https://github.com/example/repo.git bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:refs/heads/automation/sync-upstream
if FAKE_GIT_EXIT=17 aeris_writer_git_push push \
  --force-with-lease=refs/heads/automation/sync-upstream: \
  https://github.com/example/repo.git cccccccccccccccccccccccccccccccccccccccc:refs/heads/automation/sync-upstream; then
  printf 'failed Git push was reported as successful\n' >&2
  exit 1
else
  status=$?
  [[ "${status}" -eq 17 ]] || {
    printf 'failed Git push status changed: %s\n' "${status}" >&2
    exit 1
  }
fi
if AERIS_AUTONOMY_EXPIRES_AT=1970-01-01T00:00:00Z aeris_writer_git_push push \
  --force-with-lease=refs/heads/automation/sync-upstream: \
  https://github.com/example/repo.git dddddddddddddddddddddddddddddddddddddddd:refs/heads/automation/sync-upstream; then
  printf 'expired Writer push was reported as successful\n' >&2
  exit 1
else
  status=$?
  [[ "${status}" -eq 78 ]] || {
    printf 'expired Writer push status changed: %s\n' "${status}" >&2
    exit 1
  }
fi
EOF

PATH="${fake_bin}:${PATH}" GIT_CALLS="${calls}" ASKPASS_PATHS="${askpass_paths}" \
  GH_TOKEN=test-gh-token AERIS_WRITER_TOKEN=test-writer-token \
  GITHUB_OUTPUT="${RUN_ROOT}/output" GITHUB_REPOSITORY=example/repo \
  AERIS_AUTONOMY_EXPIRES_AT=2099-01-01T00:00:00Z AERIS_ISSUES_GH_TOKEN=test-issues-token \
  AERIS_WRITER_APP_SLUG=aeris-writer RUNNER_TEMP="${RUN_ROOT}" bash "${harness}"

[[ "$(wc -l <"${calls}")" -eq 3 ]] || fail 'expected one Git read and exactly two Git push attempts'
grep -q '^fetch --no-tags origin main$' "${calls}" || fail 'ordinary Git read did not run without askpass'
grep -q -- '--force-with-lease=refs/heads/automation/sync-upstream:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' "${calls}" ||
  fail 'existing-branch lease was not preserved'
grep -q -- '--force-with-lease=refs/heads/automation/sync-upstream: ' "${calls}" ||
  fail 'missing-branch lease was not preserved'
if grep -q -- 'test-writer-token\|x-access-token@' "${calls}"; then
  fail 'credentials were embedded in Git arguments'
fi
while IFS= read -r askpass; do
  [[ ! -e "${askpass}" ]] || fail "askpass helper was not removed: ${askpass}"
  [[ ! -d "$(dirname "${askpass}")" ]] || fail "askpass directory was not removed: $(dirname "${askpass}")"
done <"${askpass_paths}"
[[ "$(wc -l <"${askpass_paths}")" -eq 2 ]] || fail 'askpass cleanup was not observed for both push outcomes'
if find "${RUN_ROOT}" -maxdepth 1 -type d -name 'aeris-writer-askpass.*' | grep -q .; then
  fail 'an askpass directory survived success, failure, or expiry'
fi

printf 'PASS sync upstream Git authentication (%s)\n' "${RUN_ROOT}"
