# Issue #255: administrator audit HTTP acceptance

Date: 2026-09-13. Decision: Accepted for development, P1 / M.

2026-09-17 follow-up: system-config PUT now has atomic audit delivery. See
`issue-255-durable-audit-delivery.md` for its implementation and current acceptance.
The no-outbox statements below describe this earlier checkpoint and the other
mutation families, not the newly migrated system-config PUT path.

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

- PostgreSQL-backed administrator, audit-administrator and ordinary-user
  identities and sessions.
- Public HTTP configuration mutation, successful business state and committed
  audit row immediately after the response, with no polling for persistence.
- Protected audit API readback of the same event and joined user identity;
  the sensitive read is itself audited. Anonymous and ordinary-user access
  return 401 under the existing admin-principal contract. An audit administrator
  attempting full-admin forensic readback receives 403 and the required scope.
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

The first isolated-runner execution then reached the joined-identity and
readback assertions successfully, but failed at the ordinary-user status
expectation (actual 401, expected 403). Source revalidation established that
`resolve_local_admin_principal` deliberately rejects the `user` role before
producing an admin principal. The test now locks that existing exact 401
contract; it separately uses a real `audit_admin` session to prove the 403
full-admin forensic permission boundary. No runtime authorization is changed.

The corrected run then passed those authorization checks and exposed another
fixture assumption: this generic configuration key accepts arbitrary JSON
values, so an object-valued `value` is not rejected by its existing parser.
The failure case now supplies an object-valued `description`, which the
production parser explicitly rejects with 400, and additionally verifies that
the configuration value is unchanged. No configuration validation policy is
altered merely to make the audit test pass.

The final isolated runner executed the exact target successfully:
`1 passed; 0 failed; 0 ignored`, in 2.38 seconds after a 56.42-second incremental
compile. Its private PostgreSQL instance was stopped and the evidence directory
retained. Separate existing regressions passed: six audit tests (the new live
target is intentionally ignored in that ordinary run and separately executed
above) and sixteen operational authorization tests.

`shellcheck`, `actionlint`, whole-workspace rustfmt and the complete PR
whitespace check passed. Gateway all-features/all-targets Clippy with
`-D warnings` passed in 3m29s. Independent review accepted integrated HEAD
`055c051d` and then re-reviewed the final fixture corrections, successful
live runner log and PostgreSQL cleanup evidence without finding a blocker.
Final hosted verification is pending. The runner
also rejects a zero-test result, so moving or renaming the ignored target
cannot silently produce a green live gate.
