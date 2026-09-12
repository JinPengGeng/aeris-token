# Issue #214: exercise PostgreSQL session deadlines in required CI

## Assessment and decision

The current PostgreSQL adapter already configures request connections with a
30-second statement timeout and a 3-second lock timeout. The connection used by
migrations temporarily disables both limits and is closed on drop, so its
settings do not return to the request pool. The old claim that all PostgreSQL
connections disable timeouts is inaccurate.

An existing ignored live test verifies the implementation against PostgreSQL,
but the required `Data DB Live (selected ignored tests)` job did not select it.
Accept this small, low-risk part of the P1 availability issue: add that exact test
to the existing serial harness. Reuse the existing disposable PostgreSQL service
and test instead of creating another job or changing runtime timeout behavior.

## Acceptance and evidence

The selected test is
`pool::tests::live_session_deadlines_rollback_transactions_and_isolate_migration_overrides`
in package `aether-data-postgres`. It must execute as one passing test; an exact
filter that runs zero tests is not acceptance evidence.

With a two-connection pool, the fixture covers:

- Contended row locks produce SQLSTATE `55P03` within two seconds, and the whole
  transaction rolls back its earlier insert.
- A sleeping statement produces SQLSTATE `57014` with the configured deadline.
- A transaction-local longer deadline permits the expected operation.
- The dedicated migration connection permits an operation beyond the request
  deadline, then is discarded; subsequent pooled work retains its original
  statement timeout.

The test creates a unique table and drops it on success. The harness requires an
explicit disposable database URL and rejects conflicting legacy URL settings.
Local verification on 2026-09-13 used Rust 1.95.0 and PostgreSQL 17.11 in a
private Unix-socket-only cluster; no user or production database was used. The
complete harness passed all five selected tests, each reporting exactly one
passed test and no failures or ignored tests. The newly selected deadline test
finished in 0.47 seconds. Script syntax and `git diff --check` also passed. The
owned PostgreSQL process stopped cleanly, and the local transcript is retained at
`/tmp/aeris214-pg.KsZ6I7/live-tests.log`. Hosted CI uses its existing PostgreSQL 16
service; its results must pass separately before merge.

## Scope, workflow and rollback

Refs #214. Keep the parent issue open: server-side SQL timeouts do not prove
bounded client waits during a TCP black hole, and target admission lifetime and
Redis failure contracts require separate acceptance evidence. This change adds
no schema, application behavior, configuration or dependency.

The PR passes local script/diff checks and the real selected live test, then
review and all four protected required checks before squash merge. Rollback is
removing the added harness target; the existing timeout implementation remains
unchanged.
