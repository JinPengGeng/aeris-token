#!/usr/bin/env bash
set -euo pipefail

# Validate the deploy-time contract before starting a multi-node gateway.
# This intentionally parses key/value presence without sourcing the file, so
# secrets are never executed or printed by the preflight check.

usage() {
    echo "usage: $0 ENV_FILE" >&2
    exit 2
}

[[ $# -eq 1 ]] || usage
env_file="$1"
[[ -f "$env_file" ]] || { echo "missing env file: $env_file" >&2; exit 1; }

value_for() {
    local key="$1"
    awk -v key="$key" '
        $0 ~ "^[[:space:]]*" key "=" {
            sub("^[[:space:]]*" key "=", "", $0)
            sub("^[[:space:]]*", "", $0)
            sub("[[:space:]]*#.*$", "", $0)
            gsub(/^"|"$/, "", $0)
            gsub(/^\x27|\x27$/, "", $0)
            print $0
            exit
        }
    ' "$env_file"
}

first_value() {
    local key value
    for key in "$@"; do
        value="$(value_for "$key")"
        if [[ -n "${value//[[:space:]]/}" ]]; then
            printf '%s' "$value"
            return 0
        fi
    done
    return 1
}

topology="$(first_value AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY DEPLOYMENT_TOPOLOGY || true)"
role="$(first_value AETHER_GATEWAY_NODE_ROLE NODE_ROLE || true)"
runtime="$(first_value AETHER_RUNTIME_BACKEND || true)"
database="$(first_value AETHER_DATABASE_URL DATABASE_URL AETHER_GATEWAY_DATA_POSTGRES_URL || true)"
redis="$(first_value REDIS_URL AETHER_GATEWAY_DATA_REDIS_URL || true)"
video_store="$(first_value AETHER_GATEWAY_VIDEO_TASK_STORE_PATH || true)"
instance="$(first_value AETHER_GATEWAY_INSTANCE_ID || true)"
relay="$(first_value AETHER_TUNNEL_RELAY_BASE_URL || true)"

normalize() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | xargs; }
topology="$(normalize "$topology")"
role="$(normalize "$role")"
runtime="$(normalize "$runtime")"

errors=0
require() {
    if [[ -z "${2//[[:space:]]/}" ]]; then
        echo "FAIL: $1" >&2
        errors=$((errors + 1))
    fi
}

if [[ "$topology" != "multi-node" ]]; then
    echo "FAIL: AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY must be multi-node (got ${topology:-unset})" >&2
    errors=$((errors + 1))
fi
if [[ "$role" == "all" ]]; then
    echo "FAIL: AETHER_GATEWAY_NODE_ROLE=all is invalid for multi-node" >&2
    errors=$((errors + 1))
fi
if [[ "$runtime" == "memory" ]]; then
    echo "FAIL: AETHER_RUNTIME_BACKEND=memory is invalid for multi-node" >&2
    errors=$((errors + 1))
fi
require "shared SQL URL (AETHER_DATABASE_URL, DATABASE_URL, or AETHER_GATEWAY_DATA_POSTGRES_URL)" "$database"
require "shared Redis URL (REDIS_URL or AETHER_GATEWAY_DATA_REDIS_URL)" "$redis"
if [[ -n "${video_store//[[:space:]]/}" ]]; then
    echo "FAIL: AETHER_GATEWAY_VIDEO_TASK_STORE_PATH must be unset for multi-node" >&2
    errors=$((errors + 1))
fi

[[ -n "${instance//[[:space:]]/}" ]] || echo "WARN: set a unique AETHER_GATEWAY_INSTANCE_ID for tunnel owner routing"
[[ -n "${relay//[[:space:]]/}" ]] || echo "WARN: set AETHER_TUNNEL_RELAY_BASE_URL when cross-node tunnel forwarding is required"

if (( errors > 0 )); then
    echo "multi-node preflight: FAILED ($errors contract violation(s))" >&2
    exit 1
fi

echo "multi-node preflight: PASS"
echo "topology=${topology} role=${role:-unset} runtime_backend=${runtime:-default}"
echo "shared_sql=present shared_redis=present video_task_store=unset"
[[ -n "${instance//[[:space:]]/}" ]] && echo "instance_id=present" || echo "instance_id=absent"
[[ -n "${relay//[[:space:]]/}" ]] && echo "relay_base_url=present" || echo "relay_base_url=absent"
