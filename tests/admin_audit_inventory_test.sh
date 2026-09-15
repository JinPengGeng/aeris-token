#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INVENTORY="${REPO_ROOT}/docs/issue-triage/issue-255-admin-mutation-inventory.txt"
HANDLER_ROOT="${REPO_ROOT}/apps/aether-gateway/src/handlers/admin"

[[ -f "${INVENTORY}" ]] || {
  printf 'missing audit event inventory: %s\n' "${INVENTORY}" >&2
  exit 1
}

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/aether-admin-audit-inventory.XXXXXX")"
trap 'rm -rf "${tmp_dir}"' EXIT

# Restrict extraction to explicit attach calls and require a literal bounded
# event name. Generic finalizer fallback names are intentionally separate from
# this list and are checked below.
find "${HANDLER_ROOT}" -name '*.rs' -print0 \
  | LC_ALL=C xargs -0 perl -0777 -ne \
      'while (/attach_admin_audit_response\((?:(?!\);).){0,2000}?"(admin_[A-Za-z0-9_]+)"/sg) { print "$1\n" }' \
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
while IFS= read -r event; do
  count="$(rg -l --fixed-strings "\"${event}\"" "${HANDLER_ROOT}" --glob '*.rs' \
    | wc -l | tr -d ' ')"
  (( count > 0 )) || {
    printf 'inventory event is not present in an admin handler: %s\n' "${event}" >&2
    exit 1
  }
done < "${tmp_dir}/declared"

rg -q 'admin_mutation_completed' "${REPO_ROOT}/apps/aether-gateway/src/audit/admin.rs"
rg -q 'admin_mutation_failed' "${REPO_ROOT}/apps/aether-gateway/src/audit/admin.rs"

printf 'admin audit inventory is current (%s explicit events; generic fallback present)\n' \
  "$(wc -l < "${tmp_dir}/declared" | tr -d ' ')"
