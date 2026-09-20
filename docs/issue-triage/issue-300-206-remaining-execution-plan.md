# #300 / #206 remaining execution contract

Status: current local acceptance record and remaining-execution boundary. This
document describes an **uncommitted** working tree based on
`26638a4f72295b7f5cf4851a331efce7b2b5314b`. PR #447 still points at that
base, so its remote/hosted checks do not cover the local changes or the
2026-09-18 acceptance results recorded here. Do not close #300 or #206 from
this document.

The older `main@4620ad4e6` / PR #391 statements below are historical context
only. They are not the current local acceptance boundary. This document does
not establish production financial policy, alter wallet data, publish a
deployment, or claim real supplier invoicing.

## Current local checkpoint

`docs/issue-triage/followup-acceptance-20260918.md` records the current local
working-tree acceptance. The selected live inventories used disposable local
PostgreSQL and synthetic sources:

| Scope | Local result | Primary retained evidence |
| --- | --- | --- |
| PostgreSQL live inventory | 63 exact targets passed; zero failed and zero ignored. | `/Users/jinpeng/.agents/tmp/aeris-followup-20260918/live-gates/postgres-live.log` and `summary.json` |
| Gateway funds, recharge, and refund inventory | 24 exact targets passed; zero failed and zero ignored. | `/Users/jinpeng/.agents/tmp/aeris-followup-20260918/live-gates/gateway-live.log` and `summary.json` |
| Automatic recharge recovery | A committed local recharge collects eligible historical debt once, exposes wallet history, and survives SMTP retry without repeating money. | Gateway target `maintenance::runtime::recharge_recovery::live_tests::live_gateway_recharge_callback_collects_once_exposes_history_and_acks_smtp_retry` |
| Synthetic supplier receipts | Normal, partial, over-count/over-price, specification drift, catalog-price drift, malformed/truncated, and late/unknown receipt paths passed locally with frozen quote evidence. | Gateway receipt targets and `AETHER_SYNTHETIC_RECEIPT_EVIDENCE` records in `gateway-live.log` |
| Post-dispatch process death | The local parent kills the dispatched Gateway with SIGKILL, retains the Unknown hold across a new PID, then settles one late receipt exactly once after cleanup/replay. | `AETHER_IMAGE_CRASH_EVIDENCE` records in `gateway-live.log` |

`live-gates/summary.json` reports `postgres: 63/0/0` and `gateway: 24/0/0`
(passed/failed/ignored), with `synthetic_only: true`. This is local acceptance
evidence, not a remote CI, staging, or production result. The retained logs
contain the exact target names and state records; no credential, request-body,
or real supplier receipt is asserted by this plan.

## Accepted local implementation boundary

The automatic recharge collection contract in
`issue-206-recharge-recovery.md` is implemented and locally verified for the
selected scope. It is no longer pending implementation or a pending
finance-choice question. The accepted local behavior is bounded to committed
recharge principal, eligible legacy `insufficient_quota` debt with frozen
evidence, durable idempotency, wallet history, retryable notification delivery,
and preservation of unpaid residual debt. It does not authorize a change to
signup/referral credit, historical debt remediation, current catalog repricing,
gift balances, or attempt-funded Unknown/reconciliation obligations.

The synthetic supplier-receipt matrix is also implemented and locally verified.
It uses synthetic local upstream data and frozen quotes; it must not be
described as actual supplier billing, a provider sandbox exercise, or proof of
production cost provenance. Reconciliation-pending outcomes remain pending
when the synthetic receipt exceeds the frozen quote or has unknown/invalid
facts; malformed and truncated receipt cases retain the required financial
state before their authoritative settlement path.

The local process-death drill now supplies a completed disposable exercise,
rather than only an implementation description. Its evidence shows one
upstream call, an 8,000,000-unit Unknown hold after SIGKILL, no debit while
the result is unknown, one 7,000,000-unit late-receipt settlement, and no extra
financial or counter effects after outbox cleanup and replay. This validates a
bounded local manual recovery procedure; it is not an automatic orphan-recovery
claim.

## Work still required before issue closure

| Track | Local state | Remaining evidence or decision |
| --- | --- | --- |
| Paid stream / multi-stage / conversion | Still fail-closed before upstream dispatch for unsupported paid projections. | Product/API decision and, before enabling a billable route, an explicit quote, partial-output, cancellation/timeout, provider-projection, and public error contract. |
| Automatic orphan recovery | No demonstrated production durable lease/worker closes abandoned admission, correlates an authoritative external receipt, and applies an approved Unknown-hold escalation SLO. | Define and implement the operational recovery mechanism if required, then verify it in staging/production under separately approved change control. The local manual drill does not satisfy this gate. |
| Actual supplier billing | Synthetic local receipts passed. | Separately authorized real provider/supplier receipt evidence before claiming actual invoicing, provenance, or external settlement behavior. |
| Recharge recovery rollout | Local automatic collection passed. | Deployment, migration/restore, operational monitoring, notification-channel, and production data evidence. Do not replay historical credits or change financial policy merely to deploy it. |
| Retention and replay in an operated environment | Local cleanup/replay coverage exists for the exercised receipt and crash paths. | Staging/disposable rehearsal across prepared, dispatched-unknown, reconciliation-pending, and legacy-debt rows; confirm backup/restore and retention schedules preserve unresolved obligations. |
| Database support matrix | The completed local live scope is PostgreSQL. | Establish adapter-specific lifecycle support and CI, or document PostgreSQL-only behavior and keep unsupported paths fail-closed. |
| Enrichment, signup credit, and historical records | These are distinct #206/#253 and financial-governance concerns. | Owner-approved contracts and targeted evidence; do not infer them from the local recharge or synthetic-receipt acceptance. |

## Evidence required for an issue or deployment update

Before updating #300, #206, a Project card, or a deployment record, retain the
current commit/PR status and exact runner counts, then add only the evidence
relevant to the claim being made:

1. For a production or staging recovery claim: an authorized crash/restart or
   orphan-recovery artifact with request/attempt IDs, quote hash, hold and
   ledger states, external-receipt correlation, replay result, and redaction of
   credentials and bodies.
2. For a recharge rollout claim: migration/activation proof, payment and job
   identities, idempotency/retry history, collected and residual amounts,
   notification outcome, and evidence that historical already-credited
   payments were not newly authorized.
3. For an actual supplier-billing claim: authentic provider receipt and
   reconciliation evidence under separately authorized credentials and spend.
4. For a deployment claim: backup/restore and drain/migration rehearsal,
   health checks, rollback/forward-fix outcome, and the required change record.

Keep #300 and #206 open until their remaining product, production, and
operational boundaries have evidence. Local acceptance does not authorize
production financial changes or silently close a parent issue.

## Historical context: main / PR #391

PR #391 was squash-merged as `acf02288d5c37220313a9aed8ec6c277009252a1`.
At the older `main@4620ad4e6` checkpoint, its hosted Data DB Live job exercised
34 exact targets and its Gateway live job exercised 17 PostgreSQL/HTTP targets,
reported as `1 passed / 0 failed / 0 ignored` per selected target. That record
predates the uncommitted `26638a4f`-based work and is retained solely as
historical provenance; it must not be used to omit the newer local recharge,
receipt, or process-death acceptance results.
