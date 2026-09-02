#!/usr/bin/env bash
set -euo pipefail

# Bounded exact-ref fetch for the isolated conflict review and finalize jobs.
#
# A conflict candidate is generated from a pinned fork base, a pinned upstream
# tip, and an exactly published synchronization head. Any job that later
# re-materializes, reviews, or merges that candidate must re-prove both
# coordinates before trusting local objects:
#
#   - the synchronization branch still tips at the exact published head, and
#   - the live upstream branch tip still equals the pinned source SHA.
#
# Both fetches share the compiled bounded transport (hard deadline, byte and
# object ceilings, strict fsck, CAS publication). A drifted coordinate fails
# closed, so a stale candidate is never revalidated or merged (TOCTOU fence).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOUNDED_FETCH_HELPER="${BOUNDED_FETCH_HELPER:-${SCRIPT_DIR}/bounded-git-fetch.sh}"
source "${BOUNDED_FETCH_HELPER}"

fail_error() {
  printf 'error: conflict ref fetch: %s\n' "$1" >&2
  exit 1
}

head_sha="${1:?published head SHA is required}"
upstream_repository="${2:?upstream repository is required}"
upstream_branch="${3:?upstream branch is required}"
upstream_sha="${4:?upstream source SHA is required}"
sync_branch="${5:-automation/sync-upstream}"
policy_path="${6:-.github/upstream-sync-policy.yml}"

[[ "${head_sha}" =~ ^[0-9a-f]{40}$ ]] ||
  fail_error 'published head SHA must be a full lowercase commit SHA'
[[ "${upstream_sha}" =~ ^[0-9a-f]{40}$ ]] ||
  fail_error 'upstream source SHA must be a full lowercase commit SHA'
[[ "${upstream_repository}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$ ]] ||
  fail_error 'upstream repository must be owner/repo'
[[ "${upstream_branch}" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] ||
  fail_error 'upstream branch name is invalid'
[[ "${sync_branch}" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] ||
  fail_error 'synchronization branch name is invalid'

bounded_conflict_git() {
  aeris_bounded_run "${AERIS_FETCH_MAX_DIFF_BYTES}" git "$@"
}

AERIS_BOUNDED_FETCH_CREDENTIALLESS=true
aeris_bounded_fetch_init "${policy_path}"

# Fence the exact published head first: an out-of-band move of the
# synchronization branch invalidates every later review and merge input.
aeris_bounded_fetch_ref origin "refs/heads/${sync_branch}" "${head_sha}" \
  "refs/remotes/origin/${sync_branch}" 'published synchronization head'

# Fence the live upstream tip before the candidate is re-materialized: the
# bounded remote read fails closed when upstream drifted after the candidate
# was generated, instead of revalidating a stale integration.
bounded_conflict_git remote remove upstream >/dev/null 2>&1 || true
bounded_conflict_git remote add upstream "https://github.com/${upstream_repository}.git"
aeris_bounded_fetch_ref upstream "refs/heads/${upstream_branch}" "${upstream_sha}" \
  "refs/remotes/upstream/${upstream_branch}" 'upstream branch'
