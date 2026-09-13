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
ROLLBACK_COMPATIBLE=false
BACKUP_STAGING_DIR=""
ROLLBACK_OVERRIDE=""
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
  --rollback-compatible   allow automatic app rollback after verifying that the
                          target database migrations support the previous app
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
        --rollback-compatible)
            ROLLBACK_COMPATIBLE=true
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

compose_configuration="$(compose_config)"
configured_app_image="$(awk -v service="${APP_SERVICE}" '
    $0 == "  " service ":" { in_service = 1; next }
    in_service && $0 ~ /^  [^[:space:]]/ { exit }
    in_service && $0 ~ /^    image:/ { print $2; exit }
' <<<"${compose_configuration}")"
configured_app_pull_policy="$(awk -v service="${APP_SERVICE}" '
    $0 == "  " service ":" { in_service = 1; next }
    in_service && $0 ~ /^  [^[:space:]]/ { exit }
    in_service && $0 ~ /^    pull_policy:/ { print $2; exit }
' <<<"${compose_configuration}")"
unset compose_configuration
configured_app_image="${configured_app_image%\"}"
configured_app_image="${configured_app_image#\"}"
configured_app_image="${configured_app_image%\'}"
configured_app_image="${configured_app_image#\'}"
[[ -n "${configured_app_image}" ]] || die "app service must configure an image; use local-build mode for source builds"

is_floating_image_tag() {
    local image_ref="$1"
    [[ "${image_ref}" != *@* ]] || return 1
    local image_name="${image_ref##*/}"
    [[ "${image_name}" == *:* ]] || return 0
    local image_tag="${image_name##*:}"
    [[ "${image_tag}" == "latest" || "${image_tag}" == "nightly" ]]
}

if [[ -n "${configured_app_image}" ]] \
    && is_floating_image_tag "${configured_app_image}" \
    && [[ "${ALLOW_FLOATING_TAG}" != "true" ]]; then
    die "app image '${configured_app_image}' is a floating tag; set APP_IMAGE to an explicit version or pass --allow-floating-tag"
fi

previous_container_id="$(compose ps -q "${APP_SERVICE}" 2>/dev/null)" \
    || die "could not inspect the existing app service; update was not attempted"
[[ "${previous_container_id}" != *$'\n'* ]] \
    || die "this updater requires a single app container; use the multi-node rollout procedure for replicas"
previous_image_id=""
if [[ -n "${previous_container_id}" ]]; then
    previous_image_id="$(docker inspect --format='{{.Image}}' "${previous_container_id}" 2>/dev/null)" \
        || die "could not inspect the previous app image; update was not attempted"
    [[ "${previous_image_id}" =~ ^sha256:[a-f0-9]{64}$ ]] \
        || die "could not resolve the previous container to an immutable image ID"
fi

echo ">>> Compose directory: ${COMPOSE_DIR}"
echo ">>> App service: ${APP_SERVICE}"
if [[ -n "${configured_app_image}" ]]; then
    echo ">>> Target app image: ${configured_app_image}"
fi

cleanup_update_files() {
    if [[ -n "${BACKUP_STAGING_DIR}" ]]; then
        rm -f -- "${BACKUP_STAGING_DIR}/postgres.dump" "${BACKUP_STAGING_DIR}/metadata"
        rmdir -- "${BACKUP_STAGING_DIR}" || true
    fi
    if [[ -n "${ROLLBACK_OVERRIDE}" ]]; then
        rm -f -- "${ROLLBACK_OVERRIDE}"
    fi
}
trap cleanup_update_files EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

create_pre_update_backup() {
    local timestamp backup_bundle temp_file
    # The dump contains credentials and personal data. Never inherit a public
    # umask, and publish the dump plus metadata as one private, unique bundle.
    umask 077
    mkdir -p -- "${BACKUP_DIR}"
    timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
    BACKUP_STAGING_DIR="$(mktemp -d "${BACKUP_DIR}/.aether-postgres-${timestamp}.XXXXXX")"
    backup_bundle="${BACKUP_DIR}/$(basename -- "${BACKUP_STAGING_DIR}" | sed 's/^\.//')"
    temp_file="${BACKUP_STAGING_DIR}/postgres.dump"
    echo ">>> Creating PostgreSQL pre-update backup: ${backup_bundle}/postgres.dump"
    # Expand database settings inside the service. Host environment variables
    # can differ from Compose's .env and from the running database container.
    if ! compose exec -T "${BACKUP_SERVICE}" sh -c '
        export PGPASSWORD="${PGPASSWORD:-${POSTGRES_PASSWORD:-}}"
        exec pg_dump --format=custom --no-owner --no-acl --no-password \
            -U "${POSTGRES_USER:-postgres}" -d "${POSTGRES_DB:-aether}"
    ' \
        >"${temp_file}"; then
        die "pre-update PostgreSQL backup failed; update was not attempted"
    fi
    if [[ ! -s "${temp_file}" ]]; then
        die "pre-update PostgreSQL backup was empty; update was not attempted"
    fi
    if ! compose exec -T "${BACKUP_SERVICE}" pg_restore --list <"${temp_file}" >/dev/null; then
        die "pre-update PostgreSQL backup archive is invalid; update was not attempted"
    fi
    printf 'created_at=%s\nprevious_image_id=%s\ntarget_image=%s\n' \
        "${timestamp}" "${previous_image_id:-unavailable}" "${configured_app_image}" \
        >"${BACKUP_STAGING_DIR}/metadata"
    mv -- "${BACKUP_STAGING_DIR}" "${backup_bundle}"
    BACKUP_STAGING_DIR=""
    echo ">>> PostgreSQL backup completed: ${backup_bundle}"
}

rollback_app() {
    if [[ "${ROLLBACK_COMPATIBLE}" != "true" ]]; then
        echo ">>> WARNING: automatic rollback requires --rollback-compatible after checking migration compatibility. Previous image: ${previous_image_id:-unavailable}. Preserve the backup and inspect the database before restoring an app image." >&2
        return 1
    fi
    if [[ -z "${previous_image_id}" ]]; then
        echo ">>> WARNING: previous app image is unavailable; automatic rollback skipped." >&2
        return 1
    fi
    echo ">>> Updated app failed health verification; restoring ${previous_image_id}..."
    ROLLBACK_OVERRIDE="$(mktemp "${COMPOSE_DIR}/.aether-rollback.XXXXXX")" || return 1
    printf 'services:\n  %s:\n    image: "%s"\n' "${APP_SERVICE}" "${previous_image_id}" >"${ROLLBACK_OVERRIDE}" || return 1
    if [[ -n "${configured_app_pull_policy}" ]]; then
        # Early Compose v2 supports pull_policy but not the up --pull flag.
        # Only emit the field when the original model already uses it, keeping
        # compatibility with Compose v1 versions that reject this field.
        printf '    pull_policy: never\n' >>"${ROLLBACK_OVERRIDE}" || return 1
    fi
    (
        # The last file overrides even a hard-coded image in an operator's
        # additional Compose file. Never resolve a mutable old tag after pull.
        COMPOSE_ARGS+=(-f "${ROLLBACK_OVERRIDE}")
        if compose_supports_wait; then
            compose_up_app true || return 1
        else
            compose_up_app false || return 1
        fi
        wait_healthy || return 1
    ) || {
        echo ">>> WARNING: automatic app rollback failed; inspect Compose state and migration compatibility before choosing a database restore. A restore discards writes made after the backup." >&2
        return 1
    }
    echo ">>> Previous app image restored and healthy. Database migrations are not automatically reversed."
}

compose_pull_app() {
    compose pull "${APP_SERVICE}"
}

compose_up_app() {
    local wait_for_health="${1:-false}"
    local -a up_args=(up -d --no-deps --no-build)

    local help
    help="$(compose up --help)" || return 1
    if grep -Fq -- '--pull' <<<"${help}"; then
        # Pulling is an explicit earlier step. Do not let inherited
        # pull_policy: always change the image during up or rollback.
        up_args+=(--pull never)
    fi

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
        container_id="$(compose ps -q "${APP_SERVICE}" 2>/dev/null)" || return 1
        if [[ "${container_id}" == *$'\n'* ]]; then
            echo ">>> WARNING: expected a single app container; cannot verify a multi-replica rollout." >&2
            return 1
        fi
        if [[ -z "${container_id}" ]]; then
            sleep "${COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS}"
            elapsed=$(( elapsed + COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS ))
            continue
        fi
        state="$(docker inspect --format='{{.State.Status}} {{if .State.Health}}{{.State.Health.Status}}{{else}}missing{{end}}' \
            "${container_id}" 2>/dev/null || true)"
        if [[ "${state}" == "running healthy" ]]; then
            echo ">>> Container is healthy."
            return 0
        fi
        if [[ "${state}" == *" missing" ]]; then
            echo ">>> WARNING: app container has no healthcheck; cannot verify update health." >&2
            return 1
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
fi
# Compose --wait accepts running containers without healthchecks. Require the
# app's own healthcheck on modern and legacy Compose alike.
wait_healthy \
    || { rollback_app || true; die "updated app failed to become healthy; inspect the container and backup before retrying"; }

echo ">>> Current services:"
compose ps

echo ">>> Done."

if [[ "${SHOW_LOGS}" == "true" ]]; then
    compose logs -f "${APP_SERVICE}"
fi
