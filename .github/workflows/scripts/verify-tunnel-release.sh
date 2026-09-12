#!/usr/bin/env bash
set -euo pipefail

manifest_path="${1:-SHA256SUMS.txt}"
envelope_path="${2:-SHA256SUMS.txt.sig}"

decode_base64() {
    if base64 --help 2>&1 | grep -q -- '--decode'; then
        base64 --decode
    else
        base64 -D
    fi
}

if [[ -z "${AETHER_TUNNEL_RELEASE_KEY_ID:-}" ]]; then
    echo "AETHER_TUNNEL_RELEASE_KEY_ID is required to verify the tunnel release manifest" >&2
    exit 1
fi
if [[ -z "${AETHER_TUNNEL_RELEASE_PUBLIC_KEY:-}" ]]; then
    echo "AETHER_TUNNEL_RELEASE_PUBLIC_KEY is required to verify the tunnel release manifest" >&2
    exit 1
fi
[[ -f "${manifest_path}" && -r "${manifest_path}" ]] || {
    echo "release manifest is missing or unreadable" >&2
    exit 1
}
[[ -f "${envelope_path}" && -r "${envelope_path}" ]] || {
    echo "release signature envelope is missing or unreadable" >&2
    exit 1
}

version_count="$(awk -F= '$1 == "version" { count++ } END { print count + 0 }' "${envelope_path}")"
key_id_count="$(awk -F= '$1 == "key_id" { count++ } END { print count + 0 }' "${envelope_path}")"
signature_count="$(awk -F= '$1 == "signature" { count++ } END { print count + 0 }' "${envelope_path}")"
[[ "${version_count}" == 1 && "${key_id_count}" == 1 && "${signature_count}" == 1 ]] || {
    echo "release signature envelope must contain one version, key_id, and signature" >&2
    exit 1
}
[[ "$(sed -n 's/^version=//p' "${envelope_path}")" == 1 ]] || {
    echo "unsupported release signature envelope version" >&2
    exit 1
}
[[ "$(sed -n 's/^key_id=//p' "${envelope_path}")" == "${AETHER_TUNNEL_RELEASE_KEY_ID}" ]] || {
    echo "release signature key id does not match the configured key" >&2
    exit 1
}

work_dir="$(mktemp -d)"
cleanup() {
    rm -rf -- "${work_dir}"
}
trap cleanup EXIT

raw_key="${work_dir}/public.der"
public_key="${work_dir}/public.pem"
signature="${work_dir}/signature.bin"
{ printf '302a300506032b6570032100' | xxd -r -p; printf '%s' "${AETHER_TUNNEL_RELEASE_PUBLIC_KEY}" | decode_base64; } >"${raw_key}"
[[ "$(wc -c <"${raw_key}")" -eq 44 ]] || {
    echo "configured tunnel release public key is malformed" >&2
    exit 1
}
openssl pkey -pubin -inform DER -in "${raw_key}" -out "${public_key}" >/dev/null 2>&1 || {
    echo "configured tunnel release public key is invalid" >&2
    exit 1
}
sed -n 's/^signature=//p' "${envelope_path}" | decode_base64 >"${signature}" || {
    echo "release signature is not valid base64" >&2
    exit 1
}
[[ "$(wc -c <"${signature}")" -eq 64 ]] || {
    echo "release signature has an invalid length" >&2
    exit 1
}
openssl pkeyutl -verify -rawin -pubin -inkey "${public_key}" \
    -in "${manifest_path}" -sigfile "${signature}" >/dev/null 2>&1 || {
    echo "release manifest signature verification failed" >&2
    exit 1
}
