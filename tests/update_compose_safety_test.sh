#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
TEST_ROOT="$(mktemp -d)"
TEST_ROOT="$(cd "${TEST_ROOT}" && pwd -P)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT
OLD_IMAGE="sha256:$(printf '%064d' 1)"

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

make_fixture() {
    FIXTURE="${TEST_ROOT}/$1"
    mkdir -p "${FIXTURE}/bin" "${FIXTURE}/compose with spaces"
    printf 'services:\n  app:\n    image: example.invalid/aether:0.8.0\n    environment:\n      BASE_MARKER: preserved\n' >"${FIXTURE}/compose with spaces/docker-compose.yml"
    printf 'services:\n  app:\n    image: example.invalid/aether:0.9.0\n    environment:\n      OPERATOR_MARKER: preserved\n    pull_policy: always\n' >"${FIXTURE}/compose with spaces/operator override.yml"
    : >"${FIXTURE}/calls"
    cat >"${FIXTURE}/bin/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%q ' "$@" >>"${AETHER_TEST_FIXTURE}/calls"
printf '\n' >>"${AETHER_TEST_FIXTURE}/calls"
fail() { echo "unexpected Docker invocation: $*" >&2; exit 91; }
phase=old
[[ ! -f "${AETHER_TEST_FIXTURE}/phase" ]] || phase="$(cat "${AETHER_TEST_FIXTURE}/phase")"
case "$1" in
    info) exit 0 ;;
    inspect)
        case "$2" in
            '--format={{.Image}}')
                case "${AETHER_TEST_MODE}" in
                    old-image-inspect-failure) exit 44 ;;
                    old-image-empty) exit 0 ;;
                    *) printf '%s\n' "${AETHER_TEST_OLD_IMAGE}" ;;
                esac
                ;;
            '--format={{.State.Status}} {{if .State.Health}}{{.State.Health.Status}}{{else}}missing{{end}}')
                case "${AETHER_TEST_MODE}:${phase}" in
                    no-health:new) printf 'running missing\n' ;;
                    health-failure:new|legacy-unhealthy:new|rollback-unhealthy:rollback) printf 'running unhealthy\n' ;;
                    inspect-failure:new) exit 1 ;;
                    *) printf 'running healthy\n' ;;
                esac
                ;;
            *) fail "$@" ;;
        esac
        exit 0
        ;;
    compose) shift ;;
    *) fail "$@" ;;
esac
[[ "$1" != version ]] || exit 0
[[ "$1" == --project-directory && "$2" == "${AETHER_TEST_FIXTURE}/compose with spaces" ]] || fail "$@"
shift 2
files=()
while [[ "$1" == -f ]]; do
    [[ -f "$2" ]] || fail "missing compose file $2"
    files+=("$2")
    shift 2
done
[[ "${files[0]}" == "${AETHER_TEST_FIXTURE}/compose with spaces/docker-compose.yml" ]] || fail 'lost first Compose file'
if [[ "${AETHER_TEST_MODE}" == multi-file ]]; then
    [[ "${files[1]}" == "${AETHER_TEST_FIXTURE}/compose with spaces/operator override.yml" ]] || fail 'lost second Compose file'
fi
last_file="${files[${#files[@]}-1]}"
rollback=false
[[ "${last_file}" != */.aether-rollback.* ]] || rollback=true
case "$1" in
    config)
        if [[ "${2:-}" == --services ]]; then
            [[ "${AETHER_TEST_MODE}" == missing-backup ]] || printf 'postgres\n'
            printf 'app\n'
        elif [[ $# == 1 ]]; then
            printf 'services:\n  app:\n    environment:\n      image: not-the-container-image\n    image: %s\n' "${AETHER_TEST_IMAGE}"
            case "${AETHER_TEST_MODE}" in
                multi-file|legacy-policy) printf '    pull_policy: always\n' ;;
            esac
        else
            fail "$@"
        fi
        ;;
    ps)
        [[ $# != 1 ]] || exit 0
        [[ "$*" == 'ps -q app' ]] || fail "$@"
        if [[ "${AETHER_TEST_MODE}" == multiple-containers ]]; then
            printf 'container-1\ncontainer-2\n'
            exit 0
        fi
        printf '%s-container\n' "${phase}"
        ;;
    pull)
        [[ "$*" == 'pull app' ]] || fail "$@"
        [[ "${AETHER_TEST_MODE}" != pull-failure ]] || exit 41
        # Simulate an operator-approved mutable tag now pointing at a new image.
        printf 'new\n' >"${AETHER_TEST_FIXTURE}/tag-target"
        ;;
    exec)
        [[ "$2" == -T && "$3" == postgres ]] || fail "$@"
        shift 3
        case "$1" in
            sh)
                [[ "$2" == -c ]] || fail "$@"
                # These are the container's settings, deliberately different
                # from the host's settings. Execute the actual shell script.
                POSTGRES_USER='container user' POSTGRES_DB='container database' \
                    POSTGRES_PASSWORD='fixture-only-secret' PGPASSWORD='' "$@"
                ;;
            pg_restore)
                [[ "$*" == 'pg_restore --list' ]] || fail "$@"
                [[ "$(cat)" == 'PGDMP-fixture' ]] || exit 43
                ;;
            *) fail "$@" ;;
        esac
        ;;
    up)
        if [[ "${2:-}" == --help ]]; then
            case "${AETHER_TEST_MODE}" in
                legacy*) printf 'Usage: docker compose up\n' ;;
                *) printf '%s\n' '--wait --pull' ;;
            esac
            exit 0
        fi
        [[ " $* " == *' --no-deps '* && " $* " == *' --no-build '* ]] || fail 'recreate could change dependencies or build an image'
        [[ "${!#}" == app ]] || fail 'wrong app service'
        if [[ "${rollback}" == true ]]; then
            # Resolve the generated last override, not the APP_IMAGE variable.
            image="$(awk '$1 == "image:" { gsub(/"/, "", $2); print $2 }' "${last_file}")"
            [[ "${image}" == "${AETHER_TEST_OLD_IMAGE}" ]] || fail 'rollback used a mutable tag or the wrong image'
            case "${AETHER_TEST_MODE}" in
                multi-file|legacy-policy)
                    grep -Fq '    pull_policy: never' "${last_file}" || fail 'rollback inherited pull_policy always'
                    ;;
            esac
            case "${AETHER_TEST_MODE}" in
                legacy*) ;;
                *) [[ " $* " == *' --pull never '* ]] || fail 'rollback may pull the image again' ;;
            esac
            printf '%s\n' "${image}" >"${AETHER_TEST_FIXTURE}/restored-image"
            cp "${last_file}" "${AETHER_TEST_FIXTURE}/rollback.yml"
            printf 'rollback\n' >"${AETHER_TEST_FIXTURE}/phase"
        else
            printf 'new\n' >"${AETHER_TEST_FIXTURE}/phase"
            case "${AETHER_TEST_MODE}" in
                wait-failure|multi-file|legacy-policy|rollback-unhealthy) exit 42 ;;
            esac
        fi
        ;;
    *) fail "$@" ;;
esac
EOF
    cat >"${FIXTURE}/bin/pg_dump" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == '--format=custom --no-owner --no-acl --no-password -U container user -d container database' ]]
[[ "$5" == -U && "$6" == 'container user' && "$7" == -d && "$8" == 'container database' ]]
[[ "${PGPASSWORD}" == fixture-only-secret ]]
case "${AETHER_TEST_MODE}" in
    backup-failure) printf 'partial'; exit 42 ;;
    backup-empty) exit 0 ;;
    backup-invalid) printf 'not a PostgreSQL archive\n' ;;
    *) printf 'PGDMP-fixture\n' ;;
esac
EOF
    cat >"${FIXTURE}/bin/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    cat >"${FIXTURE}/bin/date" <<'EOF'
#!/usr/bin/env bash
printf '20260913T000000Z\n'
EOF
    chmod 0755 "${FIXTURE}/bin/docker" "${FIXTURE}/bin/pg_dump" "${FIXTURE}/bin/sleep" "${FIXTURE}/bin/date"
}

run_update() {
    local mode="$1"
    shift
    PATH="${FIXTURE}/bin:${PATH}" AETHER_TEST_FIXTURE="${FIXTURE}" \
        AETHER_TEST_MODE="${mode}" AETHER_TEST_IMAGE="${TEST_IMAGE:-example.invalid/aether:0.8.0}" \
        AETHER_TEST_OLD_IMAGE="${OLD_IMAGE}" POSTGRES_USER=wrong-host-user POSTGRES_DB=wrong-host-db \
        bash "${REPO_ROOT}/update.sh" --compose-dir "${FIXTURE}/compose with spaces" "$@" \
        >"${FIXTURE}/stdout" 2>"${FIXTURE}/stderr"
}

assert_failed_update() {
    if run_update "$@"; then
        fail_test "update unexpectedly succeeded: $*"
    fi
    ! grep -Fq '>>> Done.' "${FIXTURE}/stdout" || fail_test 'failed update was reported as successful'
}

test_image_selection() {
    local image
    for image in example.invalid/app:latest example.invalid/app:nightly example.invalid/app localhost:5000/app; do
        make_fixture "image-${image//[^a-zA-Z0-9]/_}"
        TEST_IMAGE="${image}"
        assert_failed_update success --skip-backup
        grep -Fq 'floating tag' "${FIXTURE}/stderr" || fail_test "not rejected as floating: ${image}"
        ! grep -Eq ' (pull|up) ' "${FIXTURE}/calls" || fail_test 'rejected image was pulled or recreated'
    done
    make_fixture digest-image
    TEST_IMAGE="example.invalid/app@${OLD_IMAGE}"
    run_update success --skip-backup || fail_test 'digest image was rejected'
    TEST_IMAGE='example.invalid/app:latest'
    run_update success --skip-backup --allow-floating-tag || fail_test 'explicit floating opt-in was rejected'
    unset TEST_IMAGE
}

test_backup_safety() {
    local mode
    for mode in backup-failure backup-empty backup-invalid missing-backup; do
        make_fixture "${mode}"
        assert_failed_update "${mode}"
        ! grep -Eq ' (pull|up) ' "${FIXTURE}/calls" || fail_test "${mode} reached pull/recreate"
        [[ "$(find "${FIXTURE}" -name postgres.dump -type f | wc -l | tr -d ' ')" == 0 ]] || fail_test "${mode} left a partial archive"
    done
    make_fixture backup-order-and-privacy
    (umask 022; run_update success) || fail_test 'successful backup failed'
    local dump bundle backup_line validate_line recreate_line
    dump="$(find "${FIXTURE}" -name postgres.dump -type f)"
    bundle="$(dirname "${dump}")"
    [[ "$(ls -ld "${bundle}" | cut -c1-10)" == drwx------ ]] || fail_test 'backup bundle is not private'
    [[ "$(ls -l "${dump}" | cut -c1-10)" == -rw------- ]] || fail_test 'dump is not private'
    [[ "$(ls -l "${bundle}/metadata" | cut -c1-10)" == -rw------- ]] || fail_test 'metadata is not private'
    grep -Fq "previous_image_id=${OLD_IMAGE}" "${bundle}/metadata" || fail_test 'metadata omitted previous immutable image'
    grep -Fq 'target_image=example.invalid/aether:0.8.0' "${bundle}/metadata" || fail_test 'metadata omitted target image'
    backup_line="$(grep -n 'exec -T postgres sh' "${FIXTURE}/calls" | cut -d: -f1)"
    validate_line="$(grep -n 'exec -T postgres pg_restore' "${FIXTURE}/calls" | cut -d: -f1)"
    recreate_line="$(grep -n ' up -d ' "${FIXTURE}/calls" | cut -d: -f1)"
    [[ ${backup_line} -lt ${validate_line} && ${validate_line} -lt ${recreate_line} ]] || fail_test 'archive validation did not precede recreation'
    ! grep -Rq 'fixture-only-secret' "${FIXTURE}/compose with spaces/backups" || fail_test 'metadata exposed database password'
    run_update success || fail_test 'second backup failed'
    [[ "$(find "${FIXTURE}" -name postgres.dump -type f | wc -l | tr -d ' ')" == 2 ]] || fail_test 'same-second update overwrote a backup'
}

test_rollback_and_health() {
    local mode
    make_fixture migration-boundary
    assert_failed_update wait-failure --skip-backup
    [[ ! -f "${FIXTURE}/restored-image" ]] || fail_test 'old app was restarted without migration compatibility confirmation'
    grep -Fq -- '--rollback-compatible' "${FIXTURE}/stderr" || fail_test 'missing migration compatibility guidance'
    for mode in wait-failure multi-file no-health health-failure inspect-failure legacy-unhealthy legacy-policy rollback-unhealthy; do
        make_fixture "${mode}"
        local args=(--skip-backup --rollback-compatible)
        if [[ "${mode}" == multi-file ]]; then
            args+=(-f docker-compose.yml -f 'operator override.yml')
        fi
        assert_failed_update "${mode}" "${args[@]}"
        [[ "$(cat "${FIXTURE}/restored-image")" == "${OLD_IMAGE}" ]] || fail_test "${mode} failed to restore immutable old image"
        if [[ "${mode}" == rollback-unhealthy ]]; then
            grep -Fq 'automatic app rollback failed' "${FIXTURE}/stderr" || fail_test 'unhealthy rollback was accepted'
        else
            grep -Fq 'Previous app image restored and healthy' "${FIXTURE}/stdout" || fail_test "${mode} did not verify rollback health"
        fi
        [[ "$(find "${FIXTURE}" -name '.aether-rollback.*' | wc -l | tr -d ' ')" == 0 ]] || fail_test 'rollback overlay was not cleaned up'
    done
    make_fixture legacy-success
    run_update legacy --skip-backup || fail_test 'legacy healthy update failed'
    grep -Fq 'Container is healthy' "${FIXTURE}/stdout" || fail_test 'legacy path omitted health verification'
}

test_prepare_pull_failure_and_arguments() {
    make_fixture prepare
    run_update success --prepare || fail_test 'prepare failed'
    ! grep -Eq ' (exec|up) ' "${FIXTURE}/calls" || fail_test 'prepare backed up or recreated containers'
    make_fixture pull-failure
    assert_failed_update pull-failure --skip-backup --rollback-compatible
    ! grep -Eq ' up ' "${FIXTURE}/calls" || fail_test 'pull failure recreated app'
    make_fixture unsafe-service
    assert_failed_update success --service --ansi
    grep -Fq 'service name contains unsafe characters' "${FIXTURE}/stderr" || fail_test 'unsafe service name was not rejected'
    make_fixture replicas
    assert_failed_update multiple-containers --skip-backup
    grep -Fq 'requires a single app container' "${FIXTURE}/stderr" || fail_test 'replicas were silently reduced to one container'
    ! grep -Eq ' (pull|up) ' "${FIXTURE}/calls" || fail_test 'multi-replica update changed deployment'
    local mode
    for mode in old-image-inspect-failure old-image-empty; do
        make_fixture "${mode}"
        assert_failed_update "${mode}" --skip-backup --rollback-compatible
        ! grep -Eq ' (pull|up) ' "${FIXTURE}/calls" || fail_test 'unresolved old image allowed an update'
    done
}

test_real_compose_override_merge() {
    # This command does not need a Docker daemon. GitHub's shell-fixture job
    # provides Compose; local systems without the CLI report the missing check.
    if ! docker compose version >/dev/null 2>&1; then
        echo 'SKIP: real Compose config merge (Docker Compose CLI unavailable)'
        [[ "${CI:-false}" != true ]] || fail_test 'CI must validate the generated override using real Compose'
        return
    fi
    local compose_fixture="${TEST_ROOT}/multi-file"
    docker compose --project-directory "${compose_fixture}/compose with spaces" \
        -f "${compose_fixture}/compose with spaces/docker-compose.yml" \
        -f "${compose_fixture}/compose with spaces/operator override.yml" \
        -f "${compose_fixture}/rollback.yml" config >"${compose_fixture}/merged.yml"
    [[ "$(awk '$1 == "image:" { print $2 }' "${compose_fixture}/merged.yml")" == "${OLD_IMAGE}" ]] || fail_test 'real Compose did not select the previous immutable image'
    grep -Fq 'BASE_MARKER: preserved' "${compose_fixture}/merged.yml" || fail_test 'real Compose lost base environment'
    grep -Fq 'OPERATOR_MARKER: preserved' "${compose_fixture}/merged.yml" || fail_test 'real Compose lost operator environment'
    grep -Fq 'pull_policy: never' "${compose_fixture}/merged.yml" || fail_test 'real Compose kept the operator pull policy'
}

test_image_selection
test_backup_safety
test_rollback_and_health
test_prepare_pull_failure_and_arguments
test_real_compose_override_merge
echo 'PASS: compose updater backup, rollback, health, selection and argument safety fixtures'
