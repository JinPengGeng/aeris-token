#!/usr/bin/env bash
set -euo pipefail

# Reproducible restore rehearsal. It deliberately requires an operator supplied,
# isolated DATABASE_URL and never creates or drops a database.
: "${DATABASE_URL:?set DATABASE_URL to the isolated restore database}"
: "${GATEWAY_URL:?set GATEWAY_URL, for example http://127.0.0.1:8080}"
: "${ADMIN_TOKEN:?set ADMIN_TOKEN to a short lived administrator bearer token}"
: "${RESTORED_JSON:?set RESTORED_JSON to authenticated data-export JSON}"

command -v psql >/dev/null || { echo "psql is required" >&2; exit 127; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 127; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 127; }

sql="select (select count(*) from public.users), (select count(*) from public.api_keys), (select count(*) from public.wallets), (select count(*) from public.usage), (select coalesce(sum(total_cost_usd),0)::numeric from public.usage);"
before="$(psql "$DATABASE_URL" -Atqc "$sql")"
started="$(date +%s)"
status="$(curl --fail-with-body --silent --show-error -o "$RESTORED_JSON.response" -w '%{http_code}' \
  -X POST "$GATEWAY_URL/api/admin/system/data/import" \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  --data-binary "@$RESTORED_JSON")"
elapsed=$(( $(date +%s) - started ))
test "$status" = 200 || { echo "restore HTTP status $status" >&2; cat "$RESTORED_JSON.response" >&2; exit 1; }
after="$(psql "$DATABASE_URL" -Atqc "$sql")"

export_json="$RESTORED_JSON.export"
curl --fail-with-body --silent --show-error "$GATEWAY_URL/api/admin/system/data/export" \
  -H "Authorization: Bearer $ADMIN_TOKEN" -o "$export_json"
jq -e '.user_data.users and .user_data.standalone_keys and .user_data.usage_aggregates' "$export_json" >/dev/null

printf 'restore_ok=true elapsed_seconds=%s\nbefore=%s\nafter=%s\n' "$elapsed" "$before" "$after"
printf 'operator_follow_up=verify login, API-key request, and one known billed usage row; this harness does not mint credentials\n'
