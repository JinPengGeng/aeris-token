#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Own the PostgreSQL instance and database; never reuse a caller's database URL.
for executable in initdb pg_ctl createdb cargo jq; do
  command -v "$executable" >/dev/null || { printf 'Missing executable: %s\n' "$executable" >&2; exit 2; }
done
fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/aether-restore-ci.XXXXXX")"
fixture_dir="$(cd "$fixture_dir" && pwd -P)"
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -f "$fixture_dir/postgres/postmaster.pid" ]]; then
    pg_ctl -D "$fixture_dir/postgres" stop -m fast -w -t 10 >>"$fixture_dir/cleanup.log" 2>&1 || result=1
  fi
  printf 'Restore drill evidence retained: %s\n' "$fixture_dir"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

initdb -D "$fixture_dir/postgres" -U restore_drill --auth-local=trust --auth-host=reject \
  --encoding=UTF8 --no-locale >"$fixture_dir/initdb.log" 2>&1
# Distinct private socket directories allow port 5432 without a shared TCP listener.
# mktemp paths used here contain no quote characters; pg_ctl parses this option string.
[[ "$fixture_dir" != *"'"* && "$fixture_dir" != *$'\n'* ]] || { printf 'Unsupported temporary path\n' >&2; exit 2; }
pg_ctl -D "$fixture_dir/postgres" -l "$fixture_dir/postgres.log" \
  -o "-k '$fixture_dir' -p 5432 -c listen_addresses=''" -w -t 10 start
createdb -h "$fixture_dir" -p 5432 -U restore_drill aether_restore_drill_ci
encoded_socket="$(jq -rn --arg socket "$fixture_dir" '$socket | @uri')"
export AETHER_TEST_RESTORE_DATABASE_URL="postgresql://restore_drill@localhost/aether_restore_drill_ci?host=$encoded_socket&port=5432&sslmode=disable"
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
bash "$repo_root/tools/operations/backup_restore_drill.sh" 2>&1 | tee "$fixture_dir/restore.log"
