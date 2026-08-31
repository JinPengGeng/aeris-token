# Scheduler multi-instance acceptance status

Issue #47 is **blocked and not accepted** on this branch. The branch provides backend reachability,
test infrastructure, and seven fail-closed E2E entry points. It does not claim the scheduler safety
properties pass until production observation and fault-injection seams from #46/#49/#50/#51/#52/#53
are integrated.

## Backend reachability is not acceptance

`backend_reachability_two_gateways_share_redis_and_postgres` starts two real `GatewayHarness`
instances with independent clients connected to the same Redis and Postgres services. It verifies
cross-client visibility and distinct gateway listeners. Its CI job is named
`Scheduler Backend Reachability (Not Acceptance)` and is not part of the required `test` aggregate.

Every run uses a unique Redis prefix and unique Postgres schema. Redis seed keys have a 60-second
failure-path TTL and are deleted on success. The Postgres schema is dropped explicitly on success;
an RAII guard schedules best-effort cleanup during error unwinding. CI uses a job-specific database,
bounded service resources, and hard job/test timeouts.

## Seven fail-closed E2E entries

`issue47_multi_instance_e2e.rs` exposes one ignored entry for each required scenario:

1. `exactly_one_half_open_probe_e2e`
2. `affinity_order_and_failover_e2e`
3. `pool_stampede_prevention_e2e`
4. `frozen_snapshot_pagination_and_skip_e2e`
5. `send_time_admission_no_send_e2e`
6. `post_client_commit_no_replay_http_and_ws_e2e`
7. `request_wide_attempt_budget_and_deadline_e2e`

Each entry requires real Redis and Postgres, creates a unique SQL schema and Redis namespace, starts
two real gateways, and sends HTTP traffic to an independent counting upstream recorder. The entry
then fails with the exact missing production seam. Baseline network plumbing is not treated as
scenario acceptance. The post-commit entry remains blocked on the WebSocket adapter as well as the
HTTP lifecycle adapter.

The CI job `Issue 47 Multi-Instance Acceptance (Blocked)` is gated by repository variable
`AETHER_ENABLE_ISSUE47_E2E=true`, is not in the required aggregate, and currently fails by design if
enabled. It must only become required after all seven entries consume production events and pass.

## Non-vacuous validator contract

The independent upstream recorder is authoritative for send count and destination. Diagnostic trace
events are explanatory evidence and must agree with recorder output. Every traced send requires an
earlier admission grant and attempt charge for the same request/attempt.
`NetworkObservation` cannot be populated by integration callers; only the recorder can produce a
non-empty snapshot, preventing trace-derived tests from self-certifying network behavior.

- HalfOpen requires one acquisition, at least one contender rejection, lease identity, fencing token,
  and exactly one authoritative send. The production adapter must also assert persisted SQL completion.
- Pagination events require page, real generation, rank, and cursor; current-page generation cannot
  drift and cursors cannot move backwards.
- Client-commit validation requires an actual `ClientCommitted` event before proving no replay.
- Attempt-budget validation requires charge events and evaluates each request independently, including
  credential count, provider switches, total attempts, and deadline.
- Pool checks track credential ownership by request and attempt; another attempt cannot release or
  overlap the owner.

## Production blockers

- #46: HTTP and WebSocket attempt/client-commit lifecycle observation.
- #49: trusted failure origin and retry disposition observation.
- #50: generation/rank/page/cursor production values.
- #51: pre-send barrier and authoritative admission decision.
- #52: distributed probe claim/rejection, lease/fence lifecycle, and SQL completion query.
- #53: request-wide charges, terminal reason, and injectable monotonic deadline clock.

Backend or adapter absence is never converted into a green acceptance result. Missing backend URLs
fail immediately when a reachability or E2E entry is explicitly run.
