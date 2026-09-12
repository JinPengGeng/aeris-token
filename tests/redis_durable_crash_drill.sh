#!/usr/bin/env bash
set -euo pipefail

# This drill uses an isolated Compose project and volume. It must never be
# pointed at a production project or a pre-existing Redis volume.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="aether-redis-drill-${GITHUB_RUN_ID:-$$}"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/aether-redis-drill.XXXXXX")"
password="drill-secret-${RANDOM}-${RANDOM}"
lua_file="$repo_root/crates/aether-runtime/state/src/redis/dead_letter_redrive.lua"

cleanup() {
    docker compose -p "$project" -f "$fixture/base.yml" -f "$repo_root/docker-compose.redis-durable.yml" down -v --remove-orphans >/dev/null 2>&1 || true
    rm -rf "$fixture"
}
trap cleanup EXIT

cat >"$fixture/base.yml" <<'YAML'
services:
  redis:
    image: redis:7.4.11-alpine@sha256:ff02b58f971e7d7d156a1267e283fcbbeee91773b6aa36c49dac28ecfe28eadf
YAML
printf 'REDIS_PASSWORD=%s\n' "$password" >"$fixture/.env"

compose=(docker compose -p "$project" --project-directory "$fixture" --env-file "$fixture/.env" -f "$fixture/base.yml" -f "$repo_root/docker-compose.redis-durable.yml")
redis() {
    "${compose[@]}" exec -T redis redis-cli -a "$password" --no-auth-warning "$@"
}

"${compose[@]}" up -d --wait redis
test "$(redis ping)" = PONG

source_id="$(redis XADD usage:events:dlq '*' payload fixture)"
echo "seeded DLQ entry: $source_id"
target_before="$(redis XLEN usage:events)"
echo "target length before kill: $target_before"
test "$target_before" = 0
sleep 2
aof_status="$(redis INFO persistence | awk -F: '$1 == "aof_last_write_status" { print $2 }' | tr -d '\r')"
echo "AOF write status before kill: $aof_status"
test "$aof_status" = ok

"${compose[@]}" kill -s KILL redis
"${compose[@]}" up -d --wait redis
recovered="$(redis EXISTS "usage:events:dlq")"
echo "DLQ exists after restart: $recovered"
test "$recovered" = 1

lua="$(<"$lua_file")"
marker="usage:redrive:drill:$source_id"
first=( $(redis --raw EVAL "$lua" 3 usage:events:dlq usage:events "$marker" "$source_id" 0 payload fixture) )
echo "first redrive result: ${first[*]}"
test "${first[0]}" = 1
destination_id="${first[1]}"
second=( $(redis --raw EVAL "$lua" 3 usage:events:dlq usage:events "$marker" "$source_id" 0 payload fixture) )
echo "second redrive result: ${second[*]}"
test "${second[0]}" = 2
test "${second[1]}" = "$destination_id"
target_after="$(redis XLEN usage:events)"
source_after="$(redis EXISTS "usage:events:dlq")"
marker_value="$(redis GET "$marker")"
echo "target length after redrive: $target_after"
echo "DLQ exists after redrive: $source_after"
echo "redrive marker: $marker_value"
test "$target_after" = 1
test "$source_after" = 0
test "$marker_value" = "$destination_id"

echo "PASS: Redis durable kill/recovery and idempotent DLQ redrive ($source_id -> $destination_id)"
