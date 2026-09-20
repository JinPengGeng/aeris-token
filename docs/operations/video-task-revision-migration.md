# Video task revision rollout (#211)

The `20260917000000_add_video_task_claim_fencing_token.sql` migration gives
existing tasks `row_revision = 1` and `claim_fencing_token = 0`. Newly inserted
tasks also start at revision 1. A zero revision only identifies an unbound
local snapshot; it never authorizes an update of a persisted row.

Every poll claim advances both the fencing token and row revision. Updates
compare the revision read before the upstream request; poll completions also
compare their claim token. Successful updates and administrative identity
anonymization advance the revision. A rejected update restores the database
observation instead of retrying stale content with a newer revision. Completed
and failed tasks can become deleted; other terminal states cannot be revived
or deleted through this transition.

## Deployment procedure

1. Stop admission of new video creates and mutations, then drain in-flight
   video requests and all old pollers/writers. Include administrator/user/key
   deletion and provider cleanup writers because they modify video rows too.
2. Preserve a database backup and a list of outstanding video tasks and
   upstream IDs. Keep old writers stopped throughout migration and validation.
3. Apply the migration using the normal migration runner. Verify both columns
   are present and not null, with the documented defaults. Existing embedded
   snapshots may lack revision; the database row supplies the version and
   authoritative lifecycle during reconstruction.
4. Deploy the new Gateway and workers together. Run the real PostgreSQL video
   target, the Gateway video/async-task suites, and an authenticated task list
   and read before restoring traffic. Monitor conflicts and held financial
   obligations; a projection conflict does not make an already successful
   upstream call disappear from usage records.
5. Restore admission after those checks pass. Record the migration version,
   deployed commit, start/end times and validation output in the change record.

The additive DDL allows an old binary to connect, but it does not make mixed
writers safe: old code can change lifecycle fields without advancing revision.
Do not roll back only some writers or delete the new columns while the new
binary is running. On failure keep admission and old writers stopped, preserve
database state and upstream receipts, and use a forward fix. A full restore
requires reconciling upstream tasks and financial obligations after the backup
point; copying old rows back is not sufficient.

## Local acceptance boundary

The isolated PostgreSQL tests exercise reclaim with a new token, stale token
and revision rejection, same-second races, bounded clock differences, task
summary decoding, identity anonymization followed by a rejected stale write,
and the terminal deletion matrix. Core tests cover monotonic cache publication
and local-only compare-and-replace. Native xAI presentation is retained after a
successful write; after restart, terminal presentation can be filled only
while the row and cached observation remain unchanged, without changing the
database lifecycle or version.

These local checks are not a production rollout record. Deployment remains
subject to the drain procedure above and validation of the exact deployed
revision.
