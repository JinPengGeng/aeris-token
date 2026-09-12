#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_root="$(mktemp -d)"
trap 'rm -rf "$tmp_root"' EXIT

mkdir -p "$tmp_root/bin" "$tmp_root/.github/scripts" "$tmp_root/.github/security"
cp "$repo_root/.github/scripts/cargo-audit-gate.sh" "$tmp_root/.github/scripts/"
cp "$repo_root/.github/security/cargo-audit-exception.json" "$tmp_root/.github/security/"
cp "$repo_root/Cargo.lock" "$tmp_root/Cargo.lock"

# Keep this test independent of installed cargo-audit and the full dependency
# graph. The gate's lockfile and exception checks still run unmodified.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [[ "${1:-}" == "tree" ]]; then exit 0; fi' \
  'if [[ "${1:-}" == "audit" ]]; then exit 0; fi' \
  'echo "unexpected cargo invocation: $*" >&2' \
  'exit 1' > "$tmp_root/bin/cargo"
chmod +x "$tmp_root/bin/cargo"

run_gate() (
  cd "$tmp_root"
  PATH="$tmp_root/bin:$PATH" bash .github/scripts/cargo-audit-gate.sh
)

run_gate

printf '\n[[package]]\nname = "rsa"\nversion = "0.9.9"\n' >> "$tmp_root/Cargo.lock"
if run_gate; then
  echo "gate accepted an additional rsa version" >&2
  exit 1
fi

echo "cargo audit gate exact rsa package test passed"
