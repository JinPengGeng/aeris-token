#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-autonomy.XXXXXX")"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

expect_rejected() {
  local expires_at="$1"
  if AERIS_AUTONOMY_EXPIRES_AT="${expires_at}" bash "${SCRIPT_ROOT}/github-autonomy.sh" >/dev/null 2>&1; then
    fail "autonomy value was accepted: ${expires_at:-<missing>}"
  fi
}

test_strict_expiry_values() {
  if env -u AERIS_AUTONOMY_EXPIRES_AT bash "${SCRIPT_ROOT}/github-autonomy.sh" >/dev/null 2>&1; then
    fail 'missing expiry was accepted'
  fi
  expect_rejected 'not-a-date'
  expect_rejected '2026-02-30T00:00:00Z'
  expect_rejected '1970-01-01T00:00:00Z'
  AERIS_AUTONOMY_EXPIRES_AT='2099-01-01T00:00:00Z' \
    bash "${SCRIPT_ROOT}/github-autonomy.sh"
}

test_expiry_between_plan_and_mutation() {
  local fake_bin="${RUN_ROOT}/bin" gh_calls="${RUN_ROOT}/gh-calls" clock_calls="${RUN_ROOT}/clock-calls"
  mkdir -p "${fake_bin}"
  : >"${gh_calls}"
  : >"${clock_calls}"

  cat >"${fake_bin}/date" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  '-u -d 2033-05-18T03:33:20Z +%s') printf '2000000000\n' ;;
  '-u -d @2000000000 +%Y-%m-%dT%H:%M:%SZ') printf '2033-05-18T03:33:20Z\n' ;;
  '-u +%s')
    count="$(wc -l <"${CLOCK_CALLS}")"
    printf 'tick\n' >>"${CLOCK_CALLS}"
    if [[ "${count}" -eq 0 ]]; then printf '1999999999\n'; else printf '2000000000\n'; fi
    ;;
  *) printf 'unexpected date invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GH_CALLS}"
EOF
  chmod +x "${fake_bin}/date" "${fake_bin}/gh"

  (
    export PATH="${fake_bin}:${PATH}"
    export CLOCK_CALLS="${clock_calls}" GH_CALLS="${gh_calls}"
    export AERIS_AUTONOMY_EXPIRES_AT='2033-05-18T03:33:20Z'
    source "${SCRIPT_ROOT}/github-autonomy.sh"
    aeris_gh api repos/example/repo
    if aeris_gh api --method POST repos/example/repo/issues -f title=blocked; then
      fail 'mutation ran after the autonomy deadline'
    fi
  )

  [[ "$(wc -l <"${gh_calls}")" -eq 1 ]] ||
    fail 'expired mutation reached gh'
  grep -q '^api repos/example/repo$' "${gh_calls}" ||
    fail 'planning read did not run before expiry'
}

test_strict_expiry_values
test_expiry_between_plan_and_mutation
printf 'PASS GitHub autonomy expiry (%s)\n' "${RUN_ROOT}"
