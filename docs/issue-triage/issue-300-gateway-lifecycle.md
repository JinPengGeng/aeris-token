# Issue 300 gateway lifecycle decision

Status: blocked on the request-funds settlement contract in PR #362.

As of 2026-09-13, `origin/main` has no gateway call sites for request-funds
reservation. The existing `plan_usage_policy` reservation is a separate,
expiring usage-limit ledger and cannot represent wallet or entitlement holds.
Gateway integration must therefore be compiled against the contracts introduced
by #362 rather than reusing that ledger or inventing a parallel API.

## Compatibility boundary

The integration branch should accept a server-issued reservation token and a
frozen, non-secret quote snapshot at admission. Before upstream dispatch it
calls `mark_request_funds_dispatched(identity)`. Every sync and stream terminal
path then calls exactly one of:

* `finalize_request_funds(identity, usage_settlement, reconciliation_facts)`
  when actual usage is known;
* `release_request_funds(identity, terminal_no_charge_fact)` for a proven
  no-charge terminal outcome;
* neither operation for an unknown dispatched outcome. The dispatched hold is
  retained for reconciliation.

The settlement API is idempotent and identity-fenced, so retries may safely
repeat the same terminal operation. A failed admission must release only a
prepared reservation; a dispatch failure after the dispatch fence must retain
the hold until an explicit no-charge fact or reconciliation recovery exists.

## Dependency decision

PR #362 is currently OPEN and GitHub reports `mergeStateStatus: BLOCKED`; its
gateway-facing contracts are not present on `origin/main`. Do not land gateway
call sites on main before #362 (or an equivalent contract-only compatibility
change) is merged. A follow-up branch may target #362's head and carry focused
sync/stream terminal tests, then rebase onto `origin/main` after the dependency
lands. This keeps the public settlement contract single-sourced and prevents a
temporary adapter from silently treating unknown dispatched requests as
released.
