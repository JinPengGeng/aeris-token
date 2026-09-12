# Issue #308: readiness completion

Parent: #217. Priority: P1. Decision: implement; the original static readiness
defect was valid, and #327 addressed reachability but did not meet the accepted
deadline, amplification, lifecycle or drill requirements.

## Scope and design

Follow [ADR-0046](../architecture/adr-0046-readiness-health-contract.md). Reuse
AppState's foreground SQL pool, runtime Redis client, task supervisor counters
and actual usage worker metrics. A shared, cancellation-independent probe pair
uses one overall deadline and one-second positive/negative caching. Lifecycle
and local critical-accounting-worker gates bypass the cache. The binary closes
readiness before its configured withdrawal window and existing drain stages.

Preserve `/health` liveness and all unrelated API contracts. Add only safe
static categories to `/readyz`, replacing the misleading constant warmup field
with actual lifecycle state. No provider probes, production credentials or
schema/data mutations are needed for the readiness checks.

The first real dependency-stop drill found an additional in-scope defect:
`/health` used the general proxy handler and blocked on the Redis-backed IP
blacklist after its local cache expired. The static health payload alone did
not make the endpoint dependency-free. Mount the same payload directly as a
public liveness handler, bypassing business admission/IP policy only for
`/health`. Business routes remain unchanged. The failed drill is retained as
evidence and is not counted as acceptance success.

## Acceptance evidence

The actual executable completed the isolated drill on 2026-09-12 at 20:24 UTC,
using PostgreSQL 17.11 and Redis 8.10.1. All 12 phases passed: initial ready,
SQL stop/restart/suspend/resume, Redis stop/restart/suspend/resume, both
dependencies suspended/resumed, and SIGTERM closing before listener drain.
There were 160 completed HTTP observations. Maximum readiness latency was
1.005000 seconds; maximum liveness latency was 0.001146 seconds. The gateway
exited cleanly and the fixture removed only its own disposable data/processes.

Executable SHA-256:
`7beec2580e0d54935a4186e72effd790f93a93ff3cf0294141f4740be6bcdcec`.
The retained local artifact is `aether-readiness-evidence.Zdng6n`, containing
per-request JSON, headers, timings, service logs, `run.txt`, `result.log` and
`cleanup.log`. Rust CI repeats the drill against the PR executable and uploads
the complete `readiness-withdrawal-recovery` artifact for reviewer access.

The automation contract suite passed all 203 tests. Deterministic readiness
tests cover exact deadlines, both failure directions, two simultaneous stalled
dependencies, 128 concurrent pollers, failure caching, client cancellation,
startup/closing, and actual supervisor exit/restart. CI must pass the exact
reviewed head, including the drill, before merge. The parent issue, Project and
delivery TODO are updated by the main integration agent after merge.

## Operations and rollback

Use `/readyz` for readiness and `/health` for liveness; allow startup time for
database preparation. Configure the withdrawal window for readiness polling
and load-balancer propagation; include it in the process termination grace
period. Run `tools/ci/run_readiness_drill.sh` against the reviewed executable,
retain the printed evidence directory and verify its binary SHA-256.

Rollback by reverting this PR and redeploying under the deployment's normal
review process. A zero withdrawal delay preserves fail-closed readiness while
restoring immediate listener drain timing. It does not disable dependency or
worker checks. The isolated drill proves probe-driven withdrawal/recovery,
not production ingress convergence or a complete multi-node deployment (#224).
