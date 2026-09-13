#!/usr/bin/env bash
set -euo pipefail

umask 077
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

# Exercise the real authenticated CLI with synthetic credentials and a fresh DB.
: "${AETHER_TEST_RESTORE_DATABASE_URL:?set an empty disposable aether_restore_drill_* PostgreSQL database}"
command -v cargo >/dev/null || { echo "cargo is required" >&2; exit 127; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 127; }

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd -- "$repo_root"
cargo build --locked -p aether-gateway --bin aether-backup-restore
target_directory="$(cargo metadata --locked --no-deps --format-version 1 | jq -er '.target_directory')"
export AETHER_TEST_BACKUP_RESTORE_BIN="$target_directory/debug/aether-backup-restore"

drill_log="$(mktemp "${TMPDIR:-/tmp}/aether-restore-drill.XXXXXXXX")"
started="$(date +%s)"
test_name='backup::restore_drill_tests::live_authenticated_restore_cli_preserves_credentials_wallets_and_aggregates'
cargo test --locked -p aether-gateway --lib "$test_name" -- \
  --exact --include-ignored --test-threads=1 | tee "$drill_log"
grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored;' "$drill_log" || {
  printf 'restore exercise did not execute the expected test; log=%s\n' "$drill_log" >&2
  exit 1
}
printf 'restore_drill_verified=true fixture=synthetic elapsed_seconds=%s log=%s\n' \
  "$(( $(date +%s) - started ))" "$drill_log"
