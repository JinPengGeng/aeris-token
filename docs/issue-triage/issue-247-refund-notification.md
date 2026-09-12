# Issue #247: refund status notification slice

Snapshot: 2026-09-13, fork `JinPengGeng/aeris-token`.

## Decision

Issue #247 identifies a real product gap: refund completion and failure are
durable state transitions, but the user currently has to poll an admin-facing
view to learn the result. This slice wires only the two terminal transitions:

- `processing -> succeeded` (`/api/admin/wallets/{wallet}/refunds/{refund}/complete`)
- `pending_approval|approved|processing -> failed` (`.../fail`)

Each transition uses the existing user-email notification dispatcher and the
new `user_refund_status` item. The item is enabled by default, scoped to email,
and requires the existing `user_email_enabled` preference. No email is sent
for API-key or orphaned wallets, users without an address, disabled items, or
missing SMTP configuration.

## Reliability and privacy

Delivery is best-effort and never changes the already-committed refund result.
The mutation repository accepts a terminal retry as an idempotent read, while
the handlers dispatch only on a non-terminal-to-terminal transition. Thus a
normal retry does not send a duplicate message. The repository row lock
serializes concurrent state changes; a caller that began before another
completion can still have a stale pre-state and should be treated as a
best-effort duplicate risk. A future durable outbox/notification claim is the
follow-up required for strict cross-instance at-most-once delivery and retry
after a mailer outage.

Templates receive only the refund number, amount, status, and bounded failure
reason. The notification renderer escapes HTML and the failure path redacts
dispatcher errors in logs.

## Acceptance evidence

- Default item exists in both the runtime fallback and admin system defaults.
- Unit test bounds unknown status values to `updated` and covers whitespace and
  case normalization.
- `cargo fmt --all -- --check`, `git diff --check`, and the focused gateway
  test suite are required before merge.

Remaining #247 work is intentionally separate: final OpenAI/Claude balance
denial status/code/retry-header contract and low-balance threshold producers.
