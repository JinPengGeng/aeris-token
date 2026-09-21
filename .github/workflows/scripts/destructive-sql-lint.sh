#!/usr/bin/env bash
# destructive-sql-lint.sh — refuse destructive SQL unless explicitly exempted.
#
# Scans migration/bootstrap SQL under crates/aether-data for statements that
# drop or truncate data (DROP TABLE, DROP COLUMN, TRUNCATE, DELETE without a
# WHERE clause). Schema files are expected to be expand-only; any intentional
# exception must carry a marker comment on the offending line:
#
#   -- destructive-sql: allow <reason>
#
# The marker must appear on the same line as the statement (or the line
# directly above it). Anything else fails the lint.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ALLOW_MARKER="destructive-sql: allow"

status=0

# Match destructive statements, case-insensitive, ignoring comment-only lines.
pattern='^[[:space:]]*(DROP[[:space:]]+(TABLE|COLUMN|SCHEMA)|TRUNCATE|DELETE[[:space:]]+FROM)'

while IFS= read -r file; do
  lineno=0
  prev_line=""
  while IFS= read -r line; do
    lineno=$((lineno + 1))
    # Strip trailing SQL line comments to allow: "-- destructive-sql: allow ..."
    stripped="${line%%--*}"
    if [[ "$stripped" =~ $pattern ]]; then
      if [[ "$line" != *"$ALLOW_MARKER"* && "$prev_line" != *"$ALLOW_MARKER"* ]]; then
        echo "::error file=$file,line=$lineno::destructive SQL statement requires an explicit '$ALLOW_MARKER <reason>' exemption comment"
        status=1
      fi
    fi
    prev_line="$line"
  done < "$file"
done < <(find "$ROOT/crates/aether-data" \( \
  -path '*/schema/*' -o \
  -path '*/migrations/*' -o \
  -path '*/backfills/*' \
  \) -name '*.sql' -type f | sort)

if [[ "$status" -ne 0 ]]; then
  echo "destructive SQL lint failed: add an explicit '$ALLOW_MARKER <reason>' comment to each flagged statement."
fi
exit "$status"
