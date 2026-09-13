# Issue #253 revenue and abuse risk audit

Date: 2026-09-13  
Baseline: `origin/main` at `c860eec66`

This document records the code evidence and the decisions required before any
charging or registration-policy change. It deliberately does not change
existing billing semantics or attempt to recover historical usage.

## Findings

| Area | Current evidence | Confidence | Consequence |
| --- | --- | --- | --- |
| Signup credit | `crates/aether-admin/src/system.rs:2229` returns `default_user_initial_gift_usd = 10.0`; the registration lifecycle reads this value and creates the wallet gift (`apps/aether-gateway/src/state/runtime/auth/user_lifecycle.rs:1212+`). | Code confirmed; production enablement unconfirmed | A public deployment can expose a $10 acquisition subsidy. The repository default does not establish that every deployment does so. |
| Registration controls | `turnstile_enabled` defaults to `false` (`system.rs:2256`); password and quota settings are configurable and are exercised by tests. | Code confirmed; production policy unconfirmed | Verify the effective production configuration, email verification, account/IP limits, and abuse monitoring before calling this a zero-control incident. |
| Admission and reservations | `apps/aether-gateway/src/plan_usage_policy.rs` and `executor/candidate_loop.rs` issue server-side request/cost reservations. PostgreSQL and memory adapters make request admission and cost reservation idempotent and expire/release reservations. | Covered for the paths that call these APIs; full HTTP/stream/WS inventory remains an acceptance task | Reservation protects the planned request budget, but it is not proof that every legacy usage row carries an authorization token. |
| Settlement concurrency | PostgreSQL locks the wallet row (`crates/aether-data/adapters/postgres/src/settlement.rs:1240+`) and atomically debits daily quota. Existing tests cover concurrent idempotent settlement and quota debit. | Code and tests confirmed | Re-running settlement is bounded, but policy still determines whether finite wallets may overdraw. |
| Overdraft | A finite wallet can use `recharge_overdraft`; the memory adapter test is `finite_wallet_insufficient_balance_overdraws_and_settles`. | Code and test confirmed | This is an explicit business capability, not an accidental race. Disabling it requires product approval and migration rules. |
| `insufficient_quota` | When quota is insufficient, settlement writes terminal `billing_status = insufficient_quota`, debits `0.0`, and returns. Provider monthly usage is incremented only for `settled` (`settlement.rs:1131+`, `1294+`, `1388+`). Missing-wallet nonzero-cost rows take the same terminal path. | Code confirmed; delivered-cost volume unmeasured | A provider request can be delivered without an internal receivable or provider-cost usage increment. The API contract intentionally treats this as a non-retryable account state, so changing it is compatibility-sensitive. |
| Referrals and provider margin | The issue's claims about self-referral loops, provider cost, and margin are separate accounting projects. Existing referral retry/compensation and numeric handling are documented elsewhere. | Not in scope for this slice | Do not mix referral caps or provider price-book changes into a registration or settlement hotfix. |

## Decisions required from product/finance

1. Set the production signup-credit amount and eligibility (including one credit
   per verified identity, email/phone requirements, and referral interaction).
2. Set Turnstile, email verification, per-IP/account rate limits, and abuse
   response defaults for public registration.
3. Decide whether finite wallets may overdraft, the maximum authorized amount,
   and whether a later recharge can settle an existing receivable.
4. Decide the accounting treatment for delivered usage that reaches
   `insufficient_quota`: reject before provider dispatch, record a receivable,
   or retain the current terminal state with alerting and a manual workflow.
5. Define the provider cost source and the owner/SLO for quota and unpaid-cost
   alerts.

## Safe implementation order after decisions

1. Add a production configuration/verification check and a public-registration
   threat-model acceptance fixture. This can be deployed without changing
   existing balances.
2. Add an admit/settle concurrency simulation covering duplicate requests,
   retries, finite-wallet boundaries, reservation expiry, and both memory and
   PostgreSQL adapters.
3. Add an `insufficient_quota` metric/report and alert test that distinguishes
   denied-before-dispatch from delivered-without-settlement. Preserve the
   current API contract during rollout.
4. Implement any overdraft or receivable change behind an explicit, versioned
   policy flag with a migration, rollback, and reconciliation procedure. Do not
   retroactively mutate settled or `insufficient_quota` rows without a signed
   finance decision.
5. Track referral caps and provider price/margin accounting as separate issues.

## Acceptance and rollback

- Acceptance requires production-effective configuration evidence, the
  concurrent admit/settle simulation, quota alert/report evidence, and a
  before/after ledger reconciliation on a staging dataset.
- Rollback must disable the new policy flag or alert consumer without rewriting
  historical usage. Any receivable backfill must be an explicit, reviewed
  finance operation with an immutable audit record.
- Until these criteria are met, Issue #253 remains a P1 policy/planning item;
  no automatic merge or charging-semantic change is justified by repository
  defaults alone.

