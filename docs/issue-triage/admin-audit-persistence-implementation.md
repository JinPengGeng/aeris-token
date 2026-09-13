# #255 durable audit persistence implementation record

Date: 2026-09-12

## Scope

The issue's unauthenticated `/_gateway/audit/*` claim is already disproven by
the operational permission checks. This change addresses the remaining claim:
administrator events emitted by `emit_admin_audit` were only sent to tracing
and did not create rows in `audit_logs`.

## Decision

The data layer now exposes a dedicated `AuditLogWriteRepository` and the
PostgreSQL adapter inserts a strongly typed `CreateAdminAuditLog` record with
`ON CONFLICT (id) DO NOTHING`. The result distinguishes a newly inserted row
from an idempotent duplicate. No schema change or tracing-to-database layer is
introduced.

The gateway finalizer remains synchronous because it is used by many response
paths. It places a redacted record in a response extension; the proxy request's
*lifecycle-owned* future removes that extension and awaits persistence before
returning the response to Hyper. Keeping this continuation inside
`run_request_with_usage` is required for disconnected clients: the lifecycle
wrapper may finish the inner future in a background task after the handler
future is dropped. A bounded two-second timeout prevents a database outage
from holding a request indefinitely. Insert failures and timeouts are reported
with a separate tracing event and preserve the business response, avoiding
client retries of already-applied mutations.

## Data and redaction contract

- `event_type` is the bounded classification `admin_mutation` or
  `admin_sensitive_read`; the exact event name is metadata.
- IDs, route names, action and status are copied from the already-resolved
  control decision. Paths use the existing access-log sanitizer.
- `user_id`, client IP, request/trace ID, HTTP status and a versioned metadata
  object are stored. User agents, credentials, cookies, request bodies and
  response bodies are not stored.
- The event ID is UUIDv7, allowing retries to remain idempotent while keeping
  insertion order useful for operators.

## Verification

Contract validation covers database text bounds and required fields. The
backend composition test asserts that a configured PostgreSQL backend exposes
the audit writer. The request-lifecycle regression test proves that
finalizer work continues after the handler future is dropped, covering the
client-disconnect boundary. `cargo fmt --all` and `git diff --check` pass
locally; the focused Rust test is also required in CI when the workspace
toolchain is available.

## Follow-up

Successful inserts are durable and replaying the same event ID is idempotent.
On write failure or the two-second timeout, the current code only warns and
preserves the already-applied business response; it has no retry queue, outbox
or reconciliation worker. This is bounded best-effort delivery on failure,
not an at-least-once delivery guarantee. A timeout also leaves the commit
result uncertain; it does not establish that no row was inserted.

The HTTP acceptance slice is described in
[the readback decision](issue-255-audit-readback.md). Operations that mutate
multiple stores still need a transaction/outbox design before being advertised
as strictly durable. Parent #255 remains open for those separate requirements.
