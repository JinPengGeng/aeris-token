# Issue #255: administrator audit HTTP acceptance

Date: 2026-09-13. Decision: Accepted for development, P1 / M.

## Evidence and scope

The original no-writer claim was fixed by #294 and the client-disconnect
lifecycle placement by #329. Operational audit endpoints already require
administrator permissions. #376 separately delivered Docker upgrade
guardrails. These completed slices do not close all of parent #255.

The remaining evidence gap addressed here is a real administrator mutation,
through the production request finalizer and PostgreSQL writer, immediately
read back through the protected audit API. Existing unit tests and seeded
read-only records cannot establish that chain. Reuse the operational session
fixture and the existing isolated PostgreSQL runner pattern; add no runtime
API or schema change.

## Acceptance

`tools/ci/run_admin_audit_live_tests.sh` creates a private, socket-only
PostgreSQL instance. Local socket authentication is restricted by the private
directory; TCP listening is disabled and host authentication rejects access.
The test accepts only an explicitly supplied `aether_admin_audit_*` database
with an empty public schema, then runs production bootstrap and migrations.
The runner stops only its own instance and retains its evidence directory.

The exact ignored Gateway target is
`tests::audit::admin_persistence::live_admin_mutations_persist_before_response_and_protected_readback`.
It is explicitly executed by the required Gateway CI job and verifies:

- PostgreSQL-backed administrator and ordinary-user identities and sessions.
- Public HTTP configuration mutation, successful business state and committed
  audit row immediately after the response, with no polling for persistence.
- Protected audit API readback of the same event and joined user identity;
  the sensitive read is itself audited. Anonymous access returns 401 and an
  ordinary user receives 403.
- A rejected mutation records its failed outcome under the same principal.
- Credentials, cookies, query tokens and request-body sentinels are absent
  from the stored audit record.
- Same-ID and conflicting-payload replay leave the original row unchanged.
- A real PostgreSQL INSERT failure preserves the already-applied business
  mutation and its 200 response, without exposing the database error.
- A PostgreSQL advisory lock inside a BEFORE INSERT trigger holds the audit
  writer after the business mutation. The test observes the actual waiting
  backend before requiring the HTTP response to return while the lock remains
  held, exercising the production two-second timeout. It then releases the
  lock and observes the writer finishing.

## Delivery contract and limits

Successful inserts are durable. Write errors and timeouts preserve the
business response and emit a warning; there is no retry queue or outbox. This
is bounded best-effort delivery on failure, not at-least-once delivery.

After a SQL future times out, PostgreSQL may still commit. The test proves
absence only while its BEFORE INSERT lock is held. After releasing the lock,
either zero or one additional event is legitimate; prior events must remain
unchanged, and any late event must retain the correct principal and outcome.
It does not falsely treat timeout as a guaranteed rollback.

Durable retry/reconciliation, cross-store mutation guarantees, the remaining
forensic UI integration and dangerous-operation confirmation work stay in
parent #255. This acceptance slice neither implements nor claims those.

## Validation record

The initial real-database run compiled successfully, then failed at the
username join: the default `AppState` test stores intercepted user/session
creation, leaving no matching PostgreSQL user. The preserved database confirmed
both the mutation and sensitive-read audit rows existed, but the user join
was empty. The fixture now disables both in-memory auth stores and uses the
operational helper to persist users and sessions in PostgreSQL. It independently
asserts the sensitive read
instead of counting it as a mutation. The failed database is retained; no
existing data is cleared to rerun the test.

Final isolated-runner results and independent review are pending.
