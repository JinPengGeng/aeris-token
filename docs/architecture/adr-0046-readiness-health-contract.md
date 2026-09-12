# ADR-0046: Bounded gateway readiness and health contract

## Status and correction

Accepted for #308 (parent #217). The first implementation in #327 only checked
dependency reachability. Its two sequential one-second probes did not enforce
a one-second overall deadline; it had no cache or startup/closing gate. The
previous version of this ADR overstated those guarantees. This decision and
the [completion record](../issue-triage/issue-308-readiness-completion.md)
describe the corrected behavior and its evidence.

## Endpoint and dependency decisions

- `/health` is the process liveness route. Keep its existing
  `200`, `status: "healthy"`, timestamp and compatibility pool fields unchanged.
  Its handler is mounted directly: the former proxy path first checked the
  Redis-backed IP blacklist and could block even though the health payload
  itself was static. A dependency failure must not cause a liveness restart loop.
  This public metadata-only route bypasses business admission and IP policy,
  like `/readyz`; all business APIs retain their original enforcement.
- `/_gateway/health` and `/v1/health` retain their existing telemetry contracts.
  They may read distributed state and are not the liveness probe to configure.
- `/readyz` returns `200` only when startup is complete, the closing gate is
  open, every configured SQL/Redis dependency responds, and the local role's
  critical accounting workers remain supervised and active. Otherwise it
  returns `503`; providers and external model availability are not inputs.
- SQL uses `SELECT 1` on the existing foreground data pool; Redis uses `PING`
  through the existing fast connection lane. This intentionally detects loss
  of foreground DB admission capacity, without reserving another pool or
  allocating a connection per HTTP probe. Unconfigured dependencies are
  `disabled`; a diagnostic memory-only instance may be ready.
- For roles spawning background tasks, the usage queue supervisor is required
  when queueing and its writer/backend are configured. Its supervised handle
  and at least one actual queue worker must be active. The counter flush worker
  is required when its backend exists. Missing/terminated required workers
  fail readiness; a supervised restart can recover without stale failure
  counters making the process permanently unready. Usage admission shutdown
  closes readiness as well.
- Proxy-only roles do not require a local consumer of the shared durable
  queue. Singleton lock contention is an expected standby state, not a failed
  worker. Periodic cleanup, model fetch, provider refresh and backup workers
  are observable but are not traffic-admission requirements. This checks the
  critical supervisor's live state, not business-level throughput or remote
  consumer lag; queue monitoring and alerts remain required.

## Deadline, cache and lifecycle

One process-wide single flight runs at most one SQL and one Redis probe in
parallel. Both use the same monotonic deadline, one second from the initiating
request. SQL pool acquisition is included. Completion at or after that deadline
is a timeout. The flight owns its task so disconnecting a client cannot abandon
a pool operation and trigger another probe. All other requests share that flight;
there is no queue of dependency operations proportional to poller count.

Both successful and failed snapshots are cached for one second after completion.
Thus at most one probe pair starts per second, and dependency withdrawal can lag
by up to the one-second cache plus the one-second flight (excluding executor and
network scheduling). Followers inherit an earlier deadline, never a fresh full
deadline after waiting. Startup, shutdown and required-worker state are read on
every request and after probe completion; cached green cannot reopen a closed
gateway. Closing interrupts requests waiting for a flight immediately.

`AppState::new()` starts in `starting`. The production binary completes database
preparation, pool warmup, bootstrap and required worker registration before
marking startup complete and serving traffic. Before binding there is no listener,
so orchestrators must allow startup time; an embedded router exposed before
`mark_startup_complete` returns `503` with `starting`. `build_router()` explicitly
completes its empty, unconfigured startup; embedded callers using
`build_router_with_state` must complete their own startup.

On SIGINT/SIGTERM the binary irreversibly changes to `closing`, preserves the
HTTP listener for `AETHER_GATEWAY_READINESS_WITHDRAWAL_DELAY_MS` (default 2000,
range 0–60000), then starts the existing HTTP drain and usage drain. During that
window `/readyz` is `503` and `/health` is `200`; existing traffic can finish.
Set the window for the deployment's readiness interval and load-balancer
propagation time. The termination grace period must cover this window plus
both drain deadlines and cleanup. Server exit also closes the gate. Calling
startup completion after closing cannot reopen it.

## Stable public response

Existing envelope fields remain; `warmup_status` now reports actual startup
completion instead of the former constant `disabled`. `lifecycle_status` and
`workers` are additive. No error strings, credentials, pool addresses, task
identifiers or internal topology enter the response.

```json
{
  "status": "ready|not_ready",
  "component": "aether-gateway",
  "manifest_version": "existing manifest version",
  "manifest_path": "/.well-known/aether/frontdoor.json",
  "warmup_status": "starting|complete",
  "lifecycle_status": "starting|running|closing",
  "gate_readiness": false,
  "dependencies": {
    "database": {"status": "ok|failed|timeout|disabled|unchecked", "required": true},
    "redis": {"status": "ok|failed|timeout|disabled|unchecked", "required": true}
  },
  "workers": {
    "usage_queue": {"status": "ok|failed|disabled", "required": true},
    "usage_counter_flush": {"status": "ok|failed|disabled", "required": true}
  }
}
```

`unchecked` means a lifecycle/worker gate prevented dependency work; it does
not assert a backend failed. `failed` means an error, while `timeout` means the
operation missed its deadline. These are bounded static categories.

## Validation and rollback

Virtual-time tests cover each dependency outcome, simultaneous stalls, exact
deadline boundaries, 128 concurrent pollers, negative caching, disconnected
clients, and closing during a flight. HTTP tests cover startup/running/closing
and critical-worker exit/restart without changing liveness. The
[isolated deployment drill](../operations/readiness-drill.md) launches the real
binary and disposable SQL/Redis services, stops and resumes dependencies, and
retains HTTP payloads, timing and cleanup evidence. Rust CI runs that drill and
uploads its evidence as part of the required gateway test job.

Rollback is a normal revert/redeploy of this change. Restore the previous probe
configuration only with an explicit operations decision: the earlier `/readyz`
has sequential deadlines and no worker/lifecycle gate. Setting withdrawal delay
to zero disables only the grace window, never dependency checks or the closing
gate. Do not replace failed readiness with a constant green endpoint.
