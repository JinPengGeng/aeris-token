#!/usr/bin/env bash
set -euo pipefail

exception_file=".github/security/cargo-audit-exception.json"
test -f "$exception_file"

advisory_id="$(jq -er '.advisory_id' "$exception_file")"
package="$(jq -er '.package' "$exception_file")"
affected_version="$(jq -er '.affected_version' "$exception_file")"
owner="$(jq -er '.owner' "$exception_file")"
created_on="$(jq -er '.created_on' "$exception_file")"
expires_on="$(jq -er '.expires_on' "$exception_file")"
jq -e '.review_cadence and .rationale' "$exception_file" >/dev/null

test "$advisory_id" = "RUSTSEC-2023-0071"
test "$package" = "rsa"
test "$affected_version" = "0.9.10"
test -n "$owner"
test -n "$created_on"
test -n "$expires_on"
test "$created_on" != "$expires_on"

python3 - "$created_on" "$expires_on" <<'PY'
from datetime import date, timedelta
import sys

created, expires = (date.fromisoformat(value) for value in sys.argv[1:])
if created > date.today() or expires <= created or expires > created + timedelta(days=30):
    raise SystemExit("exception expiry must be within 30 days of creation")
PY

if ! awk '
  /^\[\[package\]\]$/ {
    if (name == "rsa" && version == "0.9.10") found = 1
    name = version = ""
  }
  /^name = / { name = $3; gsub(/"/, "", name) }
  /^version = / { version = $3; gsub(/"/, "", version) }
  END {
    if (name == "rsa" && version == "0.9.10") found = 1
    exit !found
  }
' Cargo.lock; then
  echo "Cargo.lock no longer contains the reviewed rsa 0.9.10 package" >&2
  exit 1
fi

today="$(date -u +%F)"
if [[ "$today" > "$expires_on" ]]; then
  echo "Cargo advisory exception $advisory_id expired on $expires_on" >&2
  exit 1
fi

# Keep this exception tied to the reviewed production dependency surface. Do
# not pass --all-features here: that would deliberately activate optional
# backends (including sqlx-mysql) and make this guard reject its own review.
# Optional packages remain visible to cargo-audit through Cargo.lock below.
resolved="$(cargo tree --locked --workspace --target all --format '{p}' --prefix none)"
if rg -q '^(rsa|sqlx-mysql) v' <<<"$resolved"; then
  echo "rsa/sqlx-mysql is active in the resolved graph; review the exception" >&2
  exit 1
fi

# Only this exact advisory is tolerated; all other advisories and audit errors
# remain fail-closed. The expiry and scope checks above prevent silent drift.
cargo audit --ignore "$advisory_id"
