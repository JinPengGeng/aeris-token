# Request funds: implementation contract for #300

Status: accepted design, implementation in progress. Fork only; refs #300 and #206.
The paid-image unknown-cost rejection remains enabled until gateway integration and
the complete image quote contract pass review. This document does not claim rollout.

## Financial policy

New finite-wallet consumption has zero credit: it cannot decrease either balance
below zero or consume money held for another request. Existing negative recharge
balances remain debts and may be repaired by top-ups; no migration forgives them.
Explicit unlimited wallets retain postpaid accounting. Standalone credentials use
their key wallet and cannot fall back to a user's wallet or entitlement. Normal
users without wallets require entitlement coverage of the complete authorization.

Reservations hold available capacity without pretending a hold is consumption.
Funding order preserves entitlement, recharge, then gift. A reservation freezes
the entitlement and usage date, pricing snapshot, owner and amount. A terminal
settlement debits the exact actual amount up to the authorized ceiling and releases
the unused hold in the same transaction. Above-ceiling actual cost is retained for
reconciliation and never creates an unauthorized debit. Request IDs alone cannot
authorize a financial mutation; a server-issued token and owner must match.

## Contract

The public contracts live in `repository::settlement::funding` and are re-exported
from `repository::settlement`. `SettlementWriteRepository` gains these methods,
with fail-closed unsupported defaults for implementations that do not provide them:

- `reserve_request_funds(ReserveRequestFundsInput)` resolves the current funding
  sources atomically and returns a stored reservation or a structured rejection.
  Input contains identity (token, request ID, user ID, API key ID, standalone flag),
  `authorized_cost_units`, a non-secret `pricing_snapshot`, and admission time.
  Repeating the same identity and quote returns the original result; altered owner
  or quote is a conflict. Increasing a quote requires a distinct explicit API,
  not accidental mutation through replay.
- `mark_request_funds_dispatched(RequestFundsIdentity)` fences release before
  upstream dispatch. It is idempotent and refuses a released reservation.
- `release_request_funds(ReleaseRequestFundsInput)` releases only prepared work or
  dispatched work accompanied by an explicit terminal no-charge fact. A wall-clock
  timeout alone is never sufficient to release dispatched funds.
- `finalize_request_funds(FinalizeRequestFundsInput)` accepts the reservation
  identity and the existing `UsageSettlementInput`. It requires a persisted usage
  row and atomically consumes frozen sources, writes the normal settlement snapshot,
  updates provider accounting, and terminates the hold. Replays return the original
  financial result; conflicting actual facts fail without mutation.
- `recover_insufficient_quota(RecoverInsufficientQuotaInput)` is an explicit,
  idempotent recovery operation over frozen persisted cost and existing debit
  evidence. It never reruns current pricing or duplicates entitlement deductions.

Amounts use checked integer units at 100,000,000 units per USD. Available balances
are rounded down; authorization upper bounds round up. Non-finite values and range
overflow are rejected. Existing wallet storage remains compatible in this change.

## Persistence and lock order

`request_fund_reservations` stores the immutable identity, quote, financial outcome
and prepared/dispatched/settled/released/reconciliation_pending state. Frozen
funding allocations are stored with the reservation, including wallet bucket and
entitlement/date. An index supports querying active wallet and entitlement holds.
Recovery receipts preserve previously collected amounts and remaining liabilities.

Operations involving existing usage lock that usage first, then the wallet,
entitlements in deterministic order, and the reservation. Admission never acquires
a usage lock while holding financial rows. Refund and negative adjustment hold the
same wallet row while checking outstanding holds. Callbacks enqueue recovery after
credit and do not acquire usage locks while holding wallets. All debit and hold
changes roll back together when an operation fails.

The existing plan cost-window reservation is a separate limit. Its token fencing
and transaction conventions are reusable, but its expiring window counters cannot
serve as a wallet ledger. Gateway integration must successfully acquire both
applicable limits before dispatch and eventually reconcile both.

## Required evidence before readiness

- Two PostgreSQL connections competing to reserve two USD 0.08 requests against
  one USD 0.10 wallet admit exactly one request.
- Identity and terminal replay are idempotent; conflicts and SQL failures preserve
  both wallet state and holds.
- Normal settlement, refund and negative adjustment cannot consume held funds.
- Frozen entitlement/day allocation survives midnight and settlement delay.
- Partial actual cost settles once and releases only the unused amount; unknown
  dispatched outcomes retain their hold for reconciliation.
- Insufficient-quota recovery subtracts existing entitlement and collection
  receipts and never recreates negative balances or re-prices a historical request.
- PostgreSQL and in-memory contract behavior agree; migration source generation,
  bootstrap, focused tests, real database tests and required CI pass.

## Rollback

Disable new reservations before reverting application code. Drain or explicitly
reconcile every active hold before removing reservation-aware debit protection.
Additive tables must be retained while any active holds or recovery receipts exist;
dropping them is not an automatic downgrade step.
