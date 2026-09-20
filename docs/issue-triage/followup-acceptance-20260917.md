# Follow-up acceptance, 2026-09-17

This records local evidence for the uncommitted work on
`codex/integrated-approved-slices`, based on `26638a4f7`. Hosted checks on that
base commit do not validate this working tree. Parent issues remain open until
their remaining gates are evidenced.

The current issue-to-task mapping and subsequent queue are maintained in
`current-issue-task-list-20260917.md`; old delivery checkpoints do not override
that evidence-based mapping.

The latest checkpoint is `followup-acceptance-20260918.md`: protected audit
delivery operations and the 38-invocation native runner are now accepted,
three-node pass17 passed, and both real local monitoring delivery scenarios
passed. The dated sections below retain their original scope and counts.

## Recharge follow-up checkpoint

The PostgreSQL inventory now contains 57 exact targets, all executed with
`1 passed / 0 failed / 0 ignored`. This includes 20 new recharge-recovery cases
and a real JSONL import/restore regression. Evidence is in
`postgres-recharge-final.summary.json` under the task log directory below.

- Refund creation, execution and failure rollback now lock payment before
  wallet. Scoped recovery claims wallet/key locks without waiting; real
  database barriers prove contention rolls back and later replay collects once.
- A credit atomically freezes candidate membership. Real transactions prove
  that a callback begun before activation can credit afterward, while requests
  that become debt or commit after the credit cannot use its old budget.
- FIFO, partial principal collection, two independent recharge budgets,
  gift/hold protection, standalone ownership, rollback, sequence fencing,
  persisted retry intervals and notification leases passed.
- Missing or changed candidate identity/mode/evidence, changed settled prices
  and an aggregate beyond exact JSON integer precision require review. Real
  retention can remove fully paid usage and its snapshot while keeping receipts
  and allowing the remaining authorized debt to be collected.
- Notification claims and skipped delivery do not consume failed-delivery
  retries. JSONL restore suppresses enqueue only within its transaction;
  successful import, replay and rollback leave subsequent live callbacks active.
  JSONL still does not contain the complete financial ledgers; full PostgreSQL
  backup remains the required complete-recovery boundary.

The three synthetic supplier receipt targets each passed using real HTTP and
PostgreSQL: the amount/specification matrix, a catalog price change with a frozen
quote, and a truncated response followed by a late receipt and replay. Gateway
recharge API/notification checks passed 11 tests, including real SMTP failure
and recovery. Frontend recharge/API/money-format checks passed 23 tests, with
type checking and targeted lint passing. Automation passed 232 tests.

Both live-test harness fixtures reject zero/ignored tests and preserve
Cargo/log-writer failures. The required Gateway inventory passed all 22 exact
targets, including the real credit-to-wallet-history-to-authenticated-HTTP-to-
SMTP test. The fixture's explicit module path and scoped private-database
cleanup have been corrected. Additional Gateway checks passed: recharge 11,
notifications 8, full video filter 115 and full async filter 23. Evidence is in
`gateway-recharge-final.summary.json` and
`gateway-regressions-recharge-complete.summary.json`. Strict Gateway Clippy
(`--lib --bins --examples`) and changed-data/runtime-crate Clippy
(`--all-targets`) passed with `-D warnings`. Formatting issues were fixed;
`cargo fmt --all -- --check` and `git diff --check` now pass. Evidence is in
`lint-recharge.summary.json`. The earlier checkpoint below is retained with
its original scope.

## Native financial-ledger restore checkpoint

The new exact PostgreSQL target passed with `1 passed / 0 failed / 0 ignored`.
It used a full native custom archive and an empty restore database, compared
99 public tables row-for-row, and verified schema, sequence state and the
deferred recharge trigger. Nonempty jobs, candidates, operations, receipts,
notification retry, frozen quote and Unknown hold survived. Three callback
replays, pending-job execution, notification ACK replay and a synthetic late
receipt preserved financial idempotency and the original candidate budgets.
Both dedicated databases were removed, confirmed again from `pg_database`.

The required PostgreSQL inventory now has 58 targets: the prior 57-target run
plus this new individually executed target. The schema-comparison guard,
strict PostgreSQL Clippy, formatting/diff checks, live-runner fixture and all
10 Rust-CI selection tests passed. See `native-ledger-restore.summary.json`,
`native-restore-static.summary.json`, and
`docs/operations/native-financial-ledger-restore.md` for retained artifacts and
the exact scope. Production backups, PITR and production RPO/RTO remain open.

## Refund notification follow-up checkpoint

Refund terminal transitions now enqueue one notification event in the same
PostgreSQL transaction. All four new exact PostgreSQL targets passed, covering
the three terminal-branch rollback cases, concurrent replay and restart,
expired/old lease fencing, retry budgets, and changed owner/state quarantine.
The three existing payment-before-wallet lock regressions passed again. The
new native restore exercise also passed with 100 public tables and four
nonempty refund notification states, including an active lease. Restored
notification work and four terminal replays did not change money or add events.

These results are retained in `refund-postgres.summary.json` and
`native-ledger-restore-refund.summary.json`. The required inventories now contain
62 PostgreSQL and 24 Gateway exact targets. The final refund batch executed
both complete inventories successfully, with exactly one passed/non-ignored
test for every target. Both harness fixtures, strict Gateway and changed-crate
Clippy, formatting and diff checks passed. Ordinary refund regressions passed
34 tests; the two ignored SMTP targets were executed by the exact Gateway
gate. The prior 58/22 results above retain their original scope.

`refund-final.summary.json` and its per-command logs record all nine passing
checks. This supersedes the initial failing no-template email tests: the worker
omitted its fixed failure explanation, and the legacy fallback omitted refund
details. Both fallback bodies now include the safe details and fixed failure
wording. The unchanged real SMTP assertions passed for text/plain and text/html,
including exclusion of raw administrative diagnostics.

Non-outbox backends keep their existing direct notification fallback, with a
second capability check preventing PostgreSQL double dispatch. Administrative
failure reasons can contain private diagnostics, so the outbox and email carry
fixed user-facing wording; original refund diagnostics remain in their record.
Gateway SMTP/consent/fallback and final static checks have passed locally.
The next implementation is #255's first atomic audit mutation family, still a
temporary v2 draft. The refreshed task list maps all 48 open issues, including
new upstream-sync alert #449 and the ten roadmap dependency reviews.

## Atomic system-config audit follow-up checkpoint

The first #255 mutation family is now locally accepted. PostgreSQL commits the
system-config change and durable event together; enqueue failure rolls both
back. The supervised worker retries only audit delivery, uses database-clock
lease fencing and commits audit INSERT plus delivered ACK atomically. This is
one family from the 140-event inventory, not completion of the parent issue.

Four exact PostgreSQL delivery targets and their fresh bootstrap/migrations
passed. They cover concurrent claims, stale/expired tokens, an INSERT blocked
across real lease expiry with rollback, malformed payload failures and bounded
dead-letter transition. The authenticated Gateway HTTP target passed, including
enqueue rollback, secret exclusion, failure/recovery and immediate-write timeout.

The extended real subprocess target passed with one executed, non-ignored test
in 33.35 seconds. Three real SIGKILL phases cover before business commit, after
business commit, and after the worker's durable claim. A fourth PID starts while
the original 30-second lease is still live, waits for its actual expiry and
recovers with a different token. One audit is delivered; no business write is
replayed and its complete timestamps are unchanged. The harness explicitly
terminates the dead child's identified PostgreSQL backend before releasing its
barrier; SIGKILL alone is not asserted to cancel already-running server SQL.

Ordinary audit tests passed 76 cases. The three ignored entries are the two
separately executed live parents and their protected subprocess helper.
Native restore passed with 101 public tables, four synthetic audit states,
all previous recharge/refund replay checks and dedicated-database cleanup.
The retry fixture is persisted pending state with nonzero attempt count.

After production integration, all 62 PostgreSQL and 24 Gateway exact targets
passed again, each with one executed test. Task-runtime tests passed 9 and
ordinary refund tests passed 34; the two SMTP cases ignored by that filter are
included in the exact Gateway gate. All three live-runner fixtures, strict
Gateway/changed-crate Clippy and 232 automation tests passed. An import-formatting
difference stopped the first final-check run; its correction and the test-only
worker-crash extension then passed workspace formatting and diff checks.
The original failed checkpoint remains retained rather than rewritten.

Evidence under the task log root: `audit-postgres.summary.json`,
`audit-gateway.summary.json`, `audit-worker-crash.summary.json`,
`native-ledger-restore-audit.summary.json`, `audit-final.summary.json`,
`audit-final-recheck.summary.json` and `automation-audit.log`.
The socket-only CI runner has fixture coverage locally; live tests here used
the Docker PostgreSQL wrapper, not a local initdb/pg_ctl deployment.

The next #255 candidates are single/all session revocation, wallet adjustment,
manual recharge and group-member replacement: five events across three bounded
slices, followed by operational delivery/reconciliation work. Seven-path
upstream conflict drafts are prepared but not integrated or Cargo-validated.
The live 48-issue inventory still has zero omissions in the task plan.

## Upstream content integration checkpoint

The complete merge-tree against upstream `681ce56c4f27` was resolved in an
isolated candidate, including seven conflict paths and all automatic changes.
The generated eleven-byte Windsurf test artifact was excluded. The final
31-path patch was applied locally with exact tested-source hashes, preserving
163 other existing changed files. HEAD and index stayed unchanged; no merge
commit or remote write occurred.

Review found and corrected reasoning fallback data loss and multipart Gemini
citation attribution, including Responses text-coalescing offsets. Eight new
semantic regressions passed in the full 979-test formats suite. Existing
terminal observer memory behavior and actual stream emission order are covered.
Unmappable citation spans are dropped; this does not infer cross-part identity
that the provider did not supply.

The final candidate passed 379 usage-runtime tests, Gateway video 115 and async
30, and the complete 63 PostgreSQL / 24 Gateway exact inventories. The newly
added PostgreSQL body-state test exercises request-ID, ID and batch-ID read APIs
with distinct None/Truncated/Disabled/Unavailable state assignments. PostgreSQL
usage's ordinary filter passed 113 tests and left 13 live targets ignored; the
63-target gate separately executes its selected live cases. Strict Clippy,
formatting, diff, harness fixture and lockfile consistency checks passed.
Frontend changes passed 30 tests, type checking and targeted lint.

The initial candidate additionally passed provider Antigravity 18 and video
core 54 tests. Its 971-format-test and 62-PostgreSQL-target results are retained
as the earlier checkpoint, not substituted for the corrected final results.
Evidence is in `upstream-candidate-evidence/`, especially `final/`,
`adoption-manifest.json` and `adoption-verified.json`.

After validation, Cargo cleanup removed 249 completed Gateway/formats build
files, reporting 2.4 GiB reclaimed. The reproducible merge-tree tar archive was
also removed. Source candidates, adoption patches, hashes, logs and shared
dependency artifacts needed for subsequent work remain available; see
`upstream-candidate-evidence/cleanup.json`.

Subsequent session/group audit families remain draft work. Session v2 includes
single/all rollback and actual delivery failure/retry; a separate authenticated
HTTP draft covers JWT validity and marker behavior. Group review found a
concurrent replacement race; the v3 draft now has a frozen concurrency increment
with stable membership locking, bounded retries and four independent PostgreSQL
regression sources. Static checks passed, but integration and execution of those
targets in separate fresh migrated databases remain. None of these new families
has passed Cargo/live acceptance yet.

## Session/group audit integration checkpoint

The three selected events are now integrated locally with optional backend
capabilities, same-transaction audit enqueue and stable response markers.
Eight new PostgreSQL targets passed, each with one executed test and its own
fresh migrated database. Session coverage includes single/all rollback, actual
audit INSERT failure/retry, unchanged business rows, concurrent user deletion,
and ordering behind a real password-login transaction. Group coverage includes
rollback/retry, concurrent empty-group replacements with a late member,
deletion before an empty replacement, a later per-user CAS, and contention
exhaustion without business or audit writes.

Review added a user-row lock and typed NotFound to revoke-all; a concurrently
deleted target cannot produce a successful intent. This orders competing login
transactions without forbidding new logins that linearize after revocation.
The group replacement acquires affected user locks with NOWAIT before locking
the group; newly discovered members cause a full rollback and bounded retry
before any membership or intent write. Default-group policy across transactions
and cross-Gateway cache consistency are separate remaining work.

The first session test incorrectly assumed the rescheduled failure preceded
older pending audit events. Its correction identifies that event within the
claimed batch and additionally rejects the old lease token. The first hardening
compile found an unchanged inherent method return signature; it was corrected.
Both failures are retained. Passing PostgreSQL evidence is in
`audit-families-integration/postgres-20260917T104642Z/`.

CI registers 18 business targets and 12 isolated migration runs, with 16 unique
databases and two memory targets that clear database environment variables.
The shell fixture passed, as did all 232 automation checks. Both new authenticated
HTTP targets and both memory fallback targets passed individually. They verify
JWT validity across rollback/commit, authorization, stable IDs, safe payloads,
delivery failure/retry and unchanged business effects. The first memory group
test hit the existing backend-free test state's read-only guard; the corrected
fixture provides the same authenticated actor in its user store while memberships
and sessions continue using the memory repository. The successful run is in
`audit-families-integration/gateway-20260917T104740Z/`.

Focused Gateway regressions passed 59 group tests and 177 session tests; each
filter ignored its separately executed PostgreSQL-backed HTTP case. The narrower
`tests::audit::` filter passed nine ordinary cases and ignored five live/helper
cases; it is not a replacement claim for the older full `audit` filter. The new
batch did not rerun the unchanged generic crash/lease exercise.

The complete 63 PostgreSQL/24 Gateway exact inventories passed on the resulting
source. Strict Gateway Clippy included lib/bins/examples/tests; data Clippy used
all targets and all features. Formatting, diff checks and all three live-runner
fixtures passed. These results are in
`audit-families-integration/final-20260917T105235Z/`. All eight newly provisioned
PostgreSQL tests and both HTTP tests used dedicated disposable databases.

Final verification found no drift across the 23 accepted source/CI paths; their
snapshots and all 210 changed-file hashes are retained in
`audit-families-integration/accepted-evidence-index.json`. All 18 databases owned
by this batch, including failed attempts, were confirmed absent after cleanup.
HEAD and the empty staging index were unchanged. Package-scoped Gateway cleanup
then removed 110 build files, reclaiming about 449 MiB of allocated disk space;
the shared target remains about 4.2 GiB. Source, patches, logs and dependency
artifacts are retained in the task directory.

Wallet adjustment/manual recharge was still an isolated draft at that
checkpoint; the following checkpoint records its subsequent integration.
None of these slices closes parent #255.

## Wallet audit integration and native CI checkpoint

The wallet/ledger/order mutation and administrator audit intent now commit in
one PostgreSQL transaction. The old optional adapter fallback is retained, and
the original monetary transaction bodies preserve their replay contract.
Real database acceptance enables recharge recovery and seeds synthetic legacy
debt. A deferred candidate INSERT failure reached COMMIT and rolled back all
business, recovery and audit effects; a nontransactional sequence witnesses
that the deferred trigger ran. A new order number with a reused audit ID hits
the audit uniqueness constraint. Fourteen classes of persisted facts verify
that audit delivery retries never replay money or recovery writes.

The PostgreSQL target passed in `audit-wallet-integration/postgres-20260917T110722Z/`.
The first fixture omitted required wallet timestamps; both database/HTTP seed
rows were corrected, and the failed run is retained. Authenticated HTTP and
memory fallback passed in `gateway-20260917T111736Z/`. Post-commit quota-read
failure returns the pre-existing 502/control_unavailable/Retry-After contract,
not the fixture's original 500 expectation. The money and intent survive;
delivery recovers the audit only. A repeated HTTP recharge remains a new credit.
The earlier failed 500 assertion remains in `gateway-20260917T110820Z/`.

Focused regressions passed 139 Gateway wallet tests and 25 data wallet tests
in `regression-20260917T112122Z/`. Five separately scoped Gateway live cases were
ignored by the ordinary filter and are not counted as executed there.
The final 63 PostgreSQL/24 Gateway exact inventories, all three runner fixtures,
and strict Gateway/data Clippy passed in `final-20260917T112159Z/`.
Its final formatting check caught only a wrapping difference in the newly added
response assertion. The correction and diff check are recorded separately;
the original failed formatting result is retained.

PostgreSQL 15.19 native tools were installed without a default cluster or service.
The actual socket-only `run_admin_audit_live_tests.sh` then passed all 34 exact
invocations: 21 business targets plus 13 migrations, each one executed with no
ignored tests. Five HTTP/crash parents and 13 repository tests used 18 distinct
databases in a private cluster; three memory targets cleared DB variables.
This includes the generic process-kill/lease case as an actually rerun gate.
Logs remain in `aether-admin-audit-ci.gO3bMw/`. The cluster was confirmed stopped
and removed, reclaiming 416448 KiB (about 407 MiB). Four Docker databases owned
by earlier wallet attempts were confirmed absent. Sources and all failure logs
remain available. The shared Cargo artifacts are retained for the next
readiness/testkit integration, avoiding an immediate rebuild of the same crates.

Independent operations review selected protected delivery status/summary,
single dead-letter fenced redrive and low-cardinality metrics as the next
bounded #255 slice. Existing expired-lease recovery and canonical audit-log
retention already work; outbox retention and cross-table reconciliation remain
separate work. The readiness v3 draft passed independent static review but has
not yet entered the integrated runtime checkpoint described here.

## Earlier checkpoint: implemented changes

| Issue | Result and boundary |
| --- | --- |
| #211 | Persisted video revisions and poll claim tokens reject stale writers, including same-second races. Registry publication follows the accepted database row; conflicts reload it. Native xAI display data survives accepted writes and can be refilled after restart without changing terminal lifecycle. A local projection failure does not overwrite a successful upstream operation's usage with an abort failure. Rollout requires draining old writers; see `docs/operations/video-task-revision-migration.md`. |
| #223 | Capacity gauges reach queue health and authenticated Gateway metrics; warning and critical rules preserve routing labels. Real loopback Prometheus/Alertmanager delivery now tests the unchanged DLQ boundary rule, both firing and resolved notifications, and observed capacity samples 0/1/0. Production scrape/receiver delivery and production backup acceptance remain separate. |
| #216 | The live PostgreSQL harness requires exactly one executed, non-ignored test per inventory entry and preserves Cargo/tee failures. The required inventory now contains 36 targets. |
| #220 | A release builds one OCI archive, validates and scans both runtime architectures, and publishes that same archive only after both pass. Scanner/tool digests, DB freshness and image/report identity are checked; failure artifacts are retained. Local registry validation uses synthetic Debian images, not the final production image. |
| #300 / #206 | The legacy pending-request sweeper now excludes all attempt-funded parents, including zero-hold parents with known charges, open admission or reconciliation. This prevents timeouts from resetting their billing snapshot to void/zero. A separate real subprocess kill/restart test exercises explicit recovery; it does not add automatic debt collection or a durable orphan-recovery worker. |

## Earlier checkpoint: verified local evidence

All database work used disposable databases in a loopback-only PostgreSQL
15.19 container. Production was not modified.

- PostgreSQL required inventory: 36 exact targets, each `1 passed / 0 failed /
  0 ignored`, including the video claim/revision/summary/anonymization checks,
  payment callbacks and the new stale-pending regression.
- The new pending-sweeper test preserves complete parent, settlement,
  reservation and candidate records across six attempt states. Two younger
  legacy requests still complete cleanup with batch size one; a repeat is a
  no-op. An independent review confirmed the query's row locks also cover
  concurrent transitions to attempt funding.
- Video core: 53 passed. Video Memory repository: 16 passed. PostgreSQL video:
  10 passed, including the real database test; auth contracts: 6 passed; user
  anonymization contract: 1 passed.
- Gateway video regressions: 115 passed; async-task regressions: 23 passed;
  queue-health regressions: 3 passed, including authenticated HTTP metrics;
  unsigned admin identity-header integration: 1 passed.
- The real subprocess crash target passed: signal 9, a different restart PID,
  one upstream dispatch, preserved Unknown hold and quote, explicit recovery,
  late synthetic receipt settlement, and no additional financial or counter
  effects after outbox cleanup and replay. An independent review found no
  blocking flaw in this evidence chain.
- The authenticated backup/restore CLI drill passed against a fresh disposable
  database: one executed test, no failures or ignored tests. The synthetic
  exercise verifies credentials, wallets and aggregates; it does not validate
  an arbitrary production backup object.
- Runtime capacity arithmetic and queue retention: one test each passed.
  Real Redis DLQ coverage: 2 passed without ignored tests. The alert rule tests
  passed with Prometheus 3.14.0.
- DLQ delivery: real Prometheus 3.14.0 and Alertmanager 0.34.0 delivered firing
  and resolved webhook v4 notifications with all required labels. The tested
  rule-file SHA-256 is
  `36e0f63a1e5483c20b6a96728837710f8087c1767a9630698bfa01eb41b1d92f`.
- The existing billing alert delivery scenario also passed after the shared
  harness changes: authenticated scrape and real firing/resolved delivery for
  both daily-quota and RPM failures.
- Image gate: 15 executable fixtures plus release workflow contracts passed.
  Real Trivy 0.74.0 scanned both synthetic architectures using one pinned DB;
  fixed Skopeo then copied both tags to an isolated local registry. Remote
  index bytes and round-trip child/config digests matched. No external registry
  writes occurred. The index was
  `sha256:3122547b08df40c67f2904fedbf5e547f968fa70291576e4570606f76a2b87f5`.
- Full supply-chain shell fixtures passed, including 11 Rust signature tests
  and signer overlap, switch, retirement and tamper rejection. Workflow lint,
  formatting and diff checks also passed at this checkpoint.

These results describe the checkpoint before the new recharge-recovery and
synthetic receipt-matrix changes. The user selected automatic recharge-triggered
debt recovery and authorized synthetic supplier data for the local acceptance.
The concrete collection, retry and notification contract is in
`issue-206-recharge-recovery.md`; its separate evidence is recorded in the
recharge follow-up checkpoint above. Final Clippy and source checks remain
required on the resulting candidate.

## Retained evidence and remaining gates

Task logs are under `/Users/jinpeng/.agents/tmp/aeris-followup-20260917/`.
The image registry evidence is under
`/Users/jinpeng/.agents/tmp/image-gate-registry-bv714cxg/`; its
`validation-summary.json` records local-only scope and both architecture
digests. The temporary registry/network/image and its scan cache were removed.
The main scan cache was also reclaimed (1,403,523,222 bytes); the active Rust
target is retained only until the remaining tests finish.

Outstanding work includes required hosted checks on the eventual submitted
revision, a final
release-image scan, and deployment/rollback evidence. Automatic recovery is
implemented with passing PostgreSQL acceptance, and all three new synthetic
receipt targets passed; neither should be reported as unstarted work. Provider
sandbox credentials are not required for the local acceptance the user
selected. All fixture receipts remain labeled synthetic; they do not establish
an external supplier's actual invoicing behavior.
