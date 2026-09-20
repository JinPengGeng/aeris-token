#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INVENTORY="${REPO_ROOT}/docs/issue-triage/issue-255-admin-mutation-inventory.txt"
EXPECTED_EXPLICIT_EVENTS=146
SCAN_PATHS=(
  "${REPO_ROOT}/apps/aether-gateway/src/handlers/admin"
  "${REPO_ROOT}/apps/aether-gateway/src/handlers/proxy/local.rs"
  "${REPO_ROOT}/apps/aether-gateway/src/api/ops/audit.rs"
)

[[ -f "${INVENTORY}" ]] || {
  printf 'missing audit event inventory: %s\n' "${INVENTORY}" >&2
  exit 1
}

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/aether-admin-audit-inventory.XXXXXX")"
trap 'rm -rf "${tmp_dir}"' EXIT

# Extract literals passed through any audit-response wrapper, plus named event
# constants used by dynamic taxonomies. The constant prefix is the contract for
# dynamic producers; scanning every `admin_*` string would incorrectly include
# actions, target types, metrics, and test fixtures.
find "${SCAN_PATHS[@]}" -type f -name '*.rs' -print0 \
  | LC_ALL=C xargs -0 perl -0777 -ne \
      'while (/(?:attach_[A-Za-z0-9_]*audit_response|attach_admin_audit_event)\((?:(?!\);).){0,2000}?"(admin_[A-Za-z0-9_]+)"/sg) { print "$1\n" } while (/\bconst\s+ADMIN_AUDIT_EVENT_[A-Z0-9_]+\s*:\s*&str\s*=\s*"(admin_[A-Za-z0-9_]+)"/sg) { print "$1\n" }' \
  | sort -u > "${tmp_dir}/actual"

sed -E 's/[[:space:]]+#.*$//' "${INVENTORY}" \
  | sed -n -E '/^admin_[A-Za-z0-9_]+$/p' \
  | sort -u > "${tmp_dir}/declared"

if ! diff -u "${tmp_dir}/declared" "${tmp_dir}/actual"; then
  printf '\nAudit event inventory is stale. Add/remove the event in %s.\n' "${INVENTORY}" >&2
  exit 1
fi

# Every explicit event must be attached through the shared helper. A literal
# event name elsewhere is either a generic fallback or needs an explicit
# review before it can become a durable audit event.
scan_files="${tmp_dir}/scan-files"
if ! find "${SCAN_PATHS[@]}" -type f -name '*.rs' -print0 > "${scan_files}"; then
  printf 'failed to enumerate audit producer files\n' >&2
  exit 1
fi
scan_paths=()
while IFS= read -r -d '' scan_path; do
  scan_paths+=("${scan_path}")
done < "${scan_files}"

while IFS= read -r event; do
  if matches="$(grep -lF -- "\"${event}\"" "${scan_paths[@]}")"; then
    count="$(printf '%s\n' "${matches}" | wc -l | tr -d ' ')"
  else
    grep_status=$?
    (( grep_status == 1 )) || {
      printf 'failed to scan audit producers for event: %s\n' "${event}" >&2
      exit "${grep_status}"
    }
    count=0
  fi
  (( count > 0 )) || {
    printf 'inventory event is not present in an admin handler: %s\n' "${event}" >&2
    exit 1
  }
done < "${tmp_dir}/declared"

grep -Fq -- 'admin_mutation_completed' "${REPO_ROOT}/apps/aether-gateway/src/audit/admin.rs"
grep -Fq -- 'admin_mutation_failed' "${REPO_ROOT}/apps/aether-gateway/src/audit/admin.rs"

actual_count="$(wc -l < "${tmp_dir}/declared" | tr -d ' ')"
[[ "${actual_count}" == "${EXPECTED_EXPLICIT_EVENTS}" ]] || {
  printf 'unexpected explicit audit event count: got %s, expected %s\n' \
    "${actual_count}" "${EXPECTED_EXPLICIT_EVENTS}" >&2
  exit 1
}

printf 'admin audit inventory is current (%s explicit events; generic fallback present)\n' \
  "${actual_count}"
