# Issue 300: independent hard-cost policy for funded attempts

## Decision and scope

An independently billable attempt reserves both its funding sources and its hard
plan-cost allowance before dispatch. The public request ID stays unchanged.
`RequestFundsUsagePolicy` is supplied by the trusted admission context, with the
original request admission timestamp and windows. It is stored separately from
quote pricing and request/report metadata. The first successful attempt freezes
that policy, including its absence; subsequent attempts and replays must match.
The context token is distinct from child tokens. Each child uses its funds token
as the corresponding `usage_cost_reservations` token.

The existing quota table already permits many rows per public request. Keeping
one immutable terminal per attempt reuses that model without a mutable parent
terminal or retaining the maximum candidate estimate. Quota occupancy is the sum
of known actual costs plus unresolved authorization ceilings. Actual and funds
units both use 100,000,000 units per USD; no floating conversion is introduced.
Quota actual includes frozen multipliers and does not use the amount collected
from a wallet or grant. A known amount above authorization remains recorded even
if that exceeds the policy limit; subsequent admission is rejected.

## Atomicity, ownership and locks

PostgreSQL admission holds the parent request lock, then the policy subject row
lock, then wallet/entitlement and reservation locks. Outcomes follow the same
order. The ordinary quota API holds only its subject and quota rows; it never
calls a parent/funding mutation in that transaction. Both legacy and new quota
admissions therefore serialize on the existing subject lock. Funding rejection
writes no quota row; quota refusal occurs before funding mutation. Persistence
errors after either insertion roll back the whole transaction.

The memory adapter holds its existing settlement lock and parent guard, then
the cost-reservation write guard throughout joint admission/outcome. Legacy
quota methods contend on the same cost guard. All refusal checks precede
mutation. An explicit optional policy on stored attempt metadata owns its quota
entry in memory; PostgreSQL stores the policy on the funds row and a constrained
foreign-key linkage from quota to funds.

Charged outcomes finalize their own quota to actual cost regardless of client
success, failure or cancellation. A supported no-charge outcome releases only
its own quota. Unknown outcomes remain reserved. Closing request admission does
not release dispatched Unknown costs. Late-known outcomes replace only their
own Unknown, and terminal replay does not add quota or debit twice. Legacy
reserve/reconcile methods refuse a linked child token.

## Retention and compatibility

Linked reservations are excluded from the legacy expiry filter and cleanup.
Their original admission timestamp still determines window membership; an old
operation naturally stops affecting a window that no longer includes it, but
the financial identity is not erased. Linked terminal rows are retained with
their funds replay identity as well. Removing that evidence needs a separate
financial retention protocol; this change does not introduce one.

Previously stored attempts have no policy and retain that meaning. Optional
fields deserialize with defaults. Existing text request quotas keep the same
expiry, reconciliation and cleanup behavior. Standalone keys do not inherit a
user plan policy; supplying one for a standalone identity is invalid. Wallet
entitlements and explicit postpaid wallets remain separate funding choices.

Schema changes are represented in logical definitions, the incremental
PostgreSQL migration, bootstrap source and generated audit output.

## Validation status

Implementation and tests authored in isolated worktree
`/private/tmp/aeris-attempt-quota.JUL5vp` from `ca40de25bd4ef2b437b8afc59297b5a739e0837f`.
The validated data slice is committed locally for subsequent Gateway integration;
this task has not pushed a branch or opened a separate PR.

At this checkpoint Rust formatting, ShellCheck, `git diff --check`, and logical
schema generation/check have passed. Schema generation reused the existing
`/private/tmp/aeris-attempt-funds.U3xxiO/target/debug/aether-schema` executable;
it consumed this worktree's logical input and wrote only this worktree's
generated output, without invoking Cargo.

The complete bootstrap manifest has also executed successfully in one
PostgreSQL transaction against a fresh, separate socket-only PostgreSQL 17.11
instance. The new policy constraint, token equality constraint and funds FK
were read back and verified. Command: `bash /private/tmp/aeris-quota-check-bootstrap.sh`.
The validation database is `aeris_quota_bootstrap_check`; the `postgres` database
on the same task-owned instance remains fresh for migration/live tests. Socket:
`/private/tmp/aeris-attempt-quota-db.1lidH2`, port `57529`. The main Gateway
database was not touched. The first server startup failed on a misquoted empty
listen argument; reading its log identified that setup issue, and an explicit
empty listen address started the dedicated instance successfully.

After the Gateway build released its resources, the main agent executed the
actual Rust and database validations in this isolated worktree using Rust 1.95
and the shared warm target `/tmp/aeris-attempt-funds-target.LLh9Z1`:

- Two new contract tests passed. All 29 memory settlement tests passed,
  including five new quota regressions and the existing legacy quota behavior.
- All four new PostgreSQL quota targets passed with zero ignored tests.
  The full `tools/ci/run_postgres_live_tests.sh` then passed all 29 exact
  targets, each with one passed and zero ignored. This executed incremental
  migrations in the dedicated PostgreSQL instance. Log:
  `/private/tmp/aeris-attempt-quota-live-20260913.log`.
- `cargo clippy -p aether-data-contracts -p aether-data -p aether-data-postgres
  --all-features --all-targets -- -D warnings` passed in 32.33 seconds.

The main review checked the trusted owner/context contract, frozen omission,
subject-lock ordering, atomic funding/quota admission and outcome, plus legacy
expiry/reconciliation guards. No source correction was needed for these local
checks. Gateway hard-policy integration and Hosted validation remain separate
requirements; the data results do not prove them complete.

The regression set covers .10/.08 rejection while wallet has .20; .20 admission
and .06 plus late .07 = .13; distinct-request concurrency; wallet and quota
refusal; insertion/outcome rollback; entitlement sources; prepared cancellation;
charged failed/cancelled work; quote overrun; policy omission/change; legacy
reconcile/expiry/cleanup; and replay after counter delivery cleanup.

This does not complete the separate frontend/user-key Redis daily counter,
Gateway public hard-policy wiring, stream/multistage images, recovery drills, or
the whole issue 300.
