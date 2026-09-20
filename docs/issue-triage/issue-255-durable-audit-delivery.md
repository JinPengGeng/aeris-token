# Issue #255: atomic administrator audit delivery

The PostgreSQL system-config PUT path now commits the configuration and one
audit-delivery intent in the same transaction. If enqueue fails, the business
write rolls back. A successful business commit can be recovered independently
of the HTTP response finalizer. This first mutation family does not migrate
the complete administrator event inventory or cross-store writes.

## Delivery and failure behavior

The handler generates one server event UUID before mutation. The response keeps
that same pending audit record for the existing two-second immediate write.
The finalizer still emits structured audit logs and records its metrics; a
response marker prevents creation of a second random database event.
Families outside system-config PUT and the session/group/wallet batches below retain
their existing finalizer behavior.

The durable payload includes the authenticated actor, bounded target, parsed
client IP, fixed action/route metadata and server correlation ID. It excludes
configuration values, descriptions supplied in request bodies, cookies,
authorization and query secrets. The client's trace header cannot exceed the
database request-ID limit because it is not used as the durable correlation.

A supervised worker checks every two seconds and handles at most 32 events per
tick, claiming one immediately before delivery. Claims use SKIP LOCKED and a
30-second UUID lease; delivery has a ten-second timeout. Both success and
failure updates reject expired or superseded leases using database clock time.
Audit insertion and delivered ACK commit together. ON CONFLICT preserves an
already-written event, including an immediate writer's late commit.

Technical failures retain the event and schedule exponential backoff. At 12
failed deliveries the record enters dead_letter. Error codes are fixed enums;
raw database errors are not stored in the delivery record. Notification or
audit retries never call the system-config business mutation again.

## Verified local evidence

Four exact PostgreSQL targets passed, each with one executed, non-ignored test:

- Expired and superseded tokens, retry, and convergence to one audit row.
- An INSERT blocked by a real PostgreSQL advisory lock across lease expiry;
  the later ACK is rejected and the inserted audit row rolls back with it.
- Claim bounds.
- Two concurrent repositories claiming distinct events, successful independent
  delivery, and a malformed persisted payload reaching dead_letter at 12 real
  decode failures.

Fresh bootstrap plus migrations passed before each target. Integration found
and fixed missing SQLx UUID support and migration re-creation of a table already
present in the bootstrap schema. Cargo.lock only adds UUID dependency edges to
the affected SQLx packages; package versions are unchanged.

The native restore target passed with 101 public tables and four nonempty audit
delivery states, in addition to the existing recharge/refund ledgers. Pending,
retry, delivered and active-lease audit fixtures are synthetic persisted state.
Restore preserves rows, schema and sequences; replay rejects the stale token,
deduplicates an existing event and leaves the complete configuration row,
including timestamps, unchanged. Dedicated databases were removed.

The authenticated Gateway HTTP target passed, covering protected readback,
secret exclusion, long trace headers, enqueue rollback, INSERT failure,
client cancellation, retry and the immediate writer's two-second timeout/late
commit. Client cancellation alone is not evidence of process death.

The separate subprocess target passed with real SIGKILL before and after the
business commit, followed by a different PID running the production worker.
The pre-commit case rolls back business and intent; the post-commit case retains
one intent and recovers exactly one audit without replaying the configuration
write or changing its timestamps. For deterministic database barriers, the
parent explicitly terminates the dead child's identified PostgreSQL backend
before releasing its advisory lock. SIGKILL alone does not guarantee that the
server has cancelled a client's already-running SQL.

Ordinary audit tests passed 76 cases. Their three ignored entries are the two
executed live parents and the protected child helper, which is only invoked by
the crash parent. The complete 62 PostgreSQL and 24 Gateway inventories passed
again after integration, as did strict Gateway/changed-crate Clippy, the three
live-runner fixtures and all 232 automation tests.

Logs and summaries are under
`/Users/jinpeng/.agents/tmp/aeris-followup-20260917/`:
`audit-postgres.summary.json`, `native-ledger-restore-audit.summary.json`,
`audit-gateway.summary.json`, `audit-final.summary.json` and
`automation-audit.log`.

## Final local checkpoint and remaining scope

The extended subprocess target passed with one executed, non-ignored test in
33.35 seconds. It kills a worker after its durable claim commits, then starts
another PID before the original 30-second lease expires. Actual database time
passes the unmodified expiry before a new token permits delivery. The original
business row and timestamps remain unchanged, and exactly one audit is delivered.
The four child PIDs were 40430, 40431, 40432 and 40433; the third is the killed
claimant. Tokens appear in evidence only as distinct SHA-256 fingerprints.
The same explicit dead-backend cleanup boundary described above applies.
See `audit-worker-crash.summary.json` and `audit-worker-crash-0.log`.

The initial final-check run stopped at an import-wrapping formatting difference.
The correction passed workspace formatting and diff checks, recorded separately
in `audit-final-recheck.summary.json`; the original failure remains available.

CI explicitly runs the four delivery targets on separate fresh databases through
`tools/ci/run_admin_audit_live_tests.sh`, alongside the HTTP and crash parents.
Its fixture covers exact target selection, nonzero execution, command/log
failures and cleanup. The workflow retains failed-run logs. This machine used
Docker PostgreSQL for the live acceptance; the socket-only initdb/pg_ctl shell
runner has fixture coverage here, rather than a full local execution.

## Session revocation and group membership batch

Single-session revocation, all-session revocation and whole-group membership
replacement now use the same durable delivery contract. Each PostgreSQL mutation
commits its audit intent in the business transaction; other backends retain the
optional-capability fallback. A repeated HTTP single-session revoke records a
new administrator action but preserves the original revocation timestamp.

Revoke-all locks and rechecks the target user before changing sessions. A user
deleted after the handler precheck yields NotFound without a successful intent.
Password logins holding that user lock complete before the waiting revocation;
new logins ordered after revocation remain allowed.

Whole-group replacement locks affected users with NOWAIT, then the group row,
then rereads memberships. A newly discovered member forces complete rollback
and resnapshot before any business/intent write. At most 16 attempts occur with
backoff outside the transaction. Concurrent successful replacements return
complete requested sets. Default-group policy across transactions and cache
coherence between Gateway instances remain separate.

Eight new PostgreSQL targets and four Gateway exact targets passed, covering
atomic rollback, actual audit INSERT failures and recovery, stale delivery
tokens, authenticated JWT behavior, non-PostgreSQL fallback and actual database
lock barriers. Group/session regressions passed 59/177 cases. The complete
63 PostgreSQL/24 Gateway inventories, strict Gateway Clippy including tests,
data all-target/all-feature Clippy, formatting and all three runner fixtures
passed. Evidence is under `audit-families-integration/` in the same task log root;
the final gate summary is `final-20260917T105235Z/summary.json`.

The CI audit inventory now has 12 PostgreSQL targets, four live Gateway parents
and two ordinary memory targets. Each live target owns a separate database;
repository tests migrate first and HTTP parents start with empty databases.
The original generic crash/lease results above retain their earlier scope and
were not repeated for these mutation families.

## Wallet adjustment and manual recharge batch

Wallet adjustment and manual recharge now enqueue the audit intent in the
existing wallet/ledger/order transaction. Unsupported backends retain the
optional-capability fallback; a supported NotFound result does not repeat the
mutation through that fallback. Audit delivery reuses the committed event ID
and never calls the monetary mutation or recharge-recovery trigger again.

The real PostgreSQL target passed with recharge recovery enabled and synthetic
legacy debt. It verifies all-or-nothing rollback after an audit-ID conflict and
after a deferred recovery-candidate trigger fails at COMMIT. Fourteen classes
of persisted facts stay unchanged across failed mutations and audit retries.
A fresh order number with an existing audit ID specifically exercises the
audit constraint rather than the order-number constraint.

Authenticated HTTP and memory-fallback targets passed. The HTTP case also
renames a real quota-projection column to fail the response read after commit:
the existing API returns 502/control_unavailable, while the committed recharge,
recovery job and audit intent remain durable. Audit recovery changes no money.
Repeating the HTTP recharge still creates another monetary operation; this
patch does not add a client idempotency key. The first fixture expected 500 and
was corrected to the existing 502/Retry-After contract, with response assertions.

Focused regressions passed 139 Gateway wallet cases and 25 data wallet cases.
The five ignored Gateway cases are separate live cases, not counted as passes.
The complete 63 PostgreSQL/24 Gateway inventories and strict Gateway/data
Clippy passed. A final formatting-only correction has its own recheck evidence.

The native socket-only CI runner has now also executed successfully on local
PostgreSQL 15.19: 13 repository targets, five HTTP/crash parents and three memory
targets, plus 13 fresh migration targets, for 34 exact Cargo invocations.
Each result executed one test with zero ignored cases. Its 18 databases belonged
to an isolated cluster. The stopped cluster was removed, reclaiming about
407 MiB; logs remain. Four Docker databases from earlier wallet attempts are
also confirmed absent. Installing the native tools created no default cluster
or background service.

Evidence is under `audit-wallet-integration/`: `postgres-20260917T110722Z/`,
`gateway-20260917T111736Z/`, `regression-20260917T112122Z/`, and
`final-20260917T112159Z/`. Native per-target logs are in the sibling
`aether-admin-audit-ci.gO3bMw/`. Initial failed fixture/format checks are retained.

Parent #255 retains other mutation families, cross-store reconciliation,
delivery-outbox retention and deployment acceptance. Production
backup/PITR and published revision checks remain separate from these local tests.

## Operator delivery checkpoint, 2026-09-18

Protected list/summary and single dead-letter redrive endpoints are implemented
under `/api/admin/monitoring/audit-deliveries`. Listing requires monitoring read
permission; redrive requires monitoring admin permission. The bounded keyset
page omits event payloads. Redrive resets one dead-letter delivery for another
delivery attempt, preserving its event identity and facts, and does not replay
the original mutation. Concurrent redrives have one winner; stale leases cannot
acknowledge the new attempt. Bounded-label delivery metrics expose queue state,
unresolved age and redrive outcomes.

The native PostgreSQL 15.19 runner passed 38 exact invocations: 15 repository
tests with 15 fresh migrations, five Gateway HTTP/crash parents and three
memory tests. The authenticated HTTP parent exercises list/redrive permissions,
payload exclusion and operator action logging. The explicit operator tests
exercise concurrent redrive and stable pagination across tied timestamps and
concurrent inserts. Every invocation executed one test with zero ignored
results; the owned database server stopped after the run.

Evidence: `/Users/jinpeng/.agents/tmp/aeris-followup-20260918/live-gates/`
(`admin-audit-live.log`, `admin-audit-summary.json`), with per-target logs in
the sibling `aether-admin-audit-ci.rKzo9z/` directory. Reconciliation and
delivery-outbox retention remain unimplemented at this checkpoint. Existing
canonical audit-log cleanup is a separate retention mechanism.

## Paired retention checkpoint, 2026-09-18

The existing canonical audit cleanup now removes an expired audit row and its
delivered outbox row in one PostgreSQL transaction. The cutoff uses the
canonical audit timestamp, not the later delivery timestamp. The return value
still counts canonical audit rows, and each call deletes at most its requested
limit. Ordinary audit rows without an outbox entry remain eligible.

Pending, leased and dead-letter deliveries protect their canonical audit rows
from cleanup. Delivered orphans and payload identities that are absent,
non-string or different from `event_id` are retained. Eligibility validates
the payload identity; it does not revalidate every historical payload field.
The two candidate queries use `SKIP LOCKED`, followed by global timestamp/id
ordering and truncation. This bounds the deletion batch and returned candidate
pages; it is not a bound on PostgreSQL's scanned rows or physical row locks.

Six exact PostgreSQL tests passed. They cover zero limits, timestamp/id ordering
across both candidate classes, strict cutoff and batches, retained unresolved
states/orphans/invalid identities, a later eligible row behind invalid
identities, and transaction rollback when the canonical DELETE fails. Real
database barriers ensure two cleaners overlap and reclaim disjoint pairs;
separate tests hold a delivered-row lock and pause a live delivery before its
canonical INSERT to verify cleanup skips the lock and reclaims after release.

The registered native runner passed **50 exact invocations**: 21 PostgreSQL
targets plus 21 fresh migrations, five authenticated HTTP/crash parents and
three memory targets. Every invocation executed one test with zero failures or
ignored results. Its fixture checks all six retention target identities and
the full 26-database inventory, in addition to command/summary failure handling.
Strict PostgreSQL all-target/all-feature Clippy and formatting passed.

Evidence is in `/Users/jinpeng/.agents/tmp/aeris-retention-20260918/`:
`native-audit.log`, `native-audit-verification.json`, `validation-summary.json`
and `cleanup.json`. The owned PostgreSQL server was confirmed stopped and its
database files were removed, reclaiming about 586 MiB; per-target logs remain.
These changes are locally validated and uncommitted. Following the user's
2026-09-18 instruction to prioritize core code delivery, additional cross-table
reconciliation and expansion across every mutation family are deferred
hardening. They do not block completion of the original issue's code scope.

The remaining native confirmation dialogs in BillingPlansManagement,
WalletsManagement, GeminiFilesManagement and AsyncTasks now use the existing
`useConfirm` danger/warning dialogs. Existing API calls and cancel behavior
are preserved. Frontend type checking and targeted lint passed, with only
pre-existing template-formatting warnings. The original audit persistence,
protected forensic UI and Docker upgrade safeguards are implemented;
integration and deployment acceptance remain separate stages.
