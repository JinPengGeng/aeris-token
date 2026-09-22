#!/usr/bin/env bash
set -euo pipefail
root_dir="$(cd "$(dirname "$0")/../.." && pwd)"
compose_file="$root_dir/deploy/multi-node/docker-compose.yml"
standalone_compose_file="$root_dir/deploy/multi-node/docker-compose.standalone.yml"
standalone_env_example="$root_dir/deploy/multi-node/.env.standalone.example"
[[ -f "$compose_file" ]] || { echo "missing $compose_file" >&2; exit 1; }
[[ -f "$standalone_compose_file" ]] || { echo "missing $standalone_compose_file" >&2; exit 1; }
[[ -f "$standalone_env_example" ]] || { echo "missing $standalone_env_example" >&2; exit 1; }
for env_file in "$root_dir"/deploy/multi-node/.env.node-*; do
  "$root_dir/tools/operations/check_multi_node_preflight.sh" "$env_file" >/dev/null
done
if command -v docker >/dev/null 2>&1; then
  for env_file in "$root_dir"/deploy/multi-node/.env.node-*; do
    docker compose --env-file "$env_file" -f "$compose_file" config --quiet
  done
  # standalone compose 与单 env 模板配对校验。
  docker compose --env-file "$standalone_env_example" -f "$standalone_compose_file" config --quiet
else
  echo "docker unavailable: skipped compose parser (env checks passed)" >&2
fi
echo "multi-node assets: PASS"
