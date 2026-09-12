# Strict Redis recovery test evidence

Decision for fork issue [#348](https://github.com/JinPengGeng/aeris-token/issues/348),
child of [#223](https://github.com/JinPengGeng/aeris-token/issues/223).

The runtime-state test harness previously returned `None` when an isolated
Redis process could not start or its runtime connection failed. Several tests
then returned success without running their Redis assertions. A green workspace
test job did not establish that redrive, retention, concurrency or reconnection
had been exercised.

## Decision and scope

Use the existing local PostgreSQL harness convention: an explicit
`AETHER_REQUIRE_LOCAL_REDIS_TESTS=1` makes Redis fixture failures fatal. Values
`true`, `yes` and `on` also enable it, case-insensitively. Without the flag, local
tests retain an optional skip with a reason and the flag needed to require the
fixture. Port allocation, directory creation, process launch, startup readiness
and runtime connection errors all use the same required/optional result policy.

This changes test-only code and CI. It does not alter production Redis data,
retention, redrive markers, billing behavior or the default deployment profile.
The test processes bind to loopback, use temporary directories and are killed
and reaped when their fixtures drop. Startup reports early process exit and
server logs, and the readiness deadline also bounds a stalled PING exchange.

`Test (Workspace Rest)` installs Redis and runs
`tools/ci/run_redis_live_tests.sh` with strict mode enabled. The harness:

1. Records the revision, Rust/Cargo versions, Redis version and strict mode.
2. Runs an exact real test with a nonexistent Redis binary and requires one
   failed test with the expected fixture error, so a build failure or empty
   test selection cannot pass as the negative check.
3. Repeats the strict failure check with the system `false` binary to cover a
   process that launches but cannot serve Redis.
4. Verifies the same missing binary produces a reported optional local skip
   when strict mode is disabled.
5. Runs every non-ignored runtime-state library test serially with `--nocapture`,
   including the shared memory/Redis contracts whose names do not contain `redis`. A
   strict connection-failure test uses an owned non-Redis listener to prove
   runtime initialization errors follow the fatal policy.

The existing ignored `redis_usage_cleanup_large_window_timing` performance
benchmark remains outside this recovery gate. The harness requires that named
benchmark to be the only ignored test; adding another ignore makes CI fail.

The generic workspace nextest command excludes `aether-runtime-state` because
the dedicated strict invocation already covers its normal test suite. All other
workspace packages retain their existing execution. The existing `Test` and
`Rust CI / check` dependency chain requires this job to pass.

The CI harness rejects an inherited `AETHER_TEST_REDIS_URL`; it only allows
isolated test processes. Local direct Cargo tests retain their existing explicit
external-URL support, but that path is not used for CI recovery evidence.

## Evidence and local reproduction

```bash
# redis-server must be installed; no background Redis service is needed.
bash tools/ci/run_redis_live_tests.sh
```

Optionally set `AETHER_REDIS_SERVER_BIN` to an explicit server binary and
`AETHER_REDIS_TEST_EVIDENCE_DIR` to an output directory. The default output is
`artifacts/redis-runtime/`. The `redis-runtime-tests` CI artifact retains the
environment, negative checks, optional skip and full test output for 14 days,
including logs from failed test runs. Review the named redrive/retention,
concurrent stream/usage and connection recovery tests in `runtime-tests.log`.

Correction to the parent audit: `tests/redis_durable_crash_drill.sh` was already
run by `Shell security fixtures` before this change. It remains the existing
isolated Compose/AOF kill-and-restart drill, now in a separately logged step in
that same required job. Its `redis-durable-crash-drill` artifact retains
`crash-drill.log` for 14 days, including the AOF write status, post-restart DLQ
presence, idempotent redrive results and final stream counts. The drill script
and its pinned Redis image are unchanged.

Native runtime tests cover Redis contracts and reconnect behavior with
persistence disabled; the Compose drill supplies the complementary AOF crash
recovery evidence. Neither proves the remaining parent acceptance for marker
lifetime/capacity, PostgreSQL backup restore, full billing recovery or end-to-end
RPO/RTO. Parent #223 remains open.

Rollback is a revert of this test/CI change. Disabling the strict flag weakens
recovery evidence and should not be used to turn a failing CI run green.

## Implementation validation

On 2026-09-13, macOS with Rust/Cargo 1.95.0 and Redis 8.10.1:

- `bash tools/ci/run_redis_live_tests.sh`: passed; both strict negative probes
  failed the selected test for the expected fixture reason; optional local skip
  passed with its diagnostic; the full suite passed 123 tests, with only the
  pre-existing timing benchmark ignored. The real Redis suite finished in
  16.04 seconds, including redrive/retention, concurrency and restart recovery.
- `cargo clippy -p aether-runtime-state --all-targets -- -D warnings`: passed.
- `cargo fmt -p aether-runtime-state --check`, shell syntax, workflow YAML
  parsing, `test-automation-contracts.cjs`, and `git diff --check`: passed.

Docker was not installed in that local environment, so the unchanged Compose
crash drill must be verified through the required CI job and its artifact
before merge. CI also validates the Ubuntu-provided Redis version separately
from the local Homebrew Redis version.
