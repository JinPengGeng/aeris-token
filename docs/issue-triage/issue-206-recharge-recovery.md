# Recharge-triggered historical debt recovery

The user selected automatic collection on 2026-09-17 and requested concrete
limits, retry timing, and notifications. This is the implementation contract;
it is not a claim that the worker is already implemented or deployed.

## Collection policy

- A committed recharge creates one durable recovery job. Payment callbacks,
  retries, and worker restarts must not create additional budgets for the same
  credited payment. Pending, failed, refunded, gift-only, and plan-purchase
  records do not authorize a recharge recovery budget.
- The cumulative collection limit for a recharge is 100% of its actually
  credited principal. Each debit is bounded by the remaining job budget, the
  wallet's currently available recharge principal after holds, and the
  evidenced outstanding historical debt. There is no arbitrary fixed USD cap.
- Collect the oldest eligible debts first, using a stable request-ID tie break.
  Partial repayment is allowed. A USD 10 recharge against USD 15 of eligible
  debt can collect at most USD 10 and leaves USD 5 outstanding.
- Only legacy `insufficient_quota` records with complete frozen pricing
  evidence qualify. Preserve historical prices, entitlement offsets, and prior
  collection receipts. Never substitute the current catalog price or forgive
  the unpaid remainder. Attempt-funded Unknown or reconciliation obligations
  do not enter this legacy debt path.
- Bind each debit to the original debt owner, expected wallet, recharge job,
  and payment identity. Never spend gift balances, another wallet's money,
  reserved funds, or an overdraft allowance to collect this debt.
- Stop the current recharge job when its budget or available principal is
  exhausted. A residual debt waits for the next committed recharge; unused
  budgets from old jobs must not accumulate and consume later recharges.
- Activation applies to newly committed credits after a durable activation
  boundary. Deployment must not replay historical already-credited payments as
  new authorization. Records committed across worker downtime must still be
  discovered after restart.
- At deferred credit confirmation, persist the set of currently visible legacy
  `insufficient_quota` requests for this wallet and owner. This immutable set is
  the authorization boundary: an old pending request that fails later, or a
  backdated debt inserted later, waits for a subsequent recharge. Timestamps
  order eligible debts; transaction-start timestamps do not establish credit
  or debt commit order. Duplicate callbacks cannot expand the candidate set.

## Execution and retry policy

Persist the job with the credit transaction, or discover it from equivalent
durable credit evidence with an explicit activation boundary. Do not call the
debt settlement function while holding the payment callback's wallet lock.

Collection, its receipt, the job's consumed budget, and user-visible wallet
history must commit atomically. Concurrent workers must serialize the same
job, debt, and wallet. A lost response or crash after commit must not charge
again when the operation is replayed. Current low-level recovery returns a
cumulative amount; use a verified per-operation delta for job accounting.

Refund creation, processing and failure rollback lock the payment before the
wallet. Scoped recovery claims its expected wallet and API-key locks without
waiting, then revalidates ownership and wallet resolution. Database lock waits
are limited to 500 ms and individual statements to 5 seconds; contention rolls
back before durable retry bookkeeping. These are database-operation limits,
not a guarantee on network or pool acquisition time.

The first attempt is immediate after discovery. Transient technical failures
retry after 1 minute, 5 minutes, 30 minutes, 2 hours, 6 hours, and 24 hours,
measured from the previous failed attempt. Persist attempts and next-due times
so restarts do not reset the retry budget. After those six retries, retain the
job and debt for operator review and raise a durable failure notification.
Insufficient funds wait for a new recharge without repeated polling debits.
Invalid ownership, incomplete pricing evidence, unsupported billing mode, or
changed frozen facts never authorize a debit and require review.

## Notifications

Every debit has a durable wallet-history entry and collection receipt. One
logical summary per recharge reports credited principal, collected amount,
remaining debt, and available principal after collection. An interrupted job
must retain enough state to deliver its final summary after restart.

Use the existing configured user email delivery and administrator notification
channels, with a durable outbox separate from the money transaction. Delivery
failure must not repeat or roll back a committed debit. Deduplicate queued
notifications by job and audience, retain failed delivery for retry/review, and
do not mark disabled or unconfigured channels as successfully delivered. SMTP
delivery cannot promise exactly-once external receipt after a lost acknowledgement;
the stable recharge reference makes any repeated delivery identifiable.

Only a failed attempted delivery consumes a notification retry. Skipped or
unconfigured delivery and expired claims do not exhaust that budget. Pending,
retry, manual-review and unavailable-source rows display unverified balances
as awaiting reconciliation. Missing candidate evidence, changed identity/mode,
or a remaining total outside the exact JSON-integer range requires review;
none is presented as a cleared debt.

## Import and rollback boundary

Wallet JSONL import sets the transaction-local
`aether.recharge_recovery_restore=on` flag through commit so deferred triggers
cannot treat restored historical credit receipts as new authorization. The
setting rolls back or expires with that transaction and does not suppress
concurrent live credits. Raw SQL or data-only restores must likewise suppress
the enqueue trigger while replaying receipts.

Current application JSONL export does not include the complete request-funds
or recharge-recovery ledgers. A full PostgreSQL backup is required to preserve
jobs, candidate membership, operations, receipts and notification state. Keep
these tables together; do not reconstruct spent budgets by replaying credits.
To pause rollout, set `recharge_recovery_activation.enabled=false` and drain
the worker before changing code. Preserve all financial tables on rollback;
disabling new collection is not permission to reverse prior debits or erase
their evidence.

## Required evidence

Use disposable PostgreSQL and synthetic users, payments, and email receivers.
The acceptance set must cover:

1. Real payment credit commits once and creates one budget; replay before and
   after worker execution neither recredits nor recollects.
2. Partial collection, pre-existing principal, gift balance, active holds,
   standalone API-key wallets, and unavailable or mismatched wallets.
3. Oldest-first debt selection, frozen-price preservation, entitlement offsets,
   prior partial collections, and rejection of incomplete/attempt-funded facts.
4. Concurrent workers and normal spending/refunds, atomic rollback, process
   restart, and replay after a committed debit with an unobserved response.
5. Durable retry timing, exhausted retries, no polling debit on insufficient
   funds, and no accumulation of old recharge budgets.
6. A durable activation boundary that excludes historical payments and still
   includes credits committed while the worker is offline.
7. User-visible wallet history and aggregate notification contents, delivery
   failure/retry, and notification replay without any extra financial effects.

The user also authorized synthetic provider receipts for the separate image
billing acceptance. Those fixtures must identify themselves as synthetic and
cover normal, partial, over-ceiling, malformed, and duplicate receipts; no
provider sandbox credentials or external provider charges are needed for this
local acceptance scope.
