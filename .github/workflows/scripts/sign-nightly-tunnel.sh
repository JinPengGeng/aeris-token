#!/usr/bin/env bash
set -euo pipefail

# This script receives the private key only in the protected signing step.
# All inputs are environment values, never interpolated shell source.
if [[ -z "${AETHER_TUNNEL_RELEASE_KEY_ID:-}" || -z "${AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM:-}" ]]; then
  echo 'release signing key configuration is missing; refusing to publish unsigned nightly tunnel artifacts' >&2
  exit 1
fi
signing_key="${AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM}"
unset AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
cargo run --quiet --locked --manifest-path "${repo_root}/tools/ci/tunnel-release-verifier/Cargo.toml" -- check
key_directory="$(mktemp -d "${RUNNER_TEMP:?}/nightly-tunnel-sign.XXXXXX")"
trap 'rm -f -- "${key_directory}/key.pem" "${key_directory}/manifest.sig"; rmdir -- "${key_directory}"' EXIT
umask 077
printf '%s\n' "${signing_key}" > "${key_directory}/key.pem"
unset signing_key
openssl pkeyutl -sign -rawin -inkey "${key_directory}/key.pem" -in SHA256SUMS.txt -out "${key_directory}/manifest.sig"
signature="$(openssl base64 -A -in "${key_directory}/manifest.sig")"
printf 'version=1\nkey_id=%s\nsignature=%s\n' "${AETHER_TUNNEL_RELEASE_KEY_ID}" "${signature}" > SHA256SUMS.txt.sig
manifest_sha256="$(openssl dgst -sha256 -r SHA256SUMS.txt | awk '{print $1}')"
jq -n \
  --arg tag "${RELEASE_TAG:?}" \
  --arg commit "${SOURCE_SHA:?}" \
  --arg workflow "${GITHUB_WORKFLOW_REF:?}" \
  --arg key_id "${AETHER_TUNNEL_RELEASE_KEY_ID}" \
  --arg manifest_sha256 "${manifest_sha256}" \
  '{version: "nightly", tag: $tag, source_commit: $commit, workflow: $workflow, key_id: $key_id, manifest_sha256: $manifest_sha256}' \
  > release-provenance.json
bash "${repo_root}/.github/workflows/scripts/verify-tunnel-release.sh"
