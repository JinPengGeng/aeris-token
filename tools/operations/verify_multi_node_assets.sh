#!/usr/bin/env bash
set -euo pipefail
root_dir="$(cd "$(dirname "$0")/../.." && pwd)"
compose_file="$root_dir/deploy/multi-node/docker-compose.yml"
[[ -f "$compose_file" ]] || { echo "missing $compose_file" >&2; exit 1; }
for env_file in "$root_dir"/deploy/multi-node/.env.node-*; do
  "$root_dir/tools/operations/check_multi_node_preflight.sh" "$env_file" >/dev/null
done
if command -v docker >/dev/null 2>&1; then
  docker compose -f "$compose_file" config --quiet
else
  echo "docker unavailable: skipped compose parser (env checks passed)" >&2
fi
echo "multi-node assets: PASS"

