# Issue #253: registration credit and remaining revenue risks

Date: 2026-09-13. Source baseline: fork main `da2155693`. This decision is
accepted for implementation under the maintainer's existing authorization to
choose and document engineering policy. The initial audit below is historical.

## Verified problem and decision

When `default_user_initial_gift_usd` was absent, local registration, first LDAP
login, OAuth account creation and administrator-created accounts each fell back
to USD 10. The settings response and UI also advertised that default; the admin
creation form prefilled 10 and rejected zero. A deployment that enables public
registration therefore offered a spendable subsidy for each newly created
identity unless the operator configured it. Source defaults alone do not prove
production abuse or historical losses.

The accepted default is **zero initial gift**. Operators can explicitly set
`default_user_initial_gift_usd` to a nonzero promotional amount. This removes the
implicit subsidy without requiring an external CAPTCHA or mail provider just
to deploy the service. Existing IP and identity limits remain in force:
registration allows 10 attempts per IP and 5 per identity per hour; login allows
60 per IP and 10 per identity per minute. These controls are implemented in
`auth_rate_limit.rs`, so the original assertion of no login/registration limits
is stale. They do not establish one natural person per account.

## Implementation and compatibility

- `aether_admin::system::DEFAULT_USER_INITIAL_GIFT_USD` supplies one Rust default
  for settings, local registration, LDAP, OAuth and admin fallback.
- The settings client uses zero before a configured value is loaded and
  preserves a server-provided value. The new-user form starts at zero, allows
  zero, and restores zero when leaving unlimited mode. Explicitly entered
  grants continue through the existing validated admin endpoint.
- An existing persisted setting, including USD 10, keeps its configured value.
  It applies to self-service signup, LDAP/OAuth account creation and admin API
  requests that omit the gift. The admin form submits its displayed amount
  explicitly; its zero default does not inherit a configured promotion.
  With no persisted setting, accounts created after the update get zero rather
  than the old implicit USD 10. Operators who intend to retain the promotion
  should save the desired amount before upgrading.
- No migration rewrites wallets, historical ledger rows or existing settings.
  Wallet initialization/idempotency, explicit unlimited accounts, refund rules
  and settlement semantics are retained. Configuration lookup failures still
  fail account creation; they do not silently issue a grant.

Rollout: review the saved gift setting, deploy the reviewed version, create a
disposable account and inspect its gift balance and adjustment ledger. Explicit
promotions require the operator's own eligibility and abuse policy. Rollback
can restore the desired gift via the existing setting without touching history;
a code revert restores the old implicit default for later account creation.

## Validation of the accepted slice

The existing real HTTP registration regression now runs twice: absent gift
configuration must produce zero gift/adjustment after registration and login;
an explicit USD 12.50 setting must still produce USD 12.50. Neither request
reaches the upstream provider. Existing LDAP and administrator regressions
retain their explicit nonzero grants. These HTTP regressions use the test's
in-memory repositories; they do not constitute a live PostgreSQL acceptance
run or a separate zero-default HTTP matrix for LDAP/OAuth and admin fallback.

Frontend tests load missing and explicit settings and exercise the rendered
new-user form: zero enables Create, unlimited-to-finite restores zero, and zero
and USD 6.50 reach the actual emitted creation payload. Exact local/hosted
results are recorded on PR #379; CI must pass on its final head.

## Parent residuals and order

| Work | Decision | Priority / benefit / complexity | Acceptance |
| --- | --- | --- | --- |
| Actual request funding | #362 adds funding holds; plan-budget reservations are a separate contract. #300 owns Gateway integration. | P1 / prevent concurrent unfunded delivery / L | Per-attempt admission, dispatch, frozen quote, actual-cost facts, cancellation/crash/retry recovery and concurrent Gateway tests. |
| Delivered insufficient_quota usage | Preserve the public error contract while adding unpaid-cost and reconciliation evidence; data APIs alone do not connect the recharge worker. | P1 / retain unpaid-cost visibility / L | Distinguish pre-dispatch denial from delivered usage; durable cost, idempotent recharge recovery, producer metrics and alert/report tests. |
| Registration abuse controls | Keep existing rate limits and optional email/Turnstile; zero credit does not close every abuse vector. | P1 / control configured promotion exposure / M | Verify deployment policy, proof consumption, distributed limits and eligibility before enabling a nonzero public subsidy. |
| Referrals | Eligibility and caps need explicit configuration alongside existing compensation. | P2 / bound promotion liability / M-L | First-charge/verification eligibility, self-invitation threat model, configured caps and idempotent reversals. |
| Provider cost and margin | Selling-price counters are not a provider-cost price book; historical cost evidence is absent. | P2 / measure margin / L | Versioned cost inputs, separate selling/actual costs, reconciliation provenance and reporting without inferred historical charges. |

Issue #253 remains open. This PR completes the default-grant safeguard only;
production evidence and historical financial remediation remain separate.
The initial proposals below are retained as audit history; the accepted
decisions and remaining acceptance criteria above define the current plan.

## Initial audit at c860eec66 (historical)

### Findings

| Area | Current evidence | Confidence | Consequence |
| --- | --- | --- | --- |
| Signup credit | `crates/aether-admin/src/system.rs:2229` returns `default_user_initial_gift_usd = 10.0`; the registration lifecycle reads this value and creates the wallet gift (`apps/aether-gateway/src/state/runtime/auth/user_lifecycle.rs:1212+`). | Code confirmed; production enablement unconfirmed | A public deployment can expose a $10 acquisition subsidy. The repository default does not establish that every deployment does so. |
| Registration controls | `turnstile_enabled` defaults to `false` (`system.rs:2256`); password and quota settings are configurable and are exercised by tests. | Code confirmed; production policy unconfirmed | Verify the effective production configuration, email verification, account/IP limits, and abuse monitoring before calling this a zero-control incident. |
| Admission and reservations | `apps/aether-gateway/src/plan_usage_policy.rs` and `executor/candidate_loop.rs` issue server-side request/cost reservations. PostgreSQL and memory adapters make request admission and cost reservation idempotent and expire/release reservations. | Covered for the paths that call these APIs; full HTTP/stream/WS inventory remains an acceptance task | Reservation protects the planned request budget, but it is not proof that every legacy usage row carries an authorization token. |
| Settlement concurrency | PostgreSQL locks the wallet row (`crates/aether-data/adapters/postgres/src/settlement.rs:1240+`) and atomically debits daily quota. Existing tests cover concurrent idempotent settlement and quota debit. | Code and tests confirmed | Re-running settlement is bounded, but policy still determines whether finite wallets may overdraw. |
| Overdraft | A finite wallet can use `recharge_overdraft`; the memory adapter test is `finite_wallet_insufficient_balance_overdraws_and_settles`. | Code and test confirmed | This is an explicit business capability, not an accidental race. Disabling it requires product approval and migration rules. |
| `insufficient_quota` | When quota is insufficient, settlement writes terminal `billing_status = insufficient_quota`, debits `0.0`, and returns. Provider monthly usage is incremented only for `settled` (`settlement.rs:1131+`, `1294+`, `1388+`). Missing-wallet nonzero-cost rows take the same terminal path. | Code confirmed; delivered-cost volume unmeasured | A provider request can be delivered without an internal receivable or provider-cost usage increment. The API contract intentionally treats this as a non-retryable account state, so changing it is compatibility-sensitive. |
| Referrals and provider margin | The issue's claims about self-referral loops, provider cost, and margin are separate accounting projects. Existing referral retry/compensation and numeric handling are documented elsewhere. | Not in scope for this slice | Do not mix referral caps or provider price-book changes into a registration or settlement hotfix. |

### Original proposed product/finance decisions (historical)

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

### Original proposed implementation order (historical)

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

### Original proposed acceptance and rollback (historical)

- Acceptance requires production-effective configuration evidence, the
  concurrent admit/settle simulation, quota alert/report evidence, and a
  before/after ledger reconciliation on a staging dataset.
- Rollback must disable the new policy flag or alert consumer without rewriting
  historical usage. Any receivable backfill must be an explicit, reviewed
  finance operation with an immutable audit record.
- Until these criteria are met, Issue #253 remains a P1 policy/planning item;
  no automatic merge or charging-semantic change is justified by repository
  defaults alone.
