#!/usr/bin/env bash
# One-click updater for Docker Compose deployments.
#
# This updates the app container image and recreates only the app service. It is
# intentionally not a hot patch of the running Rust process.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

MODE="auto"
COMPOSE_DIR=""
APP_SERVICE="app"
BACKUP_SERVICE="postgres"
BACKUP_DIR=""
COMPOSE_WAIT_TIMEOUT_SECS=120
COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS=2
NO_PULL=false
FORCE_RECREATE=false
SHOW_LOGS=false
LOCAL_BUILD=false
PREPARE_ONLY=false
SKIP_BACKUP=false
ALLOW_FLOATING_TAG=false
COMPOSE_FILES=()
COMPOSE=()
COMPOSE_ARGS=()

usage() {
    cat <<'EOF'
Usage: ./update.sh [options]

Update Aether Docker Compose deployment in one command.

Options:
  --mode MODE             auto, compose, single-node, or local-build
                          auto uses docker-compose.yml in the current directory
  --compose-dir DIR       deployment directory, default: current directory
  -f, --compose-file FILE compose file path; can be provided multiple times
  --service NAME          app service name, default: app
  --backup-service NAME   PostgreSQL service to dump before updates, default: postgres
  --backup-dir DIR        directory for pre-update pg_dump files, default: ./backups
  --no-pull               skip docker compose pull
  --prepare               pull the latest app image only, do not recreate app
  --skip-backup           skip the pre-update PostgreSQL backup (explicitly unsafe)
  --allow-floating-tag    allow rolling image tags such as :latest or :nightly
  --force-recreate        force recreate the app container
  --logs                  follow app logs after update
  -h, --help              show help

Examples:
  ./update.sh
  ./update.sh --mode single-node
  ./update.sh --compose-dir /opt/aether/compose
  ./update.sh --mode local-build
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            [[ $# -ge 2 ]] || die "--mode requires a value"
            MODE="$2"
            shift 2
            ;;
        --compose-dir)
            [[ $# -ge 2 ]] || die "--compose-dir requires a value"
            COMPOSE_DIR="$2"
            shift 2
            ;;
        -f|--compose-file)
            [[ $# -ge 2 ]] || die "--compose-file requires a value"
            COMPOSE_FILES+=("$2")
            shift 2
            ;;
        --service)
            [[ $# -ge 2 ]] || die "--service requires a value"
            APP_SERVICE="$2"
            shift 2
            ;;
        --backup-service)
            [[ $# -ge 2 ]] || die "--backup-service requires a value"
            BACKUP_SERVICE="$2"
            shift 2
            ;;
        --backup-dir)
            [[ $# -ge 2 ]] || die "--backup-dir requires a value"
            BACKUP_DIR="$2"
            shift 2
            ;;
        --no-pull)
            NO_PULL=true
            shift
            ;;
        --prepare)
            PREPARE_ONLY=true
            shift
            ;;
        --skip-backup)
            SKIP_BACKUP=true
            shift
            ;;
        --allow-floating-tag)
            ALLOW_FLOATING_TAG=true
            shift
            ;;
        --force-recreate)
            FORCE_RECREATE=true
            shift
            ;;
        --logs)
            SHOW_LOGS=true
            shift
            ;;
        --local-build)
            MODE="local-build"
            LOCAL_BUILD=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

case "$MODE" in
    auto|compose|single-node|local-build)
        ;;
    *)
        die "unsupported mode: ${MODE}; expected auto, compose, single-node, or local-build"
        ;;
esac

[[ -n "${APP_SERVICE}" && ${#APP_SERVICE} -le 128 \
    && "${APP_SERVICE}" =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]*$ ]] \
    || die "service name contains unsafe characters"
[[ -n "${BACKUP_SERVICE}" && ${#BACKUP_SERVICE} -le 128 \
    && "${BACKUP_SERVICE}" =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]*$ ]] \
    || die "backup service name contains unsafe characters"

if [[ "${MODE}" == "local-build" || "${LOCAL_BUILD}" == "true" ]]; then
    [[ "${PREPARE_ONLY}" != "true" ]] || die "--prepare is only supported for Docker Compose deployments"
    deploy_script="${SCRIPT_DIR}/deploy.sh"
    [[ -f "${deploy_script}" ]] || die "local-build mode requires deploy.sh next to update.sh"
    args=()
    if [[ "${FORCE_RECREATE}" == "true" ]]; then
        args+=(--force)
    fi
    exec bash "${deploy_script}" "${args[@]}"
fi

resolve_compose_cli() {
    if [[ "${#COMPOSE[@]}" -gt 0 ]]; then
        return
    fi

    if docker compose version >/dev/null 2>&1; then
        COMPOSE=(docker compose)
        return
    fi

    if command -v docker-compose >/dev/null 2>&1; then
        COMPOSE=(docker-compose)
        return
    fi

    die "docker compose or docker-compose is required"
}

compose() {
    "${COMPOSE[@]}" "${COMPOSE_ARGS[@]}" "$@"
}

compose_config() {
    compose config "$@"
}

resolve_compose_cli

docker info >/dev/null 2>&1 || die "Docker is not running"

if [[ -z "${COMPOSE_DIR}" ]]; then
    COMPOSE_DIR="$(pwd -P)"
fi
COMPOSE_DIR="$(cd -- "${COMPOSE_DIR}" && pwd -P)"
if [[ -z "${BACKUP_DIR}" ]]; then
    BACKUP_DIR="${COMPOSE_DIR}/backups"
elif [[ "${BACKUP_DIR}" != /* ]]; then
    BACKUP_DIR="${COMPOSE_DIR}/${BACKUP_DIR}"
else
    BACKUP_DIR="${BACKUP_DIR}"
fi

resolve_compose_file() {
    local filename="$1"
    if [[ "${filename}" = /* ]]; then
        printf '%s\n' "${filename}"
    else
        printf '%s\n' "${COMPOSE_DIR}/${filename}"
    fi
}

resolve_default_compose_files() {
    case "${MODE}" in
        compose)
            COMPOSE_FILES=("docker-compose.yml")
            ;;
        single-node)
            if [[ -f "${COMPOSE_DIR}/docker-compose.single-node.yml" ]]; then
                COMPOSE_FILES=("docker-compose.single-node.yml")
            else
                COMPOSE_FILES=("docker-compose.yml")
            fi
            ;;
        auto)
            if [[ -f "${COMPOSE_DIR}/docker-compose.yml" ]]; then
                COMPOSE_FILES=("docker-compose.yml")
            elif [[ -f "${COMPOSE_DIR}/docker-compose.single-node.yml" ]]; then
                COMPOSE_FILES=("docker-compose.single-node.yml")
            else
                die "no docker-compose.yml or docker-compose.single-node.yml found in ${COMPOSE_DIR}"
            fi
            ;;
    esac
}

if [[ "${#COMPOSE_FILES[@]}" -eq 0 ]]; then
    resolve_default_compose_files
fi

COMPOSE_ARGS+=(--project-directory "${COMPOSE_DIR}")
for file in "${COMPOSE_FILES[@]}"; do
    resolved_file="$(resolve_compose_file "${file}")"
    [[ -f "${resolved_file}" ]] || die "compose file not found: ${resolved_file}"
    COMPOSE_ARGS+=(-f "${resolved_file}")
done

services="$(compose_config --services)"
if ! grep -Fqx -- "${APP_SERVICE}" <<< "${services}"; then
    die "service '${APP_SERVICE}' not found in compose config"
fi

configured_app_image="$(compose_config | awk -v service="${APP_SERVICE}" '
    $0 == "  " service ":" { in_service = 1; next }
    in_service && $0 ~ /^  [^[:space:]]/ { exit }
    in_service && $1 == "image:" { print $2; exit }
')"
configured_app_image="${configured_app_image%\"}"
configured_app_image="${configured_app_image#\"}"

is_floating_image_tag() {
    local image_ref="$1"
    [[ "${image_ref}" != *@* ]] || return 1
    local image_tag="${image_ref##*:}"
    [[ "${image_tag}" == "latest" || "${image_tag}" == "nightly" ]]
}

if [[ -n "${configured_app_image}" ]] \
    && is_floating_image_tag "${configured_app_image}" \
    && [[ "${ALLOW_FLOATING_TAG}" != "true" ]]; then
    die "app image '${configured_app_image}' is a floating tag; set APP_IMAGE to an explicit version or pass --allow-floating-tag"
fi

previous_container_id="$(compose ps -q "${APP_SERVICE}" 2>/dev/null | head -n 1 || true)"
previous_image_ref=""
if [[ -n "${previous_container_id}" ]]; then
    previous_image_ref="$(docker inspect --format='{{.Config.Image}}' "${previous_container_id}" 2>/dev/null || true)"
fi

echo ">>> Compose directory: ${COMPOSE_DIR}"
echo ">>> App service: ${APP_SERVICE}"
if [[ -n "${configured_app_image}" ]]; then
    echo ">>> Target app image: ${configured_app_image}"
fi

create_pre_update_backup() {
    local timestamp backup_file temp_file
    mkdir -p -- "${BACKUP_DIR}"
    timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
    backup_file="${BACKUP_DIR}/aether-postgres-${timestamp}.dump"
    temp_file="${backup_file}.tmp.$$"
    echo ">>> Creating PostgreSQL pre-update backup: ${backup_file}"
    if ! compose exec -T "${BACKUP_SERVICE}" pg_dump \
        --format=custom --no-owner --no-acl \
        -U "${POSTGRES_USER:-postgres}" -d "${POSTGRES_DB:-aether}" \
        >"${temp_file}"; then
        rm -f -- "${temp_file}"
        die "pre-update PostgreSQL backup failed; update was not attempted"
    fi
    if [[ ! -s "${temp_file}" ]]; then
        rm -f -- "${temp_file}"
        die "pre-update PostgreSQL backup was empty; update was not attempted"
    fi
    mv -- "${temp_file}" "${backup_file}"
    printf 'created_at=%s\napp_image=%s\nbackup_file=%s\n' \
        "${timestamp}" "${configured_app_image:-unknown}" "${backup_file}" \
        >"${backup_file}.meta"
    echo ">>> PostgreSQL backup completed. Metadata: ${backup_file}.meta"
}

rollback_app() {
    if [[ -z "${previous_image_ref}" ]]; then
        echo ">>> WARNING: previous app image is unavailable; automatic rollback skipped." >&2
        return 1
    fi
    echo ">>> Updated app failed health verification; restoring ${previous_image_ref}..."
    (
        export APP_IMAGE="${previous_image_ref}"
        if compose_supports_wait; then
            compose_up_app true
        else
            compose_up_app false
            wait_healthy || return 1
        fi
    ) || {
        echo ">>> WARNING: automatic app rollback failed; restore from the pre-update backup and inspect Compose state." >&2
        return 1
    }
    echo ">>> Previous app image restored and healthy. Database migrations are not automatically reversed."
}

compose_pull_app() {
    compose pull "${APP_SERVICE}"
}

compose_up_app() {
    local wait_for_health="${1:-false}"
    local -a up_args=(up -d)

    if [[ "${FORCE_RECREATE}" == "true" ]]; then
        up_args+=(--force-recreate)
    fi
    if [[ "${wait_for_health}" == "true" ]]; then
        up_args+=(--wait --wait-timeout "${COMPOSE_WAIT_TIMEOUT_SECS}")
    fi

    up_args+=("${APP_SERVICE}")
    compose "${up_args[@]}"
}

compose_supports_wait() {
    local help
    help="$(compose up --help 2>/dev/null)" || return 1
    grep -Fq -- '--wait' <<<"${help}"
}

wait_healthy() {
    local timeout="${1:-${COMPOSE_WAIT_TIMEOUT_SECS}}"
    local elapsed=0
    echo ">>> Waiting for ${APP_SERVICE} to become healthy (timeout ${timeout}s)..."
    while (( elapsed < timeout )); do
        local container_id
        local state
        container_id="$(compose ps -q "${APP_SERVICE}" 2>/dev/null | head -n 1)"
        if [[ -z "${container_id}" ]]; then
            sleep "${COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS}"
            elapsed=$(( elapsed + COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS ))
            continue
        fi
        state="$(docker inspect --format='{{.State.Health.Status}}' \
            "${container_id}" 2>/dev/null || true)"
        if [[ "${state}" == "healthy" ]]; then
            echo ">>> Container is healthy."
            return 0
        fi
        sleep "${COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS}"
        elapsed=$(( elapsed + COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS ))
    done
    echo ">>> WARNING: health check timed out after ${timeout}s."
    return 1
}

if [[ "${PREPARE_ONLY}" == "true" ]]; then
    echo ">>> Preparing update by pulling latest image for ${APP_SERVICE}..."
    compose_pull_app
    echo ">>> Done."
    echo ">>> Note: image is downloaded. Recreate ${APP_SERVICE} to apply the update."
    exit 0
fi

if [[ "${SKIP_BACKUP}" == "true" ]]; then
    echo ">>> WARNING: skipping the pre-update PostgreSQL backup by explicit request." >&2
elif ! grep -Fqx -- "${BACKUP_SERVICE}" <<< "${services}"; then
    die "backup service '${BACKUP_SERVICE}' not found; provide --backup-service or explicitly pass --skip-backup"
else
    create_pre_update_backup
fi

if [[ "${NO_PULL}" != "true" ]]; then
    echo ">>> Pulling latest image for ${APP_SERVICE}..."
    compose_pull_app || die "failed to pull the target app image; update was not attempted"
fi

echo ">>> Recreating ${APP_SERVICE}..."
if compose_supports_wait; then
    compose_up_app true \
        || { rollback_app || true; die "updated app failed to become healthy; inspect the container and backup before retrying"; }
else
    echo ">>> Compose does not support --wait; using explicit health polling..."
    compose_up_app false \
        || { rollback_app || true; die "updated app failed to start; inspect the container and backup before retrying"; }
    wait_healthy \
        || { rollback_app || true; die "updated app failed to become healthy; inspect the container and backup before retrying"; }
fi

echo ">>> Current services:"
compose ps

echo ">>> Done."

if [[ "${SHOW_LOGS}" == "true" ]]; then
    compose logs -f "${APP_SERVICE}"
fi
