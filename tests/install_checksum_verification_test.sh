#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
TEST_ROOT="$(mktemp -d "${AGENT_TMP_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/aether-installer-checksum.XXXXXX")"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

archive="${TEST_ROOT}/aether-release.tar.gz"
manifest="${TEST_ROOT}/SHA256SUMS"
asset="aether-release.tar.gz"
printf 'release fixture\n' >"${TEST_ROOT}/payload"
tar -C "${TEST_ROOT}" -czf "${archive}" payload
checksum="$(sha256sum "${archive}" | awk '{print $1}')"
printf '%s  %s\n' "${checksum}" "${asset}" >"${manifest}"

# shellcheck source=../install.sh
source "${REPO_ROOT}/install.sh"
verify_release_checksum "${archive}" "${manifest}" "${asset}"

assert_rejected() {
    if bash -c 'source "$1"; verify_release_checksum "$2" "$3" "$4"' \
        bash "${REPO_ROOT}/install.sh" "${archive}" "${manifest}" "${asset}" \
        >/dev/null 2>&1; then
        fail_test "accepted invalid checksum manifest"
    fi
}

# A release manifest must identify exactly one valid entry for the requested
# asset. This protects against ambiguous or malformed same-name records.
printf '%s  %s\n%s  %s\n' \
    "${checksum}" "${asset}" "${checksum}" "${asset}" >"${manifest}"
assert_rejected

printf '%064d  %s\n' 0 "${asset}" >"${manifest}"
assert_rejected

printf '%s  %s trailing-field\n' "${checksum}" "${asset}" >"${manifest}"
assert_rejected

echo "PASS: installer SHA256SUMS verification rejects ambiguous and malformed manifests"
