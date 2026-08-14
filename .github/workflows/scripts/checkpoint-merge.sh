#!/usr/bin/env bash
set -euo pipefail

base="${1:?checkpoint base is required}"
ours="${2:?fork base is required}"
theirs="${3:?upstream tip is required}"

fail_error() {
  printf '%s\n' "$1" >&2
  printf 'state=error\n'
  exit 3
}

require_git_capabilities() {
  local version version_major version_minor

  version="$(git version 2>/dev/null)" || fail_error 'unable to determine Git version'
  if [[ ! "${version}" =~ ^git[[:space:]]version[[:space:]]([0-9]+)\.([0-9]+) ]]; then
    fail_error 'unable to parse Git version'
  fi

  version_major="${BASH_REMATCH[1]}"
  version_minor="${BASH_REMATCH[2]}"
  if ((version_major < 2 || (version_major == 2 && version_minor < 38))); then
    fail_error 'Git 2.38 or newer is required for git merge-tree --write-tree'
  fi
}

require_git_capabilities

for entry in "checkpoint:${base}" "fork:${ours}" "upstream:${theirs}"; do
  label="${entry%%:*}"
  ref="${entry#*:}"
  resolved="$(git rev-parse --verify "${ref}^{commit}" 2>/dev/null)" ||
    fail_error "invalid ${label} commit: ${ref}"
  case "${label}" in
    checkpoint) base="${resolved}" ;;
    fork) ours="${resolved}" ;;
    upstream) theirs="${resolved}" ;;
  esac
done

set +e
git merge-base --is-ancestor "${base}" "${theirs}"
ancestor_status=$?
set -e

case "${ancestor_status}" in
  0) ;;
  1)
    printf 'state=history_rewrite\n'
    exit 2
    ;;
  *) fail_error 'unable to verify upstream checkpoint ancestry' ;;
esac

set +e
merge_output="$({
  git merge-tree \
    --write-tree \
    --name-only \
    --merge-base="${base}" \
    "${ours}" \
    "${theirs}"
} 2>&1)"
merge_status=$?
set -e

if ((merge_status == 1)); then
  printf '%s\n' "${merge_output}" >&2
  printf 'state=conflict\n'
  exit 1
fi
if ((merge_status != 0)); then
  printf '%s\n' "${merge_output}" >&2
  printf 'state=error\n'
  exit 3
fi

tree="${merge_output%%$'\n'*}"
git rev-parse --verify "${tree}^{tree}" >/dev/null 2>&1 ||
  fail_error 'git merge-tree returned an invalid tree'

printf 'state=clean\n'
printf 'tree=%s\n' "${tree}"
