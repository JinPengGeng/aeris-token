# Issue #255 Docker upgrade safety decision

Date: 2026-09-13

## Decision

The Compose updater now fails closed for unattended rolling image tags
(`:latest`, `:nightly`, and omitted tags) unless the operator explicitly passes
`--allow-floating-tag`. Set the image in the deployment's effective Compose
configuration before running the updater (`APP_IMAGE` in the shipped files).
Version tags can also be moved by a publisher; use a digest for immutability.

Every real Compose update creates a PostgreSQL custom-format dump before the
app image is pulled or recreated. A unique bundle under `./backups` (override
with `--backup-dir`) contains `postgres.dump` and `metadata`, including the UTC
timestamp, previous immutable image ID, and target image. The directory is
0700 and both files are 0600 even with a public caller umask. The dump uses the
running PostgreSQL service's database/user/password settings, never the host's
unrelated PostgreSQL variables. A failed, empty, or unreadable archive stops
the update; `pg_restore --list` checks the archive before the bundle is
published with an atomic directory rename. Same-second updates receive
different names. Deployments using another database or an external PostgreSQL
service must provide their own verified backup and explicitly opt out with
`--skip-backup`; the updater cannot validate an external backup.

The updater records the running container's immutable `.Image` ID before any
pull. Both modern and legacy Compose paths require the app container to be
running and pass its own healthcheck; Compose `--wait` alone can accept a
container without a healthcheck. Only the app is recreated (`--no-deps` and
`--no-build`), leaving database and Redis containers alone.
The updater requires a single app container; replicas need the separately
reviewed multi-node rollout procedure rather than checking only one replica.

Automatic app rollback requires `--rollback-compatible`: the operator must
first verify that the target release's database migrations remain compatible
with the previous app. Without that declaration, failure stops and reports the
old image and backup location for investigation. With it, a private temporary
Compose file appended after all operator files selects the old image ID,
including deployments with hard-coded images or additional overrides. On
Compose versions supporting `--pull`, rollback uses `--pull never`. If the
effective service already declares `pull_policy`, the rollback file overrides
it with `never`, including early Compose v2 releases without the CLI flag.
Legacy Compose without that field uses the retained local image ID.
Rollback must pass the same app
healthcheck. The overall update still exits nonzero after successful rollback.
No rollback changes the configured target image in the operator's files: pin
those files to the chosen recovery version before a later ordinary Compose up.

`--prepare` remains a pull-only operation and does not recreate the app or
perform a database backup. Local-build mode continues to delegate to
`deploy.sh` and is outside this Compose safety contract.

## Rationale and limits

This is a bounded operational guard, not a claim of transactional upgrades.
The backup must be retained off-host and its restore command exercised by the
deployment owner. Listing an archive is not a full restore drill. The updater
cannot infer migration reversibility, so it fails closed on backup failure
and never automatically restores the database or undoes schema changes.

For recovery, first block application writes and preserve the failed database
state for investigation. Check migration compatibility before selecting an app
image. If database restoration is necessary, restore the verified archive to a
fresh, isolated database with `pg_restore --exit-on-error --no-owner --no-acl`,
validate the restored app there, and switch the deployment only after choosing
how to reconcile writes made after the backup. Restoring over the live database
can discard those writes; the updater intentionally does not automate it.

## Verification

`tests/update_compose_safety_test.sh` uses a stateful Docker fixture that rejects
unknown invocations and executes the actual in-container dump shell command.
It covers immutable-image rollback, hard-coded images with multiple Compose
files and spaces in paths, migration compatibility opt-in, absent/failing
healthchecks, failed rollback, legacy polling, private backup permissions,
same-second uniqueness, archive validation, host/container database settings,
floating-tag rejection, prepare-only behavior, and pull/backup failure ordering.
CI additionally renders the generated rollback file with real Compose to
verify that the previous image wins and both operator/base settings survive.
The local fixtures are command-contract tests; this macOS review environment has no
Docker/Compose CLI or daemon, so a live container upgrade/restore drill was not
performed. `bash -n update.sh` and `git diff --check` pass locally.

## Follow-up

Issue #255 remains open for the broader admin-mutation audit event inventory,
retry/reconciliation policy, and a separately reviewed production migration
rollback runbook. This change does not alter upstream repositories.
