#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Like the existing restore drill, own a socket-only PostgreSQL instance and
# retain its evidence. Never migrate or clear a caller's database.
for executable in initdb pg_ctl createdb cargo jq; do
  command -v "$executable" >/dev/null || { printf 'Missing executable: %s\n' "$executable" >&2; exit 2; }
done
fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/aether-admin-audit-ci.XXXXXX")"
fixture_dir="$(cd "$fixture_dir" && pwd -P)"
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -f "$fixture_dir/postgres/postmaster.pid" ]]; then
    pg_ctl -D "$fixture_dir/postgres" stop -m fast -w -t 10 >>"$fixture_dir/cleanup.log" 2>&1 || result=1
  fi
  printf 'Administrator audit evidence retained: %s\n' "$fixture_dir"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

initdb -D "$fixture_dir/postgres" -U audit_drill --auth-local=trust --auth-host=reject \
  --encoding=UTF8 --no-locale >"$fixture_dir/initdb.log" 2>&1
[[ "$fixture_dir" != *"'"* && "$fixture_dir" != *$'\n'* ]] || { printf 'Unsupported temporary path\n' >&2; exit 2; }
pg_ctl -D "$fixture_dir/postgres" -l "$fixture_dir/postgres.log" \
  -o "-k '$fixture_dir' -p 5432 -c listen_addresses=''" -w -t 10 start
createdb -h "$fixture_dir" -p 5432 -U audit_drill aether_admin_audit_ci
encoded_socket="$(jq -rn --arg socket "$fixture_dir" '$socket | @uri')"
export AETHER_TEST_AUDIT_DATABASE_URL="postgresql://audit_drill@localhost/aether_admin_audit_ci?host=$encoded_socket&port=5432&sslmode=disable"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
target="tests::audit::admin_persistence::live_admin_mutations_persist_before_response_and_protected_readback"
cargo test --locked -p aether-gateway --lib "$target" -- \
  --exact --include-ignored --nocapture --test-threads=1 2>&1 | tee "$fixture_dir/audit.log"
printf 'PASS: administrator HTTP mutation and PostgreSQL audit readback\n'
