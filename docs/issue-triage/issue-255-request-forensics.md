# Issue #255: request forensics in the existing detail view

Decision: Accepted for development. P1 / Security / Medium / M. Fork only.

## Revalidated problem

The existing request drawer already shows usage, attempted candidates, selected
metadata, on-demand bodies and sanitized diagnostic exports. The original
claim that the UI only consumes metrics is obsolete. The remaining useful
addition is access to all persisted candidates and the caller's current
authorization policy from this same view.

`RequestAuditBundle.auth_snapshot` resolves current database settings using
the read-time clock. It is not immutable evidence of the original request's
authorization. Its `usage` section is also a complete storage read model that
can include captured bodies and headers. Fetching the complete bundle solely
to populate a policy panel would load more sensitive data than the UI needs.

Reuse the existing candidate timeline and protected narrow reads. Keep request
body capture on demand. Display explicit authorization fields with a clear
current-state label, and do not add raw bundle JSON or bulk export. Restrict
the new privileged view to `authStore.isAdmin`; `canAccessAdmin` includes
`audit_admin` and is not equivalent. Use actual request/user/key IDs, not a
usage-row UUID or a shared trace alias for unrelated lookups.

## Operational read audit

Source review confirmed that `api/ops.rs::authorize_operational_request`
authenticates `/_gateway/audit/*` and checks its role/scope policy, but currently
discards the resolved principal before calling the handler. Management token
usage tracking is not an administrator audit event. These mounted routes do
not run the ordinary proxy finalizer, so attaching a response event alone
would not persist a read audit.

For the existing audit GET routes, retain the verified session or management
principal and record the final response through the existing sanitized audit
builder and two-second persistence boundary. Include authenticated permission
denials; do not invent principals for anonymous/invalid credentials. Preserve
all existing role/scope checks, management-token usage behavior, response
status and no-store policy. Do not audit Prometheus scrapes or add payload,
query, credential, raw error or permission-list fields to persisted metadata.

Reuse `request_lifecycle::run_request_with_usage` for these forensic reads.
Its producer guard is tracked even when queue processing is disabled. After a
principal is verified, continue the bounded read finalizer on disconnect and
keep the producer alive so usage shutdown waits for that continuation. Avoid a
new fire-and-forget worker or queue with an independent shutdown policy. This
does not add retry/outbox delivery or claim full process-crash durability.

## Implementation and acceptance

The main thread owns backend audit integration, real PostgreSQL/HTTP tests,
lifecycle verification, final review and GitHub transitions. A bounded
`terra_worker` owns only frontend implementation and focused frontend tests;
it reuses the completed independent UI gap analysis. The main thread performs
browser verification using isolated contexts and user-facing locators.

- Ordinary audit and operational authorization regressions must retain their
  existing rejection behavior. Real session/token reads and authenticated
  denials must persist identity, target, status and sanitized audit metadata.
- Successful and partially missing forensic results remain usable; no database
  audit error may replace an already produced response. A real blocked INSERT
  must retain the two-second timeout and remain tracked through disconnect.
- The frontend must handle missing IDs/data, 403/404, loading errors, role
  changes, closing the drawer and rapid request changes. Old responses must
  never populate another request's policy. Restricted roles must not initiate
  the privileged policy fetch.
- Unit/component, real database and browser evidence are separate. A mocked
  browser API does not prove backend authorization or persistence.
- Independent review and current-head required checks precede protected merge.

Implementation and validation for this slice are complete locally on top of the
protected #396 merge (`01caaf5ce6606c4955b8c1ee4816bce8265b1d7b`, which contains
the #396 change as ancestor). The backend now audits both GET and HEAD (Axum's
`get` routes accept both methods), including authenticated permission denials;
anonymous and invalid credentials still produce no invented principal. The
owned PostgreSQL runner executed the exact mutation/readback target with
`1 passed / 0 failed / 0 ignored`, including successful and partial reads,
session and management-token reads/denials, 404, HEAD, redaction, failure and
timeout behavior, and a disconnected request held through usage shutdown.
Gateway operational authorization (16 tests), audit regressions (7 passed,
1 ignored), lifecycle tests (9), strict all-target Clippy and formatting/diff
checks passed. The ignored audit target remains intentionally live-only and is
run by `tools/ci/run_admin_audit_live_tests.sh` against a task-owned database.
An isolated Chromium browser run also covered explicit candidate/policy fetches,
actual IDs, role gating, 403/404/retry, missing IDs, stale-response cancellation,
close, desktop and mobile rendering; the API responses in that browser harness
are fixtures, while backend authorization and persistence are covered above.
An independent frontend review found no blocking issue: four focused test files
with 67 tests passed and the diff was clean.

The frontend and backend changes are still uncommitted on the feature worktree;
the next step is independent review, current-head hosted checks, and a protected
fork PR. The broader mutation inventory, historical authorization evidence,
strict durability, dangerous-operation consistency and deployment acceptance
remain part of parent #255.
The broader mutation inventory, historical authorization evidence, strict
durability, dangerous-operation consistency and deployment acceptance remain
part of parent #255.
