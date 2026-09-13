# Issue #255 Docker upgrade safety decision

Date: 2026-09-13

## Decision

The Compose updater now fails closed for unattended rolling image tags
(`:latest` and `:nightly`) unless the operator explicitly passes
`--allow-floating-tag`. Production deployments should set `APP_IMAGE` to an
immutable version or digest.

Every real Compose update creates a PostgreSQL custom-format dump before the
app image is pulled or recreated. The dump is written atomically under
`./backups` by default (override with `--backup-dir`) and receives a metadata
sidecar containing its UTC timestamp, target image and path. A failed or empty
dump stops the update. Deployments without a PostgreSQL service must explicitly
opt into the unsafe path with `--skip-backup`.

The updater records the currently running app image before the change. If
Compose health verification fails, it attempts to recreate the app with that
previous image and verifies health again. This rollback only covers the app
container; database migrations are not automatically reversed and the saved
dump remains the recovery source for database restoration.

`--prepare` remains a pull-only operation and does not recreate the app or
perform a database backup. Local-build mode continues to delegate to
`deploy.sh` and is outside this Compose safety contract.

## Rationale and limits

This is a bounded operational guard, not a claim of transactional upgrades.
The backup must be retained and its restore command must be exercised by the
deployment owner. The updater cannot infer whether a migration is reversible,
so it deliberately fails closed on backup failure and reports that app-image
rollback does not undo schema changes.

## Verification

`tests/update_compose_safety_test.sh` covers failed health checks, legacy
Compose polling, unsafe service arguments, floating-tag rejection, missing
backup services and backup ordering/content. `bash -n update.sh` and
`git diff --check` pass locally.

## Follow-up

Issue #255 remains open for the broader admin-mutation audit event inventory,
retry/reconciliation policy, and a separately reviewed production migration
rollback runbook. This change does not alter upstream repositories.
