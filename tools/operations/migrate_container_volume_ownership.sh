#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 [--rollback] VOLUME [UID GID]" >&2
    exit 2
}

rollback=false
if [[ "${1:-}" == "--rollback" ]]; then
    rollback=true
    shift
fi
[[ $# -eq 1 || (!$rollback && $# -eq 3) ]] || usage
volume="$1"
[[ "$volume" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] || { echo "invalid Docker volume name" >&2; exit 2; }
if $rollback; then
    uid=0; gid=0
else
    uid="${2:-10001}"; gid="${3:-10001}"
    [[ "$uid" =~ ^[0-9]+$ && "$gid" =~ ^[0-9]+$ ]] || { echo "UID/GID must be numeric" >&2; exit 2; }
fi

docker volume inspect "$volume" >/dev/null
docker run --rm --user 0:0 -v "${volume}:/target" busybox:1.37.0-musl@sha256:fc6dddc4c44b1bfe37f41cae8e67d1693828e8f42a91862816d7953e2c9d3f23 \
    chown -R "${uid}:${gid}" /target
echo "Updated ${volume} ownership to ${uid}:${gid}. Reverse with: $0 --rollback ${volume}"
