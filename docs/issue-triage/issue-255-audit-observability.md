# Issue #255: audit persistence failure visibility and wiring

Date: 2026-09-13. Decision: Accepted for development. P1 / Security / Medium / M.

## Why this is needed

The HTTP/PostgreSQL acceptance in #395 proves the current persistence path and
its two-second timeout. The earlier [audit design](admin-audit-design.md)
also required failure counters, alerts, an explicit unavailable state for
database-free development and startup rejection if a configured database has
no audit writer. Those requirements remain unimplemented in the accepted
writer: failure and timeout only emit tracing warnings.

Keep the already-applied business response unchanged. Add the missing bounded
metrics and operational alert so failures are visible even if an administrator
sees HTTP 200; do not equate an alert or counter with retry, an outbox, a
reconciliation journal or recovery of missing audit events.

## Contract

- `aether_gateway_admin_audit_persist_attempts_total`: each invocation of the
  persistence boundary, including sensitive reads and idempotent replays.
- `aether_gateway_admin_audit_persist_failures_total`: failed invocations,
  including writer errors, missing writer and timeout.
- `aether_gateway_admin_audit_persist_timeouts_total`: the timeout subset of
  failures; a timed-out SQL future does not establish that PostgreSQL rolled
  back, and late commits do not erase the observation.
- `aether_gateway_durable_admin_audit_available`: whether this Gateway's data
  backend has an audit writer configured, not whether the database is healthy,
  an event committed or the delivery is at least once.

Counters belong to the Gateway AppState and are shared by its request clones;
no event IDs, users, routes, request contents or raw errors become labels.
They are rendered through the existing protected Prometheus endpoint without
waiting for expensive database health snapshot refresh. As normal process
counters, they reset on restart and cannot establish durable delivery.

Enabled database configuration without an installed audit writer fails during
data-state construction. Explicitly database-free configurations remain valid
and expose the unavailable gauge; there is no new runtime policy toggle.

Alert on any observed per-instance failure increase without requiring a
minimum request volume, a ratio threshold or a sustained `for` period. An
administrator can perform only one mutation during an outage, so a volume
threshold would suppress the event this alert must report. Timeouts already
contribute to failures and do not generate a duplicate alert. The operational
runbook must preserve affected-event evidence and prevent automatic replay of
business mutations.

## Implementation and acceptance

The main thread owns Rust metrics, the persistence integration, startup
wiring, and real HTTP/PostgreSQL assertions. A bounded `terra_worker` owns
only the Prometheus rule, its lifecycle fixtures and the runbook section;
ordinary rule/test work can run independently of the Rust implementation.
The main thread integrates and independently reviews all delivered changes.

Reuse #395's real SQL rejection and advisory-lock timeout to verify counters
through the protected metrics HTTP endpoint. Existing no-database tests prove
the explicit unavailable state. Promtool fixtures cover sparse single errors,
timeouts, successful traffic, resolution, restarts and instance isolation.
Required checks and independent review precede protected merge. #255 remains
open for the complete mutation inventory and strict durability requirements.

The post-merge automated review on #395 identified that a five-second outer
wait did not constrain the advertised two-second persistence timeout. This
slice tightens the same real-lock test: once PostgreSQL confirms the INSERT
is blocked, the response must arrive within 2.8 seconds, and the total request
duration must be within 1.8 to 2.8 seconds. This leaves scheduling margin while rejecting
premature abandonment and a three-or-more-second timeout regression. Timeout
metrics must increment before lock release and remain incremented after any
legitimate late commit. No extra constant-only assertion substitutes for the
observed HTTP/database behavior.

## Validation and integration

- The final socket-only PostgreSQL runner executed the exact HTTP audit target:
  one passed, zero failed/ignored, 2.51 seconds. It includes the stricter timeout
  assertion, six observed persistence attempts, two failures and one timeout.
  The writer capability stays one through SQL failure; a possible late commit
  does not erase the timeout. The fixture PostgreSQL shutdown completed.
- Seven ordinary audit regressions passed, including the new no-database
  writer failure across AppState clones. The live target is ignored in that
  ordinary run and separately executed by the exact runner above.
- Sixteen operational authorization and 34 AppState core regressions passed.
  Existing stale/initial snapshot tests retain their prompt-return assertions
  and now include the four live audit signals alongside the cached/fallback
  sample; the tests do not expect a stale-only exposition anymore.
- Prometheus 3.14.0 `promtool check metrics` accepted the four families extracted
  from the real protected HTTP response. `check rules` accepted six rules;
  all ten rule test groups passed, including five new audit groups. These
  parser/rule tests do not claim delivery to a production Alertmanager receiver.
- Gateway all-features/all-targets strict Clippy, workspace format and diff
  whitespace checks passed. Independent review accepted runtime wiring,
  failure semantics, sparse-operation alerts and the tightened timeout test.

The branch integrated #395's actual protected main commit `544feec1737b65305b99de901013cb8671c3ea93`
with a normal merge. The original #395 head and that squash commit had identical
trees; no feature changes were discarded. Final hosted checks remain required
before protected merge. Parent #255 remains In progress.
