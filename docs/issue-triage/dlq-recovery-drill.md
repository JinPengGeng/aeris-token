# Issue #223: Redis DLQ recovery drill

Date: 2026-09-12

## Scope

The bounded-retention and authenticated operator endpoint are already merged
(`#288` and `#292`). This slice closes the remaining executable evidence gap by
running the public `RuntimeQueueStore` redrive path against a real Redis
server. It does not change production retention or redrive semantics.

## Acceptance contract

- Seed more than the configured destination `MAXLEN` in an isolated Redis
  instance.
- Redrive one source entry twice; the first call returns `Redriven`, and the
  retry returns `AlreadyRedriven` with the same destination ID.
- Redrive the remaining entries and verify the source stream is empty.
- Verify the destination stream remains bounded by the configured approximate
  `MAXLEN` (within Redis stream-node slack) and that retained entries contain
  only the server-side replay fields.
- If the local Redis fixture is unavailable, the test reports a skip; CI must
  provide `redis-server` to turn this into executable evidence.

The existing `tests/redis_durable_crash_drill.sh` remains the process-restart
and AOF durability drill from the durable Redis profile. Keeping this test at
the runtime-state layer ensures the API path is covered without requiring a
production gateway or credentials in CI.

## Verification

Run:

```sh
cargo test --locked -p aether-runtime-state redis_dead_letter_redrive -- --nocapture
cargo fmt --all -- --check
git diff --check
```

Record the test output and the CI run on Issue #223/its PR. Do not run the
drill against a production Redis project or volume.
