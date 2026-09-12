# Issue #268: candidate identity across planner steps

Issue: <https://github.com/JinPengGeng/aeris-token/issues/268>

## Accuracy, value and priority

Revalidated against fork main `5c620e7681cba50451847c3d163076614ddf8e83`.
The report is accurate and remains P1: an HTTP request can succeed while its
only candidate record remains `Skipped`. This damages decision traces and final
candidate status aggregation; it does not mean the upstream response failed.
Implementation complexity is medium because eager planning, lazy pages and
background heartbeat planning must agree on identity. No database migration or
client API change is required.

The existing test named `...after_api_key_concurrency_wait_budget_elapses`
released the slot after three runtime reads, before proving budget exhaustion.
It also delayed the successful upstream response by 100 ms, so its elapsed-time
assertion could pass without reaching the reported defect. The corrected test
waits for an actual persisted `Skipped(auth_api_key_concurrency_limit_reached)`
before releasing the slot. Against the previous implementation it failed with
`left: Skipped, right: Success` after receiving HTTP 200.

## Decision and implementation

A candidate index now identifies a logical candidate observation across the
whole HTTP request, including its successive planner steps. An exhausted step
keeps its terminal `Skipped` observation. A later step allocates a new candidate
index and may independently become `Pending`, `Streaming` or `Success`.
Existing same-key and pool-key retry indices retain their current meaning.
`request_candidate_lifecycle_would_regress` and the PostgreSQL terminal guard
remain unchanged: the first terminal fact for each identity still wins.

The existing request lifecycle owns an atomic index allocator. Eager candidate
materialization and lazy pages reserve ranges before creating candidate rows or
attempts. A final concurrency skip reserves its own range. Reservations do not
depend on database reads, asynchronous queue flushes or persistence mode. Lazy
cursors keep the allocator's `Arc` when moved into another task; the standard
text heartbeat worker explicitly restores the same scope when it creates a
planner in its background task. The request and its remaining cursors own the
allocator, so no global registry, cache expiry or cleanup timer can reset it.

The allocator checks PostgreSQL's signed 32-bit index range. Exhaustion cannot
wrap or repeatedly reuse the final index. Lazy planning returns an internal
error; existing infallible eager materialization entrypoints log the error and
produce no new attempts. This boundary would require more than two billion
candidate observations in one request.

Success already takes precedence over skipped/failed observations in
`derive_request_candidate_final_status`; preserving both rows therefore makes
the request's aggregate status accurate without changing the aggregator.
This also retains distinct failed or skipped observations when other matching
planner steps revisit the same provider. Four existing sync fixtures previously
expected those observations to collapse into one row; they now require both
terminal rows with distinct sequential candidate indices.

## Alternatives considered

| Alternative | Decision |
| --- | --- |
| Allow an auth-limit Skipped to become Success | Rejected: overwrites a real terminal observation and weakens protection from late asynchronous writes. |
| Delay the skip until the whole request finishes | Rejected: hides the exhausted step while another step runs and requires cancellation/finalization buffering. |
| Generate a new candidate UUID | Insufficient: persistence also conflicts on `(request_id, candidate_index, retry_index)` and can return the old row's ID. |
| Read database maximum index before every planner | Insufficient: queued writes need not be visible yet; concurrent cursors can observe the same maximum. It also adds hot-path queries. |
| Reserve a special retry index for concurrency skips | Rejected: overloads retry identity and confuses retry statistics. |
| Accept candidate/request disagreement | Rejected: retains the reported audit and observability defect. |

## Validation

- HTTP regression: keep one request active, observe the other request's actual
  terminal concurrency skip, release the slot, verify HTTP 200 and a separate
  successful candidate. Verify the original skip ID, reason and finish time
  survive and aggregate final status is Success.
- Exhaustion regression: keep the slot occupied through every step; verify the
  existing HTTP 503 runtime-miss response, retained terminal skips and no second
  upstream execution. Auth-limit exhaustion currently follows
  `local_execution_runtime_miss_status(false)`; this change does not redefine
  that HTTP contract.
- Request index tests: different planner handles share reservations, a captured
  lazy handle survives a task move, independent requests start at zero, and
  signed-index exhaustion never reuses a slot.
- Heartbeat regression: reserve indices in the foreground and verify the real
  background planning closure continues the same sequence.
- Existing candidate materialization and retry tests validate pool retries and
  terminal preservation. The general terminal guard remains unchanged.

Local results:

| Group | Result |
| --- | --- |
| `tests::ai_execute::sync` | 56 passed, including real wait exhaustion followed by success and all-step exhaustion |
| `tests::ai_execute::stream` | 20 passed |
| `candidate_materialization` | 14 passed |
| `request_lifecycle` | 9 passed |
| `heartbeat` | 39 passed |
| `candidate_indices` | 4 passed, including the heartbeat propagation regression |

The groups overlap. The final sync group was run through `cargo test -p
aether-gateway --lib tests::ai_execute::sync -- --nocapture`; other groups were
run directly against the same Cargo-built gateway unit-test executable, with
the group name as its filter. All production code was identical across these
runs; the last compilation updates the four existing sync fixtures and the
exhaustion test's assertion to the existing HTTP 503 contract. These checks
used `RUST_MIN_STACK=16777216` for gateway integration coverage: an initial
unconfigured broad heartbeat run overflowed the default test-thread stack in
an existing tunnel heartbeat integration test; the configured run passed all
39 tests. macOS linking emitted the existing large `__eh_frame` unwind-table
warning. No local Clippy or live PostgreSQL result is claimed; CI and final
review remain required before merge.

## Boundaries and rollback

This fixes successive local planner steps belonging to one HTTP request. It
does not define idempotency for independent requests that reuse a trace ID or
allocate identities supplied by a separate remote planner. Historical rows
are not rewritten: a missing historical Success cannot be inferred safely
from the old Skipped row alone.

Rollback is a code revert. The schema and stored status vocabulary are
unchanged, and previously persisted distinct rows remain valid. Reverting the
allocator reintroduces the original collision for newly handled requests.
