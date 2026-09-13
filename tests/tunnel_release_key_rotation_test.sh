#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
VERIFIER_MANIFEST="${REPO_ROOT}/tools/ci/tunnel-release-verifier/Cargo.toml"
VERIFY_SCRIPT="${REPO_ROOT}/.github/workflows/scripts/verify-tunnel-release.sh"
FIXTURE="$(mktemp -d)"
trap 'rm -rf -- "${FIXTURE}"' EXIT
umask 077
unset AETHER_TUNNEL_RELEASE_TRUST_KEYS AETHER_TUNNEL_RELEASE_KEY_ID AETHER_TUNNEL_RELEASE_PUBLIC_KEY

fail() { echo "FAIL: $*" >&2; exit 1; }
reject() {
    if "$@" >"${FIXTURE}/rejection.log" 2>&1; then
        fail "invalid release fixture was accepted"
    fi
}
check_inputs() { cargo run --quiet --locked --manifest-path "${VERIFIER_MANIFEST}" -- check "$@"; }
verify() { "${VERIFY_SCRIPT}" "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/$1.sig"; }

printf 'abc123  aether-tunnel-linux-amd64.tar.gz\n' >"${FIXTURE}/SHA256SUMS.txt"
for key in old new; do
    openssl genpkey -algorithm ED25519 -out "${FIXTURE}/${key}.pem" >/dev/null 2>&1
    openssl pkey -in "${FIXTURE}/${key}.pem" -pubout -outform DER \
        -out "${FIXTURE}/${key}.der" >/dev/null 2>&1
    openssl pkeyutl -sign -rawin -inkey "${FIXTURE}/${key}.pem" \
        -in "${FIXTURE}/SHA256SUMS.txt" -out "${FIXTURE}/${key}.signature" >/dev/null 2>&1
    signature="$(base64 <"${FIXTURE}/${key}.signature" | tr -d '\n')"
    printf 'version=1\nkey_id=%s\nsignature=%s\n' "${key}" "${signature}" >"${FIXTURE}/${key}.sig"
done
old_public="$(dd if="${FIXTURE}/old.der" bs=1 skip=12 count=32 2>/dev/null | base64 | tr -d '\n')"
new_public="$(dd if="${FIXTURE}/new.der" bs=1 skip=12 count=32 2>/dev/null | base64 | tr -d '\n')"
overlap="[{\"key_id\":\"old\",\"public_key\":\"${old_public}\"},{\"key_id\":\"new\",\"public_key\":\"${new_public}\"}]"
retired="[{\"key_id\":\"new\",\"public_key\":\"${new_public}\"}]"

cargo test --quiet --locked --manifest-path "${VERIFIER_MANIFEST}"
reject check_inputs
check_inputs --allow-unconfigured
export AETHER_TUNNEL_RELEASE_KEY_ID=old AETHER_TUNNEL_RELEASE_PUBLIC_KEY="${old_public}"
check_inputs
verify old
reject verify new
export AETHER_TUNNEL_RELEASE_TRUST_KEYS="${overlap}"
check_inputs
verify old
export AETHER_TUNNEL_RELEASE_KEY_ID=new
# A stale single-key input must block switching the signer.
reject check_inputs
export AETHER_TUNNEL_RELEASE_PUBLIC_KEY=""
check_inputs
verify new
# Publishing must use the configured signer, even while both are trusted by clients.
reject verify old
export AETHER_TUNNEL_RELEASE_TRUST_KEYS="${retired}"
check_inputs
verify new
export AETHER_TUNNEL_RELEASE_KEY_ID=old AETHER_TUNNEL_RELEASE_PUBLIC_KEY="${old_public}"
reject check_inputs
export AETHER_TUNNEL_RELEASE_KEY_ID=unknown AETHER_TUNNEL_RELEASE_PUBLIC_KEY=""
reject check_inputs
export AETHER_TUNNEL_RELEASE_KEY_ID=new AETHER_TUNNEL_RELEASE_TRUST_KEYS='[]'
reject check_inputs
export AETHER_TUNNEL_RELEASE_TRUST_KEYS="${retired}"
printf 'unexpected=field\n' >>"${FIXTURE}/new.sig"
reject verify new

# Exercise option_env! in the actual production verifier. Mutating process env
# after compilation cannot change the public trust set embedded in the binary.
# Use a private target directory so fixtures cannot race another build's env.
export CARGO_TARGET_DIR="${FIXTURE}/target"
binary="${CARGO_TARGET_DIR}/debug/tunnel-release-verifier"
export AETHER_TUNNEL_RELEASE_TRUST_KEYS="${overlap}" AETHER_TUNNEL_RELEASE_KEY_ID=old
cargo build --quiet --locked --manifest-path "${VERIFIER_MANIFEST}"
sed '$d' "${FIXTURE}/new.sig" >"${FIXTURE}/new-valid.sig"
"${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/old.sig"
"${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/new-valid.sig"
export AETHER_TUNNEL_RELEASE_TRUST_KEYS="${retired}" AETHER_TUNNEL_RELEASE_KEY_ID=new
"${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/old.sig"
cargo build --quiet --locked --manifest-path "${VERIFIER_MANIFEST}"
"${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/new-valid.sig"
reject "${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/old.sig"
export AETHER_TUNNEL_RELEASE_TRUST_KEYS="" AETHER_TUNNEL_RELEASE_KEY_ID=old AETHER_TUNNEL_RELEASE_PUBLIC_KEY="${old_public}"
cargo build --quiet --locked --manifest-path "${VERIFIER_MANIFEST}"
"${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/old.sig"
reject "${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/new-valid.sig"
printf 'tampered\n' >>"${FIXTURE}/SHA256SUMS.txt"
reject "${binary}" verify-embedded "${FIXTURE}/SHA256SUMS.txt" "${FIXTURE}/old.sig"

echo "PASS: release trust inputs, embedded overlap, signer switch, retirement and tamper rejection"
