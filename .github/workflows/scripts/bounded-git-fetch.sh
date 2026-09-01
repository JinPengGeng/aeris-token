#!/usr/bin/env bash

# Shared, fail-closed Git transport for upstream synchronization. Callers must
# source this file, validate the protected policy, and fetch only exact refs.

AERIS_FETCH_TIMEOUT_SECONDS=90
AERIS_FETCH_MAX_REMOTE_REF_BYTES=4096
AERIS_FETCH_MAX_RECEIVED_BYTES=268435456
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=1073741824
AERIS_FETCH_MAX_RECEIVED_OBJECTS=250000
AERIS_FETCH_MAX_IMPORT_BYTES=268435456
AERIS_FETCH_MAX_IMPORT_OBJECTS=250000
AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES=536870912
AERIS_FETCH_MAX_OBJECT_BYTES=33554432
AERIS_FETCH_MAX_BLOB_BYTES=33554432
AERIS_FETCH_MAX_CHANGED_BLOB_BYTES=268435456
AERIS_FETCH_MAX_CHANGED_PATHS=20000
AERIS_FETCH_MAX_TREE_ENTRIES=500000
AERIS_FETCH_MAX_DIFF_BYTES=33554432

AERIS_FETCH_RECEIVED_BYTES_TOTAL=0
AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL=0
AERIS_FETCH_RECEIVED_OBJECTS_TOTAL=0
AERIS_FETCH_IMPORT_BYTES_TOTAL=0
AERIS_FETCH_IMPORT_OBJECTS_TOTAL=0
AERIS_BOUNDED_FETCHED_SHA=''
AERIS_BOUNDED_REMOTE_SHA=''
AERIS_BOUNDED_LAST_RECEIVED_BYTES=0
AERIS_BOUNDED_LAST_RECEIVED_EXPANDED_BYTES=0
AERIS_BOUNDED_LAST_RECEIVED_OBJECTS=0
AERIS_BOUNDED_LAST_IMPORT_BYTES=0
AERIS_BOUNDED_LAST_IMPORT_OBJECTS=0

# Object reads in every caller and child process must fail instead of consulting
# a promisor remote. Repository/environment checks below reject other stores.
export GIT_NO_LAZY_FETCH=1

aeris_bounded_fetch_error() {
  printf 'error: bounded Git fetch: %s\n' "$1" >&2
  return 1
}

aeris_bounded_policy_value() {
  local policy="$1" key="$2"
  awk -v key="${key}" '
    /^resource_bounds:[[:space:]]*$/ { in_bounds = 1; next }
    in_bounds && /^[^[:space:]#]/ { exit }
    in_bounds && $0 ~ "^[[:space:]]{2}" key ":[[:space:]]*[0-9]+[[:space:]]*$" {
      value = $0
      sub("^[[:space:]]{2}" key ":[[:space:]]*", "", value)
      sub("[[:space:]]*$", "", value)
      print value
      exit
    }
  ' "${policy}"
}

aeris_bounded_fetch_assert_policy() {
  local policy="$1" key expected actual
  if [[ ! -f "${policy}" ]]; then
    aeris_bounded_fetch_error "resource-bound policy is unavailable: ${policy}"
    return
  fi
  while read -r key expected; do
    actual="$(aeris_bounded_policy_value "${policy}" "${key}")"
    if [[ "${actual}" != "${expected}" ]]; then
      aeris_bounded_fetch_error "protected policy ${key} must equal ${expected}"
      return
    fi
  done <<EOF
fetch_timeout_seconds ${AERIS_FETCH_TIMEOUT_SECONDS}
max_received_bytes ${AERIS_FETCH_MAX_RECEIVED_BYTES}
max_received_expanded_bytes ${AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES}
max_received_objects ${AERIS_FETCH_MAX_RECEIVED_OBJECTS}
max_import_bytes ${AERIS_FETCH_MAX_IMPORT_BYTES}
max_import_objects ${AERIS_FETCH_MAX_IMPORT_OBJECTS}
max_process_memory_bytes ${AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES}
max_object_bytes ${AERIS_FETCH_MAX_OBJECT_BYTES}
max_blob_bytes ${AERIS_FETCH_MAX_BLOB_BYTES}
max_changed_blob_bytes ${AERIS_FETCH_MAX_CHANGED_BLOB_BYTES}
max_changed_paths ${AERIS_FETCH_MAX_CHANGED_PATHS}
max_tree_entries ${AERIS_FETCH_MAX_TREE_ENTRIES}
max_diff_bytes ${AERIS_FETCH_MAX_DIFF_BYTES}
EOF
}

aeris_bounded_normalize_directory() {
  local path="$1"
  if [[ "${path}" =~ ^[A-Za-z]:[/\\] ]] && command -v cygpath >/dev/null 2>&1; then
    path="$(cygpath -u "${path}")" || return 1
  fi
  [[ "${path}" == /* ]] || path="$(pwd -P)/${path}"
  (cd "${path}" 2>/dev/null && pwd -P)
}

aeris_bounded_fetch_init() {
  local policy="$1" shallow shallow_file shallow_head partial alternates object_root
  local git_dir common_dir_raw common_dir worktree_count
  if [[ -v GIT_OBJECT_DIRECTORY || -v GIT_ALTERNATE_OBJECT_DIRECTORIES || -v GIT_COMMON_DIR ]]; then
    aeris_bounded_fetch_error 'object-directory environment overrides are forbidden'
    return
  fi
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true ]]; then
    if [[ "${AERIS_BOUNDED_FETCH_TEST_FIXTURE:-false}" != true ]]; then
      aeris_bounded_fetch_error 'test-mode resource overrides require the explicit fixture fence'
      return
    fi
    AERIS_FETCH_TIMEOUT_SECONDS="${AERIS_TEST_FETCH_TIMEOUT_SECONDS:-${AERIS_FETCH_TIMEOUT_SECONDS}}"
    AERIS_FETCH_MAX_RECEIVED_BYTES="${AERIS_TEST_FETCH_MAX_RECEIVED_BYTES:-${AERIS_FETCH_MAX_RECEIVED_BYTES}}"
    AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES="${AERIS_TEST_FETCH_MAX_RECEIVED_EXPANDED_BYTES:-${AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES}}"
    AERIS_FETCH_MAX_RECEIVED_OBJECTS="${AERIS_TEST_FETCH_MAX_RECEIVED_OBJECTS:-${AERIS_FETCH_MAX_RECEIVED_OBJECTS}}"
    AERIS_FETCH_MAX_IMPORT_BYTES="${AERIS_TEST_FETCH_MAX_IMPORT_BYTES:-${AERIS_FETCH_MAX_IMPORT_BYTES}}"
    AERIS_FETCH_MAX_IMPORT_OBJECTS="${AERIS_TEST_FETCH_MAX_IMPORT_OBJECTS:-${AERIS_FETCH_MAX_IMPORT_OBJECTS}}"
    AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES="${AERIS_TEST_FETCH_MAX_PROCESS_MEMORY_BYTES:-${AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES}}"
    AERIS_FETCH_MAX_OBJECT_BYTES="${AERIS_TEST_FETCH_MAX_OBJECT_BYTES:-${AERIS_FETCH_MAX_OBJECT_BYTES}}"
    AERIS_FETCH_MAX_BLOB_BYTES="${AERIS_TEST_FETCH_MAX_BLOB_BYTES:-${AERIS_FETCH_MAX_BLOB_BYTES}}"
    AERIS_FETCH_MAX_CHANGED_BLOB_BYTES="${AERIS_TEST_FETCH_MAX_CHANGED_BLOB_BYTES:-${AERIS_FETCH_MAX_CHANGED_BLOB_BYTES}}"
    AERIS_FETCH_MAX_CHANGED_PATHS="${AERIS_TEST_FETCH_MAX_CHANGED_PATHS:-${AERIS_FETCH_MAX_CHANGED_PATHS}}"
    AERIS_FETCH_MAX_TREE_ENTRIES="${AERIS_TEST_FETCH_MAX_TREE_ENTRIES:-${AERIS_FETCH_MAX_TREE_ENTRIES}}"
    AERIS_FETCH_MAX_DIFF_BYTES="${AERIS_TEST_FETCH_MAX_DIFF_BYTES:-${AERIS_FETCH_MAX_DIFF_BYTES}}"
  else
    aeris_bounded_fetch_assert_policy "${policy}" || return
  fi
  for value in \
    "${AERIS_FETCH_TIMEOUT_SECONDS}" \
    "${AERIS_FETCH_MAX_RECEIVED_BYTES}" \
    "${AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES}" \
    "${AERIS_FETCH_MAX_RECEIVED_OBJECTS}" \
    "${AERIS_FETCH_MAX_IMPORT_BYTES}" \
    "${AERIS_FETCH_MAX_IMPORT_OBJECTS}" \
    "${AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES}" \
    "${AERIS_FETCH_MAX_OBJECT_BYTES}" \
    "${AERIS_FETCH_MAX_BLOB_BYTES}" \
    "${AERIS_FETCH_MAX_CHANGED_BLOB_BYTES}" \
    "${AERIS_FETCH_MAX_CHANGED_PATHS}" \
    "${AERIS_FETCH_MAX_TREE_ENTRIES}" \
    "${AERIS_FETCH_MAX_DIFF_BYTES}"; do
    if [[ ! "${value}" =~ ^[1-9][0-9]*$ ]]; then
      aeris_bounded_fetch_error 'every resource bound must be a positive integer'
      return
    fi
  done
  git_dir="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --absolute-git-dir 2>/dev/null)" || {
    aeris_bounded_fetch_error 'current Git directory is unavailable'
    return
  }
  common_dir_raw="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --git-common-dir 2>/dev/null)" || {
    aeris_bounded_fetch_error 'current Git common directory is unavailable'
    return
  }
  git_dir="$(aeris_bounded_normalize_directory "${git_dir}")" || {
    aeris_bounded_fetch_error 'current Git directory cannot be normalized'
    return
  }
  common_dir="$(aeris_bounded_normalize_directory "${common_dir_raw}")" || {
    aeris_bounded_fetch_error 'current Git common directory cannot be normalized'
    return
  }
  if [[ "${git_dir}" != "${common_dir}" ]]; then
    aeris_bounded_fetch_error 'linked worktrees and shared Git common directories are forbidden'
    return
  fi
  worktree_count="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git worktree list --porcelain 2>/dev/null |
    awk '/^worktree / { count += 1 } END { print count + 0 }')" || {
    aeris_bounded_fetch_error 'unable to enumerate Git worktrees'
    return
  }
  if [[ ! "${worktree_count}" =~ ^[0-9]+$ || "${worktree_count}" -ne 1 ]]; then
    aeris_bounded_fetch_error 'repositories with linked worktrees are forbidden'
    return
  fi
  shallow="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --is-shallow-repository 2>/dev/null)" || {
    aeris_bounded_fetch_error 'current Git repository is unavailable'
    return
  }
  if [[ "${shallow}" != false ]]; then
    if [[ "${AERIS_BOUNDED_BOOTSTRAP_SHALLOW:-false}" != true ]]; then
      aeris_bounded_fetch_error 'shallow repositories cannot prove checkpoint ancestry'
      return
    fi
    shallow_file="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --git-path shallow)" || return 1
    shallow_head="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --verify HEAD^{commit} 2>/dev/null)" || return 1
    if [[ "$(wc -l <"${shallow_file}")" -ne 1 || "$(tr -d '\r\n' <"${shallow_file}")" != "${shallow_head}" ]]; then
      aeris_bounded_fetch_error 'bootstrap checkout must contain exactly one shallow boundary at HEAD'
      return
    fi
    object_root="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --git-path objects)" || return 1
    if [[ -L "${object_root}" ]] || find "${object_root}" -type l -print -quit | grep -q . ||
       [[ -e "${object_root}/info/alternates" ]]; then
      aeris_bounded_fetch_error 'bootstrap checkout uses a forbidden object-store indirection'
      return
    fi
    # Do not let the one-commit bootstrap advertise its incomplete tip while
    # importing the independently validated full graph. The checked-out files
    # remain available for the trusted helper; every object is reacquired by
    # the bounded exact-ref receiver before history is inspected.
    while IFS= read -r bootstrap_ref; do
      aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
        git update-ref -d "${bootstrap_ref}" || return 1
    done < <(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git for-each-ref --format='%(refname)')
    find "${object_root}" -type f \
      \( -path '*/pack/pack-*' -o -regex '.*/[0-9a-f][0-9a-f]/[0-9a-f]\{38\}' \) \
      -delete || return 1
    rm -f -- "${shallow_file}" || return 1
    [[ "$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --is-shallow-repository 2>/dev/null)" == false ]] || return 1
  fi
  partial="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git config --get-regexp '^(extensions\.partialClone|remote\..*\.promisor)$' 2>/dev/null || true)"
  if [[ -n "${partial}" ]]; then
    aeris_bounded_fetch_error 'partial-clone repositories could trigger an unbounded lazy fetch'
    return
  fi
  object_root="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --git-path objects)" || {
    aeris_bounded_fetch_error 'current Git object directory is unavailable'
    return
  }
  if [[ -L "${object_root}" ]] || find "${object_root}" -type l -print -quit | grep -q .; then
    aeris_bounded_fetch_error 'symbolic links in the object store are forbidden'
    return
  fi
  alternates="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git config --get core.alternateRefsCommand 2>/dev/null || true)"
  if [[ -n "${alternates}" || -e "${object_root}/info/alternates" ]]; then
    aeris_bounded_fetch_error 'alternate object stores are forbidden'
    return
  fi
  if find "${object_root}/pack" -maxdepth 1 -type f -name '*.promisor' -print -quit 2>/dev/null |
    grep -q .; then
    aeris_bounded_fetch_error 'promisor object packs are forbidden'
    return
  fi
}

aeris_bounded_network_git() {
  local delay="${AERIS_TEST_FETCH_DELAY_SECONDS:-0}"
  local max_file_bytes="${AERIS_BOUNDED_NETWORK_MAX_FILE_BYTES:-${AERIS_FETCH_MAX_RECEIVED_BYTES}}"
  local allow_fixture_config=false transport_home='' status
  if [[ -n "${AERIS_BOUNDED_FETCH_PREFLIGHT:-}" ]]; then
    if ! declare -F "${AERIS_BOUNDED_FETCH_PREFLIGHT}" >/dev/null; then
      aeris_bounded_fetch_error 'configured network preflight is not a shell function'
      return
    fi
    "${AERIS_BOUNDED_FETCH_PREFLIGHT}" || return
  fi
  local -a command_line=(git)
  if [[ "${AERIS_BOUNDED_FETCH_CREDENTIALLESS:-false}" == true ]]; then
    if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
          "${AERIS_BOUNDED_FETCH_TEST_FIXTURE:-false}" == true &&
          "${AERIS_BOUNDED_CREDENTIALLESS_TEST_ALLOW_CONFIG:-false}" == true ]]; then
      allow_fixture_config=true
    fi
    aeris_bounded_assert_credentialless_transport || return
    transport_home="$(mktemp -d "${AERIS_BOUNDED_FETCH_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/aeris-credentialless-home.XXXXXX")" || return 1
    command_line+=(
      -c credential.helper=
      -c http.https://github.com/.extraheader=
      -c http.lowSpeedLimit=1
      -c http.lowSpeedTime=30
    )
  fi
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true && "${delay}" != 0 ]]; then
    if (
      unset GIT_SSH GIT_SSH_COMMAND SSH_ASKPASS SSH_AUTH_SOCK GIT_PROXY_COMMAND \
        GIT_CONFIG_PARAMETERS CURL_HOME
      [[ "${allow_fixture_config}" == true ]] || export GIT_CONFIG_COUNT=0
      export GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=echo GCM_INTERACTIVE=Never
      if [[ -n "${transport_home}" ]]; then
        export HOME="${transport_home}" XDG_CONFIG_HOME="${transport_home}"
      fi
      aeris_bounded_run "${max_file_bytes}" bash -c \
        'sleep "$1"; shift; exec "$@"' bash "${delay}" "${command_line[@]}" "$@"
    ); then status=0; else status=$?; fi
  else
    if (
      unset GIT_SSH GIT_SSH_COMMAND SSH_ASKPASS SSH_AUTH_SOCK GIT_PROXY_COMMAND \
        GIT_CONFIG_PARAMETERS CURL_HOME
      [[ "${allow_fixture_config}" == true ]] || export GIT_CONFIG_COUNT=0
      export GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=echo GCM_INTERACTIVE=Never
      if [[ -n "${transport_home}" ]]; then
        export HOME="${transport_home}" XDG_CONFIG_HOME="${transport_home}"
      fi
      aeris_bounded_run "${max_file_bytes}" "${command_line[@]}" "$@"
    ); then status=0; else status=$?; fi
  fi
  [[ -z "${transport_home}" ]] || rm -rf -- "${transport_home}"
  return "${status}"
}

aeris_bounded_assert_credentialless_transport() {
  local name config_file status line key allow_fixture_config=false
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
        "${AERIS_BOUNDED_FETCH_TEST_FIXTURE:-false}" == true &&
        "${AERIS_BOUNDED_CREDENTIALLESS_TEST_ALLOW_CONFIG:-false}" == true ]]; then
    allow_fixture_config=true
  fi
  for name in GIT_SSH GIT_SSH_COMMAND SSH_ASKPASS GIT_ASKPASS GIT_CONFIG_COUNT \
    GIT_CONFIG_PARAMETERS GIT_CONFIG_SYSTEM GIT_CONFIG_GLOBAL GIT_CONFIG \
    GIT_PROXY_COMMAND GIT_HTTP_PROXY_AUTHMETHOD GIT_SSL_NO_VERIFY GIT_SSL_CAINFO \
    GIT_SSL_CAPATH CURL_HOME HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY \
    http_proxy https_proxy all_proxy no_proxy; do
    if [[ -v "${name}" &&
          !( "${allow_fixture_config}" == true && "${name}" == GIT_CONFIG_COUNT ) ]]; then
      aeris_bounded_fetch_error "credentialless transport forbids inherited ${name}"
      return
    fi
  done
  config_file="$(mktemp "${AERIS_BOUNDED_FETCH_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}/aeris-transport-config.XXXXXX")" || return 1
  set +e
  aeris_bounded_run 1048576 git --no-pager config --show-origin --name-only --get-regexp \
    '^(url\..*\.(insteadof|pushinsteadof)|credential(\..*)?|http\..*|core\.sshcommand|remote\..*\.proxy|include(\..*)?\.path)$' \
    >"${config_file}"
  status=$?
  set -e
  if ((status != 0 && status != 1)); then
    rm -f -- "${config_file}"
    aeris_bounded_fetch_error 'unable to inspect credentialless transport configuration'
    return
  fi
  while IFS= read -r line; do
    key="${line#*$'\t'}"
    # actions/checkout installs this exact header and the command line clears it.
    [[ "${key,,}" == 'http.https://github.com/.extraheader' ]] && continue
    if [[ "${allow_fixture_config}" == true && "${key,,}" == url.file://*.insteadof ]]; then
      continue
    fi
    rm -f -- "${config_file}"
    aeris_bounded_fetch_error "credentialless transport forbids Git config ${key}"
    return
  done <"${config_file}"
  rm -f -- "${config_file}"
}

aeris_bounded_run() {
  local max_file_bytes="$1"
  shift
  local memory_kib file_blocks kernel require_limits=false
  memory_kib=$(((AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES + 1023) / 1024))
  file_blocks=$(((max_file_bytes + 1023) / 1024))
  kernel="$(uname -s 2>/dev/null || true)"
  if [[ "${kernel}" == Linux || "${GITHUB_ACTIONS:-false}" == true ]]; then
    require_limits=true
  fi
  if ! command -v timeout >/dev/null 2>&1; then
    aeris_bounded_fetch_error 'the runner cannot enforce the hard process deadline'
    return 125
  fi
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
        "${AERIS_BOUNDED_FETCH_TEST_FIXTURE:-false}" == true &&
        "${AERIS_BOUNDED_TEST_DISABLE_LIMITS:-false}" == true ]]; then
    if [[ "${require_limits}" == true ]]; then
      aeris_bounded_fetch_error 'the runner cannot enforce process memory and file-size limits'
      return 125
    fi
  elif (
    ulimit -v "${memory_kib}" 2>/dev/null &&
      ulimit -f "${file_blocks}" 2>/dev/null
  ); then
    (
      ulimit -v "${memory_kib}"
      ulimit -f "${file_blocks}"
      exec timeout -k 5s "${AERIS_FETCH_TIMEOUT_SECONDS}s" "$@"
    )
    return
  elif [[ "${require_limits}" == true ]]; then
    aeris_bounded_fetch_error 'the runner cannot enforce process memory and file-size limits'
    return 125
  fi
  # Git for Windows does not implement these POSIX resource limits. Production
  # is Linux-only; local Windows fixtures retain deadline and aggregate checks.
  timeout -k 5s "${AERIS_FETCH_TIMEOUT_SECONDS}s" "$@"
}

# Go-based CLIs such as gh reserve far more than AERIS_FETCH_MAX_PROCESS_MEMORY_BYTES
# of virtual address space at startup, so the ulimit -v ceiling that C tools
# (git, jq, curl) tolerate kills them before main with "failed to reserve page
# summary memory". This runner enforces the same hard deadline and file-size
# bound as aeris_bounded_run but leaves virtual memory uncapped; aggregate
# response-byte checks in the callers still bound the data plane.
aeris_bounded_run_deadline() {
  local max_file_bytes="$1"
  shift
  local file_blocks kernel require_limits=false
  file_blocks=$(((max_file_bytes + 1023) / 1024))
  kernel="$(uname -s 2>/dev/null || true)"
  if [[ "${kernel}" == Linux || "${GITHUB_ACTIONS:-false}" == true ]]; then
    require_limits=true
  fi
  if ! command -v timeout >/dev/null 2>&1; then
    aeris_bounded_fetch_error 'the runner cannot enforce the hard process deadline'
    return 125
  fi
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
        "${AERIS_BOUNDED_FETCH_TEST_FIXTURE:-false}" == true &&
        "${AERIS_BOUNDED_TEST_DISABLE_LIMITS:-false}" == true ]]; then
    if [[ "${require_limits}" == true ]]; then
      aeris_bounded_fetch_error 'the runner cannot enforce the process file-size limit'
      return 125
    fi
  elif (
    ulimit -f "${file_blocks}" 2>/dev/null
  ); then
    (
      ulimit -f "${file_blocks}"
      exec timeout -k 5s "${AERIS_FETCH_TIMEOUT_SECONDS}s" "$@"
    )
    return
  elif [[ "${require_limits}" == true ]]; then
    aeris_bounded_fetch_error 'the runner cannot enforce the process file-size limit'
    return 125
  fi
  # Git for Windows does not implement these POSIX resource limits. Production
  # is Linux-only; local Windows fixtures retain deadline and aggregate checks.
  timeout -k 5s "${AERIS_FETCH_TIMEOUT_SECONDS}s" "$@"
}

aeris_bounded_fetch_network() {
  aeris_bounded_network_git "$@"
}

aeris_bounded_read_remote_ref() {
  local remote="$1" ref="$2" label="$3" optional="${4:-false}"
  local output_file output_size status sha returned_ref extra tmp_root
  if [[ "${ref}" != refs/heads/* ]]; then
    aeris_bounded_fetch_error "${label} is not an exact branch ref"
    return
  fi
  tmp_root="${AERIS_BOUNDED_FETCH_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
  mkdir -p "${tmp_root}" || {
    aeris_bounded_fetch_error "unable to create bounded ${label} discovery output"
    return
  }
  output_file="$(mktemp "${tmp_root%/}/aeris-remote-ref.XXXXXX")" || true
  if [[ -z "${output_file}" || ! -f "${output_file}" ]]; then
    aeris_bounded_fetch_error "unable to create bounded ${label} discovery output"
    return
  fi
  set +e
  AERIS_BOUNDED_NETWORK_MAX_FILE_BYTES="${AERIS_FETCH_MAX_REMOTE_REF_BYTES}" \
    aeris_bounded_network_git ls-remote --heads \
      "${remote}" "${ref}" >"${output_file}" 2>/dev/null
  status=$?
  set -e
  if ((status != 0)); then
    rm -f -- "${output_file}"
    aeris_bounded_fetch_error "unable to discover ${label} within the hard deadline"
    return
  fi
  output_size="$(wc -c <"${output_file}")"
  if [[ ! "${output_size}" =~ ^[0-9]+$ ||
        ${output_size} -gt ${AERIS_FETCH_MAX_REMOTE_REF_BYTES} ]]; then
    rm -f -- "${output_file}"
    aeris_bounded_fetch_error "${label} discovery exceeded ${AERIS_FETCH_MAX_REMOTE_REF_BYTES} bytes"
    return
  fi
  if ((output_size == 0)) && [[ "${optional}" == true ]]; then
    rm -f -- "${output_file}"
    AERIS_BOUNDED_REMOTE_SHA=''
    return 0
  fi
  if [[ "$(wc -l <"${output_file}")" -ne 1 ]]; then
    rm -f -- "${output_file}"
    aeris_bounded_fetch_error "${label} discovery returned more than one ref"
    return
  fi
  IFS=$'\t ' read -r sha returned_ref extra <"${output_file}" || true
  rm -f -- "${output_file}"
  if [[ ! "${sha}" =~ ^[0-9a-f]{40}$ || "${returned_ref}" != "${ref}" || -n "${extra:-}" ]]; then
    aeris_bounded_fetch_error "${label} discovery returned an invalid exact ref"
    return
  fi
  AERIS_BOUNDED_REMOTE_SHA="${sha}"
}

aeris_bounded_stage_bytes() {
  local root="$1" total=0 file size
  while IFS= read -r -d '' file; do
    size="$(wc -c <"${file}")"
    [[ "${size}" =~ ^[0-9]+$ ]] || return 1
    ((total += size))
  done < <(find "${root}" -type f -print0)
  printf '%s\n' "${total}"
}

aeris_bounded_list_stage_objects() {
  local object_root="$1" output="$2" file current remaining_count remaining_expanded
  local stage="${object_root%/objects}" oid_file="${output}.oids" metadata_limit
  local verify_program metadata_program
  local remaining_count=$((AERIS_FETCH_MAX_RECEIVED_OBJECTS - AERIS_FETCH_RECEIVED_OBJECTS_TOTAL))
  local remaining_expanded=$((AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES - AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL))
  : >"${output}"
  : >"${oid_file}"
  if find "${object_root}" -type l -print -quit | grep -q .; then
    return 1
  fi
  while IFS= read -r -d '' file; do
    local base="${file##*/}" directory="${file%/*}"
    directory="${directory##*/}"
    if [[ "${directory}" =~ ^[0-9a-f]{2}$ && "${base}" =~ ^[0-9a-f]{38}$ ]]; then
      printf '%s%s\n' "${directory}" "${base}" >>"${oid_file}"
      current="$(wc -l <"${oid_file}")"
      current="${current//[[:space:]]/}"
      ((current <= remaining_count)) || return 1
    fi
  done < <(find "${object_root}" -mindepth 2 -maxdepth 2 -type f -print0)
  # Git may unpack small local/HTTP fetches into loose objects (notably on
  # older Git versions). They are safe to account for here: stage bytes and
  # per-object expanded sizes are measured before import, so rejecting the
  # storage format would incorrectly deny a bounded fetch.
  while IFS= read -r -d '' file; do
    current="$(wc -l <"${oid_file}")"
    current="${current//[[:space:]]/}"
    remaining_count=$((AERIS_FETCH_MAX_RECEIVED_OBJECTS - AERIS_FETCH_RECEIVED_OBJECTS_TOTAL - current))
    remaining_expanded=$((AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES - AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL))
    ((remaining_count > 0 && remaining_expanded > 0)) || return 1
    verify_program='
          length($1) == 40 && $1 ~ /^[0-9a-f]+$/ {
            count += 1
            if (count > max_count) exit 42
            print $1
          }
        '
    if ! aeris_bounded_run "${AERIS_FETCH_MAX_RECEIVED_BYTES}" bash -o pipefail -c \
      'git verify-pack -v "$1" | awk -v max_count="$3" "$4" >>"$2"' \
      bash "${file}" "${oid_file}" "${remaining_count}" "${verify_program}"; then
      return 1
    fi
  done < <(find "${object_root}/pack" -maxdepth 1 -type f -name '*.idx' -print0 2>/dev/null)
  current="$(wc -l <"${oid_file}")"
  current="${current//[[:space:]]/}"
  ((current <= AERIS_FETCH_MAX_RECEIVED_OBJECTS - AERIS_FETCH_RECEIVED_OBJECTS_TOTAL)) || return 1
  metadata_limit=$((current * 128 + 1))
  ((metadata_limit <= AERIS_FETCH_MAX_RECEIVED_BYTES)) || metadata_limit="${AERIS_FETCH_MAX_RECEIVED_BYTES}"
  metadata_program='
        {
          if (length($1) != 40 || $1 !~ /^[0-9a-f]+$/ ||
              $2 !~ /^(blob|tree|commit|tag)$/ || $3 !~ /^[0-9]+$/ || NF != 3) exit 43
          expanded += $3
          if (expanded > max_expanded || $3 > max_object ||
              ($2 == "blob" && $3 > max_blob)) exit 42
          print $1, $2, $3
        }
      '
  if ! aeris_bounded_run "${metadata_limit}" bash -o pipefail -c \
    'git -C "$1" cat-file --batch-check="%(objectname) %(objecttype) %(objectsize)" <"$2" | awk -v max_expanded="$4" -v max_object="$5" -v max_blob="$6" "$7" >"$3"' \
    bash "${stage}" "${oid_file}" "${output}" "${remaining_expanded}" \
    "${AERIS_FETCH_MAX_OBJECT_BYTES}" "${AERIS_FETCH_MAX_BLOB_BYTES}" "${metadata_program}"; then
    return 1
  fi
  rm -f -- "${oid_file}"
}

aeris_bounded_validate_stage() {
  local stage="$1" expected="$2" objects_file="$3"
  local bytes count expanded total_bytes total_expanded total_count
  local metadata_file="${stage}/received-metadata"
  bytes="$(aeris_bounded_stage_bytes "${stage}")" || {
    aeris_bounded_fetch_error 'unable to measure received stage storage'
    return
  }
  if ! aeris_bounded_list_stage_objects "${stage}/objects" "${objects_file}"; then
    aeris_bounded_fetch_error 'unable to enumerate received objects'
    return
  fi
  count="$(aeris_bounded_run 1024 wc -l <"${objects_file}")"
  count="${count//[[:space:]]/}"
  expanded="$(aeris_bounded_run 1024 awk '{ total += $3 } END { print total + 0 }' "${objects_file}")"
  if [[ ! "${bytes}" =~ ^[0-9]+$ || ! "${count}" =~ ^[0-9]+$ ||
        ! "${expanded}" =~ ^[0-9]+$ ]]; then
    aeris_bounded_fetch_error 'received-object measurements are invalid'
    return
  fi
  total_bytes=$((AERIS_FETCH_RECEIVED_BYTES_TOTAL + bytes))
  total_expanded=$((AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL + expanded))
  total_count=$((AERIS_FETCH_RECEIVED_OBJECTS_TOTAL + count))
  if ((total_bytes > AERIS_FETCH_MAX_RECEIVED_BYTES)); then
    aeris_bounded_fetch_error "received stage storage exceeds ${AERIS_FETCH_MAX_RECEIVED_BYTES} bytes"
    return
  fi
  if ((total_count > AERIS_FETCH_MAX_RECEIVED_OBJECTS)); then
    aeris_bounded_fetch_error "received object count exceeds ${AERIS_FETCH_MAX_RECEIVED_OBJECTS}"
    return
  fi
  if ((total_expanded > AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES)); then
    aeris_bounded_fetch_error "received expanded objects exceed ${AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES} bytes"
    return
  fi
  cp "${objects_file}" "${metadata_file}"
  if ! aeris_bounded_run "${AERIS_FETCH_MAX_RECEIVED_BYTES}" git -C "${stage}" \
    fsck --strict --no-reflogs --no-dangling "${expected}" >/dev/null; then
    aeris_bounded_fetch_error 'received object graph failed strict fsck within the hard deadline'
    return
  fi
  AERIS_BOUNDED_LAST_RECEIVED_BYTES="${bytes}"
  AERIS_BOUNDED_LAST_RECEIVED_EXPANDED_BYTES="${expanded}"
  AERIS_BOUNDED_LAST_RECEIVED_OBJECTS="${count}"
  AERIS_FETCH_RECEIVED_BYTES_TOTAL="${total_bytes}"
  AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL="${total_expanded}"
  AERIS_FETCH_RECEIVED_OBJECTS_TOTAL="${total_count}"
}

aeris_bounded_publish_exact_ref() {
  local destination="$1" expected="$2" previous_ref="$3" label="$4" final_ref
  local zero=0000000000000000000000000000000000000000
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
        -n "${AERIS_BOUNDED_IMPORT_PRE_PUBLISH_HOOK:-}" ]]; then
    "${AERIS_BOUNDED_IMPORT_PRE_PUBLISH_HOOK}" \
      "${destination}" "${expected}" "${previous_ref}" || return 1
  fi
  if ! aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git update-ref "${destination}" "${expected}" "${previous_ref:-${zero}}"; then
    aeris_bounded_fetch_error "${label} destination changed concurrently before publication"
    return
  fi
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
        -n "${AERIS_BOUNDED_IMPORT_POST_PUBLISH_HOOK:-}" ]]; then
    "${AERIS_BOUNDED_IMPORT_POST_PUBLISH_HOOK}" \
      "${destination}" "${expected}" "${previous_ref}" || return 1
  fi
  final_ref="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --verify "${destination}^{commit}" 2>/dev/null)" || true
  if [[ "${final_ref}" != "${expected}" ]]; then
    aeris_bounded_fetch_error "${label} destination did not retain the exact validated SHA"
    return
  fi
}

aeris_bounded_import_stage() {
  local stage="$1" expected="$2" destination="$3" objects_file="$4"
  local imported_metadata previous_ref import_bytes import_objects
  local total_bytes total_objects remaining_bytes remaining_objects
  previous_ref="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --verify "${destination}" 2>/dev/null || true)"
  imported_metadata="${stage}/imported-metadata"
  remaining_bytes=$((AERIS_FETCH_MAX_IMPORT_BYTES - AERIS_FETCH_IMPORT_BYTES_TOTAL))
  remaining_objects=$((AERIS_FETCH_MAX_IMPORT_OBJECTS - AERIS_FETCH_IMPORT_OBJECTS_TOTAL))
  ((remaining_bytes > 0 && remaining_objects > 0)) || return 1
  # Import ceilings are checked against the entire validated quarantine store.
  # This is conservative when the caller already has some objects, and avoids
  # observing or deleting files in a shared object directory under concurrency.
  import_bytes="$(aeris_bounded_stage_bytes "${stage}/objects")" || return 1
  import_objects="$(wc -l <"${objects_file}")"
  import_objects="${import_objects//[[:space:]]/}"
  if [[ ! "${import_bytes}" =~ ^[0-9]+$ || ! "${import_objects}" =~ ^[0-9]+$ ||
        ${import_bytes} -gt ${remaining_bytes} || ${import_objects} -gt ${remaining_objects} ]]; then
    return 1
  fi
  if ! aeris_bounded_run "${remaining_bytes}" git \
    -c fetch.fsckObjects=true -c transfer.fsckObjects=true -c fetch.unpackLimit=0 \
    fetch --quiet --force --no-tags --no-recurse-submodules --refmap= --no-write-fetch-head \
    --no-auto-maintenance --no-write-commit-graph \
    "${stage}" refs/aeris/incoming; then
    return 1
  fi
  if ! aeris_bounded_run "${remaining_bytes}" git \
    cat-file --batch-check='%(objectname) %(objecttype) %(objectsize)' \
    < <(awk '{ print $1 }' "${objects_file}") >"${imported_metadata}" ||
    ! cmp -s "${objects_file}" "${imported_metadata}" ||
    ! aeris_bounded_run "${remaining_bytes}" git \
      fsck --strict --no-reflogs --no-dangling "${expected}" >/dev/null; then
    return 1
  fi
  if ! aeris_bounded_publish_exact_ref \
    "${destination}" "${expected}" "${previous_ref}" 'validated import'; then
    return 1
  fi
  AERIS_BOUNDED_LAST_IMPORT_BYTES="${import_bytes}"
  AERIS_BOUNDED_LAST_IMPORT_OBJECTS="${import_objects}"
  total_bytes=$((AERIS_FETCH_IMPORT_BYTES_TOTAL + import_bytes))
  total_objects=$((AERIS_FETCH_IMPORT_OBJECTS_TOTAL + import_objects))
  AERIS_FETCH_IMPORT_BYTES_TOTAL="${total_bytes}"
  AERIS_FETCH_IMPORT_OBJECTS_TOTAL="${total_objects}"
}

aeris_bounded_fetch_ref() {
  local remote="$1" ref="$2" expected="$3" destination="$4" label="$5"
  local tmp_root stage object_list fetched remote_sha
  if [[ "${ref}" != refs/heads/* || ! "${expected}" =~ ^[0-9a-f]{40}$ ]]; then
    aeris_bounded_fetch_error "${label} fetch coordinates are invalid"
    return
  fi
  if ! aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git check-ref-format "${destination}" >/dev/null 2>&1; then
    aeris_bounded_fetch_error "${label} destination ref is invalid"
    return
  fi
  aeris_bounded_read_remote_ref "${remote}" "${ref}" "${label}" || return
  remote_sha="${AERIS_BOUNDED_REMOTE_SHA}"
  if [[ "${remote_sha}" != "${expected}" ]]; then
    aeris_bounded_fetch_error "${label} drifted from ${expected} to ${remote_sha}"
    return
  fi
  if aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git cat-file -e "${expected}^{commit}" 2>/dev/null &&
    aeris_bounded_run "${AERIS_FETCH_MAX_IMPORT_BYTES}" git \
      fsck --strict --no-reflogs --no-dangling "${expected}" >/dev/null 2>&1; then
    local previous_ref
    previous_ref="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --verify "${destination}" 2>/dev/null || true)"
    aeris_bounded_publish_exact_ref \
      "${destination}" "${expected}" "${previous_ref}" "local exact ${label}" || {
      aeris_bounded_fetch_error "unable to retain local exact ${label}"
      return
    }
    AERIS_BOUNDED_LAST_RECEIVED_BYTES=0
    AERIS_BOUNDED_LAST_RECEIVED_EXPANDED_BYTES=0
    AERIS_BOUNDED_LAST_RECEIVED_OBJECTS=0
    AERIS_BOUNDED_LAST_IMPORT_BYTES=0
    AERIS_BOUNDED_LAST_IMPORT_OBJECTS=0
    AERIS_BOUNDED_FETCHED_SHA="${expected}"
    return 0
  fi
  tmp_root="${AERIS_BOUNDED_FETCH_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
  if ! mkdir -p "${tmp_root}"; then
    aeris_bounded_fetch_error 'unable to create the isolated receiver parent'
    return
  fi
  stage="$(mktemp -d "${tmp_root%/}/aeris-bounded-fetch.XXXXXX")" || true
  if [[ -z "${stage}" || ! -d "${stage}" ]]; then
    aeris_bounded_fetch_error 'unable to create an isolated object receiver'
    return
  fi
  object_list="${stage}/received-objects"
  if ! git init -q --bare "${stage}"; then
    rm -rf -- "${stage}"
    aeris_bounded_fetch_error 'unable to initialize isolated object receiver'
    return
  fi
  if ! aeris_bounded_fetch_network \
    -c fetch.fsckObjects=true \
    -c transfer.fsckObjects=true \
    -c fetch.unpackLimit=0 \
    -C "${stage}" fetch --quiet --force --no-tags --no-recurse-submodules --refmap= \
    --no-auto-maintenance --no-write-commit-graph \
    "${remote}" "+${ref}:refs/aeris/incoming"; then
    rm -rf -- "${stage}"
    aeris_bounded_fetch_error "unable to fetch ${label} within the hard deadline"
    return
  fi
  if [[ "${AERIS_BOUNDED_FETCH_TEST_MODE:-false}" == true &&
        -n "${AERIS_BOUNDED_STAGE_POST_HOOK:-}" ]]; then
    if ! "${AERIS_BOUNDED_STAGE_POST_HOOK}" "${stage}"; then
      rm -rf -- "${stage}"
      aeris_bounded_fetch_error "${label} test stage hook failed"
      return
    fi
  fi
  fetched="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git -C "${stage}" rev-parse --verify 'refs/aeris/incoming^{commit}' 2>/dev/null)" || {
    rm -rf -- "${stage}"
    aeris_bounded_fetch_error "${label} did not resolve to a commit"
    return
  }
  if [[ -e "${stage}/objects/info/alternates" ]] ||
    find "${stage}/objects/pack" -maxdepth 1 -type f -name '*.promisor' -print -quit 2>/dev/null |
      grep -q .; then
    rm -rf -- "${stage}"
    aeris_bounded_fetch_error "${label} receiver created a forbidden alternate or promisor store"
    return 1
  fi
  if [[ "${fetched}" != "${expected}" ]]; then
    rm -rf -- "${stage}"
    aeris_bounded_fetch_error "${label} drifted from ${expected} to ${fetched}"
    return 1
  fi
  if ! aeris_bounded_validate_stage "${stage}" "${expected}" "${object_list}"; then
    rm -rf -- "${stage}"
    return 1
  fi
  if ! aeris_bounded_import_stage "${stage}" "${expected}" "${destination}" "${object_list}"; then
    rm -rf -- "${stage}"
    aeris_bounded_fetch_error "unable to import validated ${label} within the caller bounds"
    return
  fi
  fetched="$(aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git rev-parse --verify "${destination}^{commit}" 2>/dev/null)" || true
  rm -rf -- "${stage}"
  if [[ "${fetched}" != "${expected}" ]]; then
    aeris_bounded_fetch_error "validated ${label} import did not preserve the exact SHA"
    return
  fi
  AERIS_BOUNDED_FETCHED_SHA="${fetched}"
}

aeris_enforce_change_bounds() {
  local base="$1" tip="$2" label="$3"
  local meta path old_mode new_mode old_oid new_oid status extra type size oid
  local path_count=0 total_blob_bytes=0
  local tmp_root diff_file oid_file metadata_file diff_bytes failed=''
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git cat-file -e "${base}^{commit}" 2>/dev/null || {
    aeris_bounded_fetch_error "${label} checkpoint commit is unavailable"
    return
  }
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git cat-file -e "${tip}^{commit}" 2>/dev/null || {
    aeris_bounded_fetch_error "${label} tip commit is unavailable"
    return
  }
  tmp_root="${AERIS_BOUNDED_FETCH_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
  mkdir -p "${tmp_root}" || {
    aeris_bounded_fetch_error "${label} cannot create a bounded diff workspace"
    return
  }
  diff_file="$(mktemp "${tmp_root%/}/aeris-bounded-diff.XXXXXX")" || true
  if [[ -z "${diff_file}" || ! -f "${diff_file}" ]]; then
    aeris_bounded_fetch_error "${label} cannot create a bounded diff stream"
    return
  fi
  oid_file="${diff_file}.oids"
  metadata_file="${diff_file}.metadata"
  : >"${oid_file}"
  if ! aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git ls-tree -r -t -z --full-tree "${tip}" |
    awk -v max="${AERIS_FETCH_MAX_TREE_ENTRIES}" \
      'BEGIN { RS = "\0" } { count += 1; if (count > max) exit 42 }'; then
    rm -f -- "${diff_file}" "${oid_file}" "${metadata_file}"
    aeris_bounded_fetch_error "${label} tree traversal exceeds its entry or time bound"
    return
  fi
  if ! aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" \
    git diff --raw -z --abbrev=40 --no-renames "${base}" "${tip}" -- >"${diff_file}"; then
    rm -f -- "${diff_file}" "${oid_file}" "${metadata_file}"
    aeris_bounded_fetch_error "${label} changed-path enumeration exceeded its output, memory, or time bound"
    return
  fi
  diff_bytes="$(wc -c <"${diff_file}")"
  if [[ ! "${diff_bytes}" =~ ^[0-9]+$ ]] || ((diff_bytes > AERIS_FETCH_MAX_DIFF_BYTES)); then
    rm -f -- "${diff_file}" "${oid_file}" "${metadata_file}"
    aeris_bounded_fetch_error "${label} changed-path output exceeds ${AERIS_FETCH_MAX_DIFF_BYTES} bytes"
    return
  fi
  while IFS= read -r -d '' meta; do
    if ! IFS= read -r -d '' path; then
      failed="${label} changed-path stream is truncated"
      break
    fi
    read -r old_mode new_mode old_oid new_oid status extra <<<"${meta#:}"
    if [[ ! "${old_mode}" =~ ^[0-7]{6}$ || ! "${new_mode}" =~ ^[0-7]{6}$ ||
          ! "${old_oid}" =~ ^[0-9a-f]{40}$ || ! "${new_oid}" =~ ^[0-9a-f]{40}$ ||
          ! "${status}" =~ ^[AMDTUXB]([0-9]{1,3})?$ || -n "${extra:-}" ]]; then
      failed="${label} changed-path metadata is invalid: ${meta}"
      break
    fi
    ((path_count += 1))
    if ((path_count > AERIS_FETCH_MAX_CHANGED_PATHS)); then
      failed="${label} exceeds ${AERIS_FETCH_MAX_CHANGED_PATHS} changed paths"
      break
    fi
    if [[ "${new_oid}" != 0000000000000000000000000000000000000000 ]]; then
      printf '%s\n' "${new_oid}" >>"${oid_file}"
    fi
  done <"${diff_file}"
  if [[ -z "${failed}" && -s "${oid_file}" ]]; then
    if ! aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git \
      cat-file --batch-check='%(objectname) %(objecttype) %(objectsize)' \
      <"${oid_file}" >"${metadata_file}"; then
      failed="${label} changed-object inspection exceeded its time bound"
    else
      while read -r oid type size extra; do
        if [[ ! "${oid}" =~ ^[0-9a-f]{40}$ || ! "${type}" =~ ^(blob|tree|commit)$ ||
              ! "${size}" =~ ^[0-9]+$ || -n "${extra:-}" ]]; then
          failed="${label} changed object metadata is invalid"
          break
        fi
        if ((size > AERIS_FETCH_MAX_OBJECT_BYTES)); then
          failed="${label} changed object ${oid} exceeds ${AERIS_FETCH_MAX_OBJECT_BYTES} bytes"
          break
        fi
        if [[ "${type}" == blob ]]; then
          if ((size > AERIS_FETCH_MAX_BLOB_BYTES)); then
            failed="${label} changed blob exceeds ${AERIS_FETCH_MAX_BLOB_BYTES} bytes"
            break
          fi
          ((total_blob_bytes += size))
          if ((total_blob_bytes > AERIS_FETCH_MAX_CHANGED_BLOB_BYTES)); then
            failed="${label} exceeds ${AERIS_FETCH_MAX_CHANGED_BLOB_BYTES} changed-blob bytes"
            break
          fi
        fi
      done <"${metadata_file}"
    fi
  fi
  rm -f -- "${diff_file}" "${oid_file}" "${metadata_file}"
  if [[ -n "${failed}" ]]; then
    aeris_bounded_fetch_error "${failed}"
    return
  fi
}
