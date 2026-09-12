# Referral rebate historical NUMERIC audit

Issue #316 records the post-fix historical-data review for referral rewards.
The runtime defect was fixed by adding `CAST(... AS DOUBLE PRECISION)` to all
PostgreSQL referral reward reads. This work intentionally remains read-only:
no production rows are modified and no automatic backfill is proposed.

## Procedure

Run the whole file in a fresh session with a PostgreSQL role that has only
`SELECT` on `referral_rewards` and `wallet_transactions`. Export only the
aggregate result sets. The queries report row counts, time bounds, status/type
breakdowns, amount totals, missing ledger links, ledger mismatches, and invalid
counter totals. They never return user/order identifiers or notes. The script
uses a repeatable-read, read-only transaction so all four reports share one
snapshot, with a 30-second statement timeout and a 2-second lock timeout.
Use a restored snapshot or a read replica for large datasets. A timeout is an
incomplete audit, never evidence of zero impact.

```sh
psql --no-psqlrc --set ON_ERROR_STOP=1 --dbname 'service=aether-audit' \
  --file docs/operations/referral-rebate-numeric-audit.sql
```

Configure the service and credentials locally; do not put passwords in this
command, terminal transcripts, Issues or PRs.

The linked-ledger comparison uses exact, signed `NUMERIC` arithmetic at the
schema scale of eight decimal places. A difference of `0.00000001` USD is
reported, as is a debit linked to a positive reward. Missing links include
dangling IDs, because this column has no foreign key. Ledger category, reason
and reward identity are also checked. A non-zero mismatch or
missing applied link requires a separately reviewed, idempotent repair plan;
this audit does not infer that a row should be re-issued.

`fixtures/referral-numeric-regression.sql` exercises representative
`numeric(20,8)` values through the production cast shape in a transaction that
rolls back. The Rust PostgreSQL integration test verifies that the raw NUMERIC
column fails direct `sqlx` `f64` decoding while the explicit cast succeeds;
the SQL output alone cannot establish driver decoding behavior. Converting to
`f64` is an API compatibility fix and does not preserve every decimal digit
of large amounts, which is why the audit keeps all comparisons in NUMERIC.

## Decision record

Until an operator runs the aggregate queries against a production snapshot,
the historical impact is **unknown** and no backfill is authorized. If all
four result sets are empty/consistent, record “no repair required” on Issue
#316, including snapshot time, fork SHA, query version and all four aggregate
results. The audit cannot prove that a reward row was never created for an
eligible order; cross-check pending/failed rewards and historical incident
windows before concluding that no repair is needed. Otherwise create a
follow-up migration issue with an idempotency key,
bounded batches, audit logging, and a rollback query before changing data.
The toolkit PR does not close #316 or parent #208; production impact remains
unverified until the above evidence and a reviewed decision are recorded.
