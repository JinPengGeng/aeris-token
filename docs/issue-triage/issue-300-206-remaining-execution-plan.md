# #300 / #206 remaining execution contract

Status: planning-only checkpoint for the fork. This document does not change
pricing, wallet policy, database data, or public admission behavior. It records
what is already evidenced at `main@4620ad4e6` and the evidence that is still
required before either issue can be closed.

## Current checkpoint

The bounded image quote, token-bound attempt funds, hard plan quota, daily cost
ledger, retention guard, and Gateway admission/dispatch wiring are present in
the fork history. PR #391 was squash-merged as `acf02288d5c37220313a9aed8ec6c277009252a1`.
Its hosted Data DB Live job exercised 34 exact targets and its Gateway live job
exercised 17 PostgreSQL/HTTP targets; the issue record reports every target as
`1 passed / 0 failed / 0 ignored`, with the required checks green.

The current evidence is limited to the fixed synchronous JSON image boundary.
The Gateway runner is an explicit inventory in
`tools/ci/run_gateway_attempt_funds_live_tests.sh`; its public fixture includes
the four account classes, retries, unknown and late charges, quota races,
wallet revocation, daily limits, and the deliberate stream/unbounded rejection.
The corresponding implementation refuses paid multi-stage or streamed image
execution before upstream dispatch when the request context says streaming or
one of the unsupported provider projections
(`apps/aether-gateway/src/execution_runtime/funded_image.rs`, lines 119-137).
That refusal is a security boundary, not evidence that paid streaming is
implemented.

The parent issues remain open. GitHub currently labels #300 `status:in-progress`
and #206 `status:triage`; the repository delivery queue classifies #206 as a
`P1 | Split` parent (`docs/issue-triage/delivery-todo.md:318`). This is
consistent with the merged PR's stated boundary:
paid streaming/multi-stage work, full crash recovery, recharge recovery, and
real provider billing receipts are outside the merged evidence.

## Remaining work and acceptance contracts

| Track | Current evidence | Required implementation or exercise | External input required |
| --- | --- | --- | --- |
| Paid stream / multi-stage / conversion | The funded image gate returns a 422-style unsupported error before upstream dispatch; the live fixture asserts zero reservation and zero upstream calls for stream and unbounded requests. | Choose one contract: (A) retain fail-closed rejection for every paid stream, multi-stage, and unsupported conversion and publish the exact error/status matrix; or (B) implement a new quote/evidence path. If B is chosen, add per-attempt quote capture, partial-output facts, cancellation/timeout semantics, and provider-specific projection tests before enabling any route. | Product/API decision on whether these modes are billable; maximum outputs, dimensions, quality, formats, partial count, provider capabilities, and public error compatibility. |
| Post-dispatch process crash | Prepared cancellation and in-process drop fallback are tested. A process that dies after `mark dispatched` and before an authoritative result is not represented by a completed staging drill. | Run a disposable staging kill/restart exercise after a real upstream dispatch. Verify the reservation remains `Unknown`/held, no timeout releases it, a later authoritative outcome settles once, replay after outbox cleanup is idempotent, and no parent or provider counter is duplicated. Add a durable recovery lease/worker only if the exercise shows the current operator procedure is insufficient. | Staging Gateway, PostgreSQL and Redis topology; permission to terminate/restart a worker; recovery SLO, lease/attempt limits, alert destination, and an approved unknown-hold escalation policy. |
| Recharge / insufficient-quota recovery | PostgreSQL exposes idempotent `recover_insufficient_quota`; ordinary settlement still treats `insufficient_quota` as terminal. There is no verified payment-commit callback that schedules recovery. | After a recharge is durably committed, enqueue recovery using the frozen historical cost and receipts. Verify lock order (payment/wallet then usage), idempotent retries, no current-price re-evaluation, no duplicate debit, and a durable failed-recovery alert. Keep unpaid evidence retained. | Finance decision: automatic collection versus manual review, retry schedule/backoff, maximum collection, customer notification, and treatment of expired/partially paid debts. |
| Enrichment failure | The runtime now has failure metrics and retry/DLQ-oriented paths in the delivery history, but the billing-integrity queue still requires an owner-signoff test at the current merged head. This is a #206 integrity gate, not a reason to alter image pricing here. | Force pricing-context/enrichment failure through direct, worker, queued, and replay paths. Prove no zero-cost terminal settlement, exactly-once failure metric, retained retry/DLQ evidence, and successful later enrichment without duplicate debit. | Failure-injection environment, retry/DLQ retention and alert SLO, owner for financial reconciliation, and approval of the current cancellation/enrichment billing contract. |
| Signup credit / delivered debt | The delivery queue keeps signup credit and promotion eligibility under #253; #300 must not infer a credit policy from image funding tests. | Audit the account-creation credit decision and its ledger/entitlement effects in a disposable database. Verify default amount, eligibility, replay/idempotency, disabled/referral paths, and that historical debts are not silently forgiven. | Product/finance decision for default credit, eligibility, referrals/promotions, expiry, currency, and remediation of already-created accounts; owner sign-off and a representative fixture set. |
| Real provider receipt / price drift | Tests use a local upstream fixture and frozen quote. They are not evidence for a real provider's billing receipt or revised price. | In a non-production provider account, capture the authoritative output/usage receipt, map it to the frozen quote, and verify below-ceiling collection, above-ceiling reconciliation, malformed/partial receipt retention, and duplicate receipt replay. Do not enable a route from this plan alone. | Provider sandbox credentials, receipt schema and request correlation contract, sandbox rate/cost limits, approved test budget, and finance treatment for price drift. |
| Retention and replay after deployment | Retention predicates and attempt/daily-ledger tests are in the merged history; local fixtures use disposable databases. | Exercise cleanup while an attempt is prepared, dispatched-unknown, reconciliation-pending, or an insufficient-quota debt exists. Confirm obligations and frozen facts survive, then resolve and confirm a later cleanup removes only resolved rows. Replay usage/outbox after cleanup and compare balances and daily contributions. | Staging retention schedule, payload/body retention policy, backup/restore window, and approval to run cleanup against a disposable copy of representative data. |
| Deployment and rollback | The merged PR documents drain-before-migration and forbids mixed old-writer/new-reader operation. This is a procedure, not a deployment record. | In a disposable or staging environment only, produce a runbook rehearsal: stop/drain test Gateway and usage writers, snapshot/backup, apply bootstrap+migrations, run the live runner, start new readers/writers, and verify rollback/forward-fix behavior. Never downgrade a binary that cannot read attempt facts. Any production execution requires separate explicit authorization and a change record. | Staging topology, migration owner, disposable backup/restore evidence, rollback authority, and test traffic-drain/health-check thresholds. Production topology or maintenance-window details are required only for a separately authorized production change. |
| Database support matrix | Required hosted evidence is PostgreSQL. The contract text mentions multiple adapters, but no current evidence in this checkpoint proves identical Gateway attempt lifecycle semantics on MySQL/SQLite. | Either add adapter-specific lifecycle tests and required CI, or explicitly document PostgreSQL-only support for this feature and keep non-PostgreSQL paths fail-closed. Do not infer parity from compile success. | Supported-driver decision, CI services/versions, migration rollback constraints, and owner for each adapter. |

## #206 residuals outside #300

These items must not be silently folded into the image funding change:

1. `insufficient_quota` recovery policy is the same finance decision described
   above and overlaps #253. The data API alone is not a user-visible recovery
   workflow.
2. Usage stream approximate trimming (`stream_maxlen`, currently configured in
   `crates/aether-usage/runtime/src/config.rs`) and its loss metric/alert belong
   to the runtime/operations track (#223/#217). They require a capacity budget,
   retention policy, and alert threshold; changing the max length as part of
   #300 would mix unrelated operational behavior.
3. Cancellation charging outside the explicit image attempt contract remains a
   product/contract question. Existing client-disconnect policy and the funded
   image evidence must not be reinterpreted as a token-level billing decision.
4. Historical rate multipliers and old settlement records need an audit and
   remediation policy. They must not be rewritten opportunistically while
   validating new image reservations.

## Evidence gates

Before updating either issue or its Project card, collect all of the following:

1. A current `main` commit and PR status, plus the exact hosted runner logs for
   every selected ignored target. A green aggregate job without exact target
   counts is insufficient.
2. A staging crash/restart artifact containing request ID, attempt IDs, frozen
   quote hash, reservation states before/after restart, upstream call count,
   final ledger/hold values, and replay result. Redact credentials and bodies.
3. A staging recharge-recovery artifact containing payment commit ID, recovery
   idempotency key, lock/retry timeline, collected amount, remaining debt, and
   alert/notification result.
4. A provider sandbox receipt artifact and a reconciliation report for normal,
   partial, over-ceiling, malformed, and duplicate receipts.
5. A staging/disposable deployment runbook rehearsal with backup/restore proof
   and migration/rollback outcome. Production data must not be used for fixture
   creation or exploratory repair; any production run requires separate
   authorization and a change record.

Until these gates exist, keep #300 and #206 open and keep paid unsupported
stream/multi-stage requests fail-closed. No code or configuration change should
claim to implement the missing external evidence.

## Safe local verification (no policy change)

The existing no-upstream security boundary is already covered by the exact live
fixture `public_tests::live_public_images_reject_unbounded_and_stream_before_every_account_shortcut`.
The remaining work is environment- and policy-dependent, so this checkpoint
does not add a second test that merely mirrors that implementation. A future
change should first add the external decision and then add the smallest
behavioral test for that decision before touching production code.
