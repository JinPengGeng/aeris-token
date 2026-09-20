# Native financial-ledger restore acceptance

The application JSONL export does not contain the complete request-fund,
recharge-recovery, refund-notification and administrator audit-delivery ledgers.
A full PostgreSQL backup is
required to preserve their existing authorization, operation, receipt and
notification state.

The exact local acceptance target is:

```text
settlement::recharge_recovery::native_restore_tests::live_native_postgres_restore_preserves_nonempty_funds_and_replay_boundaries
```

Run it with `cargo test --locked -p aether-data-postgres --all-features --lib`
and the exact target followed by
`-- --exact --include-ignored --test-threads=1 --nocapture`.
`tools/ci/run_postgres_live_tests.sh` includes it in the required inventory and
rejects zero-test or ignored-test success.

Set `AETHER_TEST_DATABASE_URL` to a disposable PostgreSQL administrative
connection that may create and remove databases. The test writes only two
randomly named source/restored databases; it does not use the administrative
database as the fixture. Native `pg_dump`/`pg_restore` on PATH are the default.
The explicit `AETHER_TEST_PG_CONTAINER` option runs those tools inside the
chosen PostgreSQL container, matching their major version to the server. CI
uses its own service container ID. Passwords are passed through process
environment, not command arguments.

Set `AETHER_NATIVE_RESTORE_ARTIFACT_DIR` for retained evidence. Its fallback is
`AGENT_TMP_DIR`, then `RUNNER_TEMP`, then the system temporary directory. Each
run owns a private subdirectory. On this workstation use the existing task
directory under `/Users/jinpeng/.agents/tmp`; reuse the Cargo target with two
build jobs and reclaim it after the active validation batch.

## Verified financial boundary

The synthetic source includes an already partially collected recharge, a second
pending recharge budget, a failed notification awaiting retry, and an Unknown
attempt with an active hold and frozen quote. Another debt appears after the
second credit and must remain outside that credit's candidates.

The test takes a full custom-format archive and restores into an empty database
with `pg_restore --exit-on-error --single-transaction --no-owner --no-acl`.
It compares every public table row before any resumed work, plus constraints,
indexes, sequence values and `is_called`, views, and the enabled deferred
recharge trigger/function. PostgreSQL may redistribute a constant varchar-array
to text-array cast when reparsing DDL. Only the two equivalent spellings of
strict ASCII enum literals are normalized for constraint/index comparison;
values, order, multiplicity and surrounding SQL remain significant. Original
schema evidence is retained, and a separate test rejects changed predicates.

After restore, the real repository APIs replay callbacks, resume the pending
budget and retry notification ACKs. The old exhausted budget stays exhausted,
new debts cannot enter old candidates, gifts and active holds stay protected,
and a synthetic late receipt settles once using its frozen quote. A genuinely
new recharge alone can authorize the later debt. Job totals must reconcile with
operation, receipt and wallet-transaction totals. The source remains unchanged.

The latest 2026-09-18 local exercise compared 104 public tables and replayed
three callbacks. It collected 21,000,000 debt units, settled 7,000,000 attempt
units, and left 16,000,000 wallet units; one USD is 100,000,000 units. The
fixture includes nonempty provider-cost prices, current Unknown, Estimated and
Known snapshots, and immutable snapshot-import receipts. It passed one executed
test with no failures or ignored tests. Both dedicated databases were removed,
and the stopped owned cluster was removed after reclaiming 64,861,663 bytes.
The extension also restored four real terminal-refund notification events:
delivered, retry, unclaimed pending and pending with an active lease. A separate
wallet isolates their arithmetic. Due/lease gates, expired-token rejection, ACK
replay and four refund terminal replays changed no financial records and created
no new events. This extended exact test passed; its new integrated Gateway
checks remain separate.

The audit extension restored four synthetic delivery scenarios: pending with
an existing immediate audit row, pending with a scheduled retry, delivered,
and leased. The same-ID replay acknowledged the existing row without replacing
it; expiry and reclaim rejected the old token. Audit continuation left the full
system-config row, including its timestamp, unchanged. Pending-with-retry is a
scenario label; its persisted state is `pending` with a nonzero attempt count.

Evidence includes `synthetic-ledger.dump`, `archive.sha256`, the archive TOC,
source/restored row and schema JSON, tool versions and original stderr,
`reconciliation.json`, and `verified.json`. The last file is written only after
the financial assertions and database cleanup succeed. The latest local
evidence is `/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/pg-iegtejl1/summary.json`,
`cleanup.json`, and
`native-ledger-restore-7d7a0cc2014740ffad0c67751d5b93e2/verified.json`.
CI retains the corresponding synthetic artifact directory even on failure.

This proves a complete native archive round trip for the specified nonempty
synthetic fixture. It does not validate production backup objects, production
ownership/ACL restoration, PITR, or production RPO/RTO. New financial or
notification tables require nonempty restore/replay coverage before extending
this acceptance claim to them.
