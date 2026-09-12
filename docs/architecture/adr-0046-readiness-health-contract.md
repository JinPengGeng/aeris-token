# ADR-0046: Bounded Gateway Readiness and Health Contract

## Status

Accepted for Issue #308. The implementation is intentionally limited to local
dependency reachability; provider availability and business traffic health are
not readiness inputs.

## Decision

- `/_gateway/health` remains the lightweight process/telemetry endpoint and
  keeps its existing `200` response contract.
- `/readyz` is the orchestration endpoint. It returns `200` with
  `{"status":"ready","gate_readiness":true}` only when every configured
  hard dependency is reachable.
- A configured PostgreSQL backend is probed with a side-effect-free `SELECT 1`.
  A configured Redis runtime backend is probed with `PING`. Memory-only
  backends are treated as `disabled`, not as an external dependency.
- Each dependency probe is bounded by a one-second `/readyz` deadline. A
  timeout or connection error is represented as `status: "failed"` and causes
  HTTP `503 Service Unavailable`; error details, URLs, credentials, and network
  topology are never returned.
- The response schema is stable and machine-readable:

  ```json
  {
    "status": "ready|not_ready",
    "component": "aether-gateway",
    "manifest_version": "...",
    "manifest_path": "/.well-known/aether/frontdoor.json",
    "warmup_status": "disabled",
    "gate_readiness": true,
    "dependencies": {
      "database": {"status": "ok|failed|disabled", "required": true},
      "redis": {"status": "ok|failed|disabled", "required": true}
    }
  }
  ```

## Lifecycle and rollback

The process is considered live while the HTTP server can answer
`/_gateway/health`. During startup, `/readyz` remains `503` until configured
dependencies answer. During shutdown the listener drain stops accepting new
connections; deployments should remove the instance from service before
shutdown. Reverting the implementation restores the previous static `/readyz`
handler without changing unrelated routes.

## Test and operations boundary

The no-backend fixture verifies the disabled dependency contract without
production credentials. CI and deployment drills should additionally run with
an isolated PostgreSQL/Redis pair and validate `503` on stopped dependencies.
This change does not add provider probes, cache state, background-worker
supervision, or a readiness override switch.
