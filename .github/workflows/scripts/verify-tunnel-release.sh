#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
# Reuse the tunnel's strict parser and Ed25519 verifier, including all trust keys.
exec cargo run --quiet --locked \
    --manifest-path "${repo_root}/tools/ci/tunnel-release-verifier/Cargo.toml" \
    -- verify "${1:-SHA256SUMS.txt}" "${2:-SHA256SUMS.txt.sig}"
