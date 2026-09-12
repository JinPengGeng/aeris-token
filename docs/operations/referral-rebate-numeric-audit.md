# Referral rebate historical NUMERIC audit

Issue #316 records the post-fix historical-data review for referral rewards.
The runtime defect was fixed by adding `CAST(... AS DOUBLE PRECISION)` to all
PostgreSQL referral reward reads. This work intentionally remains read-only:
no production rows are modified and no automatic backfill is proposed.

## Procedure

Run `referral-rebate-numeric-audit.sql` with a PostgreSQL role that has only
`SELECT` on `referral_rewards` and `wallet_transactions`. Export only the
aggregate result sets. The queries report row counts, time bounds, status/type
breakdowns, amount totals, missing ledger links, ledger mismatches, and invalid
counter totals. They never return user/order identifiers or notes.

The linked-ledger comparison uses `NUMERIC` arithmetic and a one-cent-in-100M
tolerance (`0.00000001`) matching the schema scale. A non-zero mismatch or
missing applied link requires a separately reviewed, idempotent repair plan;
this audit does not infer that a row should be re-issued.

`fixtures/referral-numeric-regression.sql` is the adapter regression fixture.
It exercises the smallest and largest representative `numeric(20,8)` values
through the production cast shape, including an 8-decimal value that would
have failed under a direct sqlx `f64` decode.

## Decision record

Until an operator runs the aggregate queries against a production snapshot,
the historical impact is **unknown** and no backfill is authorized. If all
four result sets are empty/consistent, record “no repair required” on Issue
#316. Otherwise create a follow-up migration issue with an idempotency key,
bounded batches, audit logging, and a rollback query before changing data.
