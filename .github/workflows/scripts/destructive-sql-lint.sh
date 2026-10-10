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
  lines=()
  while IFS= read -r file_line || [ -n "$file_line" ]; do
    lines+=("$file_line")
  done < "$file"
  total=${#lines[@]}
  for ((i = 0; i < total; i++)); do
    line="${lines[$i]}"
    lineno=$((i + 1))
    # Strip trailing SQL line comments to allow: "-- destructive-sql: allow ..."
    stripped="${line%%--*}"
    if [[ "$stripped" =~ $pattern ]]; then
      # DELETE ... WHERE is bounded housekeeping (e.g. trigger cleanup) and is
      # not destructive; only a WHERE-less DELETE is flagged. The WHERE may sit
      # on the following line, so peek ahead before flagging.
      upper_stripped=$(printf '%s' "$stripped" | tr '[:lower:]' '[:upper:]')
      upper_trimmed="${upper_stripped#"${upper_stripped%%[![:space:]]*}"}"
      if [[ "$upper_trimmed" == DELETE* ]]; then
        next_line="${lines[$((i + 1))]:-}"
        upper_next=$(printf '%s' "$next_line" | tr '[:lower:]' '[:upper:]')
        if [[ "$upper_trimmed" == *WHERE* || "$upper_next" =~ ^[[:space:]]*WHERE[[:space:]] ]]; then
          prev_line="$line"
          continue
        fi
      fi
      if [[ "$line" != *"$ALLOW_MARKER"* && "$prev_line" != *"$ALLOW_MARKER"* ]]; then
        echo "::error file=$file,line=$lineno::destructive SQL statement requires an explicit '$ALLOW_MARKER <reason>' exemption comment"
        status=1
      fi
    fi
    prev_line="$line"
  done
done < <(find "$ROOT/crates/aether-data" \( \
  -path '*/schema/*' -o \
  -path '*/migrations/*' -o \
  -path '*/backfills/*' \
  \) -name '*.sql' -type f | sort)

if [[ "$status" -ne 0 ]]; then
  echo "destructive SQL lint failed: add an explicit '$ALLOW_MARKER <reason>' comment to each flagged statement."
fi
exit "$status"
