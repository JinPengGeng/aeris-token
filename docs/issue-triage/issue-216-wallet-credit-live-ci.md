# Issue #216: require the existing wallet credit regressions

## Assessment and decision

On fork main `d7b36b9c90dedcb92c8855cdbf4c94e4fc11eb11`, eight existing
PostgreSQL wallet tests were marked ignored and were not selected by the
required live database job. They exercise payment callback idempotency and
ownership checks, manual recharge, administrator order transitions and credit,
and redemption into recharge/gift buckets. Losing these checks would leave
real money mutation paths without their existing database regression coverage.

Accept this as a bounded P1 CI work package under #216: high regression value,
small implementation, no new product or financial policy. Add each existing
test to `tools/ci/run_postgres_live_tests.sh` as a separately logged exact
target, after the schema migration prerequisite. Preserve the explicit
disposable database requirement and serial execution. Do not enable unrelated
ignored tests by a broad name filter.

## Selected contracts

| Existing test suffix | Behavior asserted |
| --- | --- |
| `live_payment_callback_user_wallet_credits_once` | Repeated callbacks for a user wallet credit once and retain one ledger entry. |
| `live_payment_callback_api_key_wallet_credits_once` | The same guarantee for a standalone API key wallet. |
| `live_payment_callback_rejects_wrong_or_missing_wallet_owner` | Invalid ownership leaves the order pending and balances unchanged; the failed callback is persisted. |
| `live_manual_recharge_commits_wallet_order_and_transaction` | Wallet, order and ledger commit together; duplicate orders and invalid amounts are rejected. |
| `live_admin_order_state_changes_preserve_metadata` | Administrator state transitions preserve order metadata. |
| `live_admin_wallet_order_credit_commits_once` | Wallet credit is idempotent and rejects incompatible order states. |
| `live_admin_plan_order_credit_commits_once` | Plan entitlement and gift credit commit once. |
| `live_redeem_code_commits_order_and_wallet_once_for_each_bucket` | Recharge and gift redemption preserve bucket/refund semantics and do not duplicate orders or transactions. |

## Validation and limits

The tests call the real PostgreSQL repository. Their single connection uses
`search_path = pg_temp` and temporary tables copied from the migrated public
schema with `LIKE ... INCLUDING ALL`. PostgreSQL does not copy foreign keys
with that clause: these checks do not prove the full production relationship
graph, concurrent callbacks on multiple connections, external payment-provider
delivery, or HTTP behavior.

Local validation on 2026-09-13 used Rust 1.95.0 and a disposable PostgreSQL
17.11 cluster bound to `127.0.0.1:56639`. The complete expanded harness ran
20 separately named targets: every target reported one passed, zero failed
and zero ignored. This includes all eight wallet tests above and the 12
existing prerequisites/regressions. The temporary server was stopped normally
after execution; its data files remain available for review. No application
database was used. Independent review and hosted checks are still pending.

The initial local attempt stopped at `pg_isready` because the restarted
temporary cluster used the default port. The owned cluster was stopped and
restarted with its explicit test port/socket before the successful full run.

The required hosted `Data DB Live (selected ignored tests)` job must actually
run every selected target; compiling tests or a successful run with zero
matching tests is insufficient acceptance. Issue #216 remains open for its
other ignored tests, integration execution and build-trigger work.
