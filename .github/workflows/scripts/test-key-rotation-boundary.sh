#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd -P)"

if rg -n "tunnel_signing_key_id|TunnelSigningKeySet" \
    "${repo_root}/apps/aether-gateway" "${repo_root}/crates/aether-gateway" \
    >/dev/null 2>&1; then
    echo "FAIL: gateway wiring appeared without the required integration review" >&2
    exit 1
fi

echo "PASS: tunnel key rotation remains contract-only pending gateway/wire ADR"
