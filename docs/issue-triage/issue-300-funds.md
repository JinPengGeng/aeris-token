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
  `authorized_cost_units`, a non-secret `pricing_snapshot` (at most 64 KiB), and
  server-assigned admission time. The same admission timestamp selects eligible
  entitlements (`starts_at <= admission < expires_at`) and their reset-timezone
  usage date. An active entitlement valid only at processing time is ineligible.
  Repeating the same identity and quote returns the original result; altered owner
  or quote is a conflict. No quote-increase API is implemented; callers must
  authorize the full supported execution before dispatch.
- `mark_request_funds_dispatched(RequestFundsIdentity)` fences release before
  upstream dispatch. It is idempotent and refuses a released reservation.
- `release_request_funds(ReleaseRequestFundsInput)` releases only prepared work or
  dispatched work accompanied by an explicit terminal no-charge fact. A wall-clock
  timeout alone is never sufficient to release dispatched funds.
- `finalize_request_funds(FinalizeRequestFundsInput)` accepts the reservation
  identity and the existing `UsageSettlementInput`. It requires a persisted usage
  row and atomically consumes frozen sources, writes the normal settlement snapshot,
  updates provider accounting, and terminates the hold. Replays return the original
  financial result; conflicting actual facts fail without mutation. Optional
  non-secret `reconciliation_facts` are bounded to a 16 KiB object and persisted
  even for an actual amount within the quote. Terminal reconciliation releases
  unused holds while preserving the discrepancy for review.
- `recover_insufficient_quota(RecoverInsufficientQuotaInput)` is an explicit,
  idempotent recovery operation over frozen persisted cost and existing debit
  evidence. It never reruns current pricing or duplicates entitlement deductions.

Amounts use checked integer units at 100,000,000 units per USD. Available balances
are rounded down; authorization upper bounds round up. Non-finite values and range
overflow are rejected. The maximum is `(1 << 52) - 1` units, matching SQL checks;
this permits eight-decimal round trips through existing f64 wallet storage.
The caller must reject any upstream quote exceeding this bound.

The reservation outcome boxes its stored reservation to keep the common rejection
variants small without changing the serialized contract. Integer conversion uses
checked decimal arithmetic and the standard divisibility predicate; neither change
relaxes the financial bounds to satisfy lint checks.

## Persistence and lock order

`request_fund_reservations` stores the immutable identity, quote, financial outcome
and prepared/dispatched/settled/released/reconciliation_pending state. Frozen
funding allocations are stored with the reservation, including wallet bucket and
entitlement/date. An index supports querying active wallet and entitlement holds.
Recovery receipts preserve previously collected amounts and remaining liabilities.

Operations involving existing usage lock that usage first, then the wallet,
entitlements in deterministic order, and the reservation. Admission never acquires
a usage lock while holding financial rows. Refund and negative adjustment hold the
same wallet row while checking outstanding holds. All debit and hold changes roll
back together when an operation fails. Future recharge callbacks must enqueue
recovery after credit and must not acquire usage locks while holding wallets;
that callback/worker integration is not implemented here.

The existing plan cost-window reservation is a separate limit. Its token fencing
and transaction conventions are reusable, but its expiring window counters cannot
serve as a wallet ledger. Gateway integration must successfully acquire both
applicable limits before dispatch and eventually reconcile both.

## Required evidence before readiness

The required PostgreSQL live harness selects all six `live_request_funds_*`
tests explicitly, in addition to the existing five exact targets. A green unit
job that skips these ignored fixtures is insufficient. Each invocation must run
one passing test against the disposable hosted PostgreSQL service. Main-thread
review corrected admission eligibility and usage-date calculation to use the same
server-provided timestamp; the live fixture checks valid, not-yet-started and
exactly-expired grants independently of the database's current date, then checks
the persisted ledger and finalization replay after the date boundary.

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

## Admission-time correction and validation

The initial date-only change still selected entitlement validity against database
`NOW()`. Review identified that this could allocate an entitlement that had not
started at admission, or discard one that was valid then. Both SQL validity bounds
now bind the same checked admission timestamp used to calculate the usage date.
The earlier fixed-future-time test has been replaced by a real PostgreSQL test
using the previous UTC day's last second, ensuring it differs from database time.
The test creates an entitlement starting exactly at admission, one starting at
midnight, and one ending exactly at admission. It asserts the selected entitlement
and capacity, next-day selection, then finalizes after midnight and replays the
result after the admitted entitlement is marked expired. The only ledger entry
must be produced by finalize for the originally admitted entitlement and day.

Validation on Rust 1.95.0 and an owned disposable PostgreSQL 17 database:

```sh
cargo test -p aether-data-postgres --all-features \
  settlement::funding::tests::live_request_funds_ --lib -- \
  --include-ignored --nocapture --test-threads=1
```

With `AETHER_TEST_DATABASE_URL` set to that disposable database and a separate
`CARGO_TARGET_DIR`, all four live tests passed (4 passed, 0 failed, 0 ignored).
The database was stopped after validation, with its files retained for recovery.

Required lifecycle integration remains outstanding: gateway admission/dispatch/
terminal calls, retry authorization, prepared crash recovery, dispatched outcome
reconciliation, and recharge-triggered recovery workers. Live CI wiring is included
in this branch and must pass on the final PR head. This data
branch does not complete or close #300/#206 or re-enable unknown paid images.
The in-memory repository implements wallet funding but does not store entitlements;
entitlement evidence here comes from actual PostgreSQL transactions.

## Decimal boundary review and correction

Independent review identified floating-point boundaries shared by ordinary
settlement and reservations. Ordinary recharge/gift subtraction now uses integer
units in both PostgreSQL and memory, preserving another request's complete hold.
The ordinary entitlement allocator also tracks capacity, deductions and the split
between entitlement and wallet in units. This fixes the real mixed-payment case:
USD 0.40 total minus USD 0.10 entitlement must consume exactly USD 0.30 of wallet
capacity, not reject it after rounding the floating remainder upward.

Live schema inspection corrected part of the initial review hypothesis:
`wallets.balance`, `wallets.gift_balance` and
`entitlement_usage_ledgers.amount_usd` are PostgreSQL `NUMERIC(20,8)` in the real
migrations, despite their logical float64 representation. Therefore a pure
floating-SUM reproduction is not evidence that PostgreSQL's stored ledger sum
invented debt. SQL SUM was already exact, and persisted wallet scale normalizes
the proposed USD 0.30 minus USD 0.10 writeback example. The memory hold failure and
Rust mixed-payment subtraction remain valid concerns. The final SQL converts
the existing exact NUMERIC sum directly into integer units; the regression
proves the resulting contract without claiming that the prior SQL SUM was float.

PostgreSQL now permits reservations against explicitly unlimited wallets with
historical negative recharge balances, consistent with memory and existing
postpaid accounting. Finite wallets continue to reject new paid reservations
while recharge is negative.

The two added live fixtures cover recharge and gift holds coexisting with ordinary
settlement, mixed entitlement/wallet funding, finite-to-unlimited transitions,
sequential USD 0.10/0.20/0.30 entitlement use and recovery after two historical
grant payments. Replay assertions verify that none of these actions duplicates
collection. A memory regression covers both buckets and negative postpaid balance.

Final local verification of the reviewed data change used the required harness
itself: all eleven exact targets passed against the disposable PostgreSQL 17
database (five existing targets plus six funds targets; each ran one test).
Twenty settlement memory tests and the two currency conversion contract tests
also passed. A first run of the new mixed-payment assertion used a Rust f64 decoder
for a NUMERIC column and correctly failed; its query now explicitly casts the
asserted value, matching the adapter's established decoding contract.

Usage-row retention remains a required gateway/recovery integration dependency:
cleanup must retain rows needed by unresolved holds or partially collected debt,
or move equivalent authoritative facts into durable recovery storage first. This
branch does not claim that existing time-based usage cleanup meets that contract.

Final independent review accepted the integer mixed-payment fix and the schema
correction at PR #362 head `9e32e2d7`. Hosted Test (Data) then caught a stale
explicit migration-version fixture: the runtime correctly included the new
`20260913010000` reservation migration, but the expected pending list omitted it.
The fixture is updated to retain its complete ordered-list assertion; no runtime
migration or assertion is removed to satisfy CI.
The complete local `aether-data --all-features --lib` suite passed afterward:
363 passed, 0 failed, 1 explicitly ignored. The separate required PostgreSQL live
harness supplies the previously documented database execution evidence.

## Refund and adjustment boundary correction

Independent review of head `2d5abae6` found one remaining decimal boundary in
refunds and negative administrator adjustments. A USD 0.30 recharge balance with
a USD 0.20 request hold must permit a USD 0.10 refund. The debit paths still
calculated the remainder as `0.19999999999999998` before invoking the new hold
guard, which converted that decimal to 19,999,999 units and incorrectly rejected
the refund. NUMERIC writeback could not repair this case because the guard ran
before the write. The new live regression reproduced exactly that rejection
against an owned PostgreSQL 17.11 database before the implementation was changed.

Refunds now subtract the already persisted refund amount from the recharge bucket
in canonical units. Negative adjustments use the same unit subtraction for both
positive buckets, preserving the requested bucket priority and existing explicit
administrator debt behavior after those buckets are exhausted. Adjustment inputs
are first converted with PostgreSQL's existing NUMERIC(20,8) scale, matching the
ledger's persisted amount; this avoids introducing request-authorization round-up
semantics for sub-unit administrator inputs. The hold guard remains strict, with
no epsilon or rounding relaxation that could spend a reserved unit.

The existing required decimal-hold live target now covers successful refunds,
recharge adjustments and gift adjustments that leave exactly USD 0.20, including
an administrator input that rounds to the ledger's eight-decimal scale. Every
path rejects taking one unit beyond available funds before the allowed debit,
then rejects taking one unit from the remaining hold afterward. Assertions check
both wallet buckets, refund status and transaction counts after failed mutations;
the original held request then settles all 20,000,000 units exactly once. A
separate assertion preserves administrator adjustment spillover and explicit debt.

Validation after the correction: all eleven exact targets in
`tools/ci/run_postgres_live_tests.sh` passed against the disposable PostgreSQL 17.11
database on Rust 1.95.0, each with one executed, non-skipped test. All twenty
settlement memory tests also passed, including the existing recharge/gift boundary
and postpaid debt regression. The memory wallet adapter does not implement admin
refund or adjustment mutations, so these public mutation entry points are verified
against PostgreSQL rather than claimed as memory-adapter behavior. An initial
local harness invocation encountered a stale contracts artifact in the reused
build cache; removing only that crate's generated build artifacts and rebuilding
resolved the missing-symbol errors without source changes or relaxed checks.
PostgreSQL adapter all-features/all-targets Clippy with warnings denied, changed
Rust file formatting and `git diff --check` also passed. The isolated database was
stopped after validation; its files were retained and no application data was used.

## Rollback

Disable new reservations before reverting application code. Drain or explicitly
reconcile every active hold before removing reservation-aware debit protection.
Additive tables must be retained while any active holds or recovery receipts exist;
dropping them is not an automatic downgrade step.
