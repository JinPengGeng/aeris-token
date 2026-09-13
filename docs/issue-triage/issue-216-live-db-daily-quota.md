# Issue 216: live PostgreSQL daily-quota gate

The required Rust CI live-DB harness now executes
`settlement::tests::live_daily_quota_serializes_each_entitlement_without_locking_shared_plan`.

This test exercises the concurrency boundary that protects each user's
entitlement row while allowing users on a shared billing plan to proceed
independently. It uses the disposable PostgreSQL service and the canonical
`AETHER_TEST_DATABASE_URL` set by `data_db_ignored_postgres`; the harness runs
the test exactly, with `--include-ignored` and one test thread.

The test is intentionally added as a single selected target. The remaining
ignored adapter tests still require separate fixture review before they are
made required CI inputs.

Local reproduction:

```sh
AETHER_TEST_DATABASE_URL=postgres://aether:aether@127.0.0.1:5432/aether_test \
  bash tools/ci/run_postgres_live_tests.sh
```

Refs #216.
