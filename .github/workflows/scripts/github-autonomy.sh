#!/usr/bin/env bash

aeris_autonomy_fail() {
  printf 'error: autonomy window validation failed: %s\n' "$1" >&2
  return 78
}

aeris_require_active_autonomy_window() {
  local minimum_remaining_seconds="${1:-0}"
  local expires_at="${AERIS_AUTONOMY_EXPIRES_AT:-}" expires_epoch round_trip now_epoch

  if [[ ! "${minimum_remaining_seconds}" =~ ^[0-9]+$ ]]; then
    aeris_autonomy_fail 'minimum remaining autonomy seconds must be a non-negative integer'
    return
  fi
  if [[ ! "${expires_at}" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]]; then
    aeris_autonomy_fail 'AERIS_AUTONOMY_EXPIRES_AT must be an exact UTC timestamp (YYYY-MM-DDTHH:MM:SSZ)'
    return
  fi
  if ! expires_epoch="$(date -u -d "${expires_at}" +%s 2>/dev/null)"; then
    aeris_autonomy_fail 'AERIS_AUTONOMY_EXPIRES_AT is not a valid UTC timestamp'
    return
  fi
  if ! round_trip="$(date -u -d "@${expires_epoch}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null)"; then
    aeris_autonomy_fail 'unable to normalize AERIS_AUTONOMY_EXPIRES_AT'
    return
  fi
  if [[ "${round_trip}" != "${expires_at}" ]]; then
    aeris_autonomy_fail 'AERIS_AUTONOMY_EXPIRES_AT is not an exact UTC timestamp'
    return
  fi
  if ! now_epoch="$(date -u +%s 2>/dev/null)"; then
    aeris_autonomy_fail 'unable to read the current UTC time'
    return
  fi
  if [[ ! "${now_epoch}" =~ ^[0-9]+$ ]]; then
    aeris_autonomy_fail 'current UTC time is invalid'
    return
  fi
  if (( now_epoch + minimum_remaining_seconds >= expires_epoch )); then
    aeris_autonomy_fail "authorization expired at ${expires_at}"
    return
  fi
}

aeris_gh() {
  aeris_require_active_autonomy_window || return
  command gh "$@"
}

aeris_git_network() {
  aeris_require_active_autonomy_window || return
  command git "$@"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  aeris_require_active_autonomy_window
fi
