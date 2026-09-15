# Issue #255 administrator mutation inventory

Date: 2026-09-15. Scope: inventory and coverage guard only.

## What is covered today

The request finalizer is the mandatory producer boundary for responses. For an
identified administrator principal it records every `POST`, `PUT`, `PATCH`, and
`DELETE` response as `admin_mutation_completed` or `admin_mutation_failed` when
the handler did not attach a domain event. This generic path protects new admin
handlers from silently becoming unaudited at the HTTP-result level.

Handlers that need a business outcome, asynchronous task state, or a sensitive
read attach a bounded `admin_*` event through
`handlers/shared/admin_proxy.rs`. The canonical event list is kept in
`issue-255-admin-mutation-inventory.txt` and checked by
`tests/admin_audit_inventory_test.sh`. The check fails when an explicit event is
added or removed without updating the inventory, so the list remains reviewable
and does not depend on a stale issue comment.

The inventory currently contains 116 explicit event names across the admin
handler tree. The generic fallback is intentionally not duplicated in the list;
its two stable names are asserted directly against the finalizer source.

## Coverage classes

| Class | Producer | Durable event type | Evidence |
| --- | --- | --- | --- |
| Explicit mutation/read event | domain handler via `attach_admin_audit_response` | `admin_mutation` or `admin_sensitive_read` | event inventory + normal handler tests |
| Generic mutation fallback | `finalize_gateway_response` | `admin_mutation` | finalizer unit/HTTP tests |
| Operational forensic read | `api/ops/audit.rs` context | `admin_sensitive_read` | protected readback and denial tests |
| Async terminal outcome | task/provider handler when terminal state is known | `admin_mutation` | provider/video/task terminal tests |

Unauthenticated requests and principals that are not accepted as an admin are
not assigned fabricated audit identities. A failed authenticated admin mutation
is still recorded with its final HTTP status. Sensitive fields remain governed
by the existing typed redaction contract; this inventory does not expand the
metadata allowlist or include request/response bodies.

## Deliberate gaps kept as separate work

This guard does not claim that every multi-store operation has one transaction,
that a timeout is known to have rolled back, or that a missing row is repaired.
The current persistence boundary remains bounded best effort on writer failure;
retry/reconciliation requires an outbox or equivalent durable journal and must
be reviewed as a separate design. Likewise, historical authorization is not
reconstructed from the current policy snapshot, and dangerous-operation
confirmation remains a UI/operation contract rather than an audit event-list
problem.

The inventory test is a CI drift detector, not a substitute for the live
PostgreSQL readback, failure metrics, operational-read UI, or provider terminal
tests already delivered in #395, #396, #397, and #441.
