# Video task registry retention decision

This slice closes the terminal age-retention gap left after the per-provider
capacity bound was added to `VideoTaskRegistry`.

- Terminal snapshots are removed after 24 hours when a file-backed registry is
  loaded, then the existing 4096-entry per-provider cap is applied.
- Active snapshots are never removed by this policy. The database-backed video
  task lifecycle and the poller's due-task claims remain unchanged.
- Snapshots from older versions without a creation timestamp are retained until
  the capacity bound removes them. This avoids treating unknown age as expired.
- The legacy OpenAI `created_at_unix_ms` field is interpreted as Unix seconds,
  matching the video-task database projection and existing poll scheduling;
  historical millisecond values are normalized before age comparison.
- Cleanup is persisted through the existing encrypted atomic rewrite. No
  plaintext store, credential, API response, or upstream repository is changed.

This does not close the remaining #211 work on stale active tasks, lease-loss
response semantics, or provider lifecycle/operational acceptance.
