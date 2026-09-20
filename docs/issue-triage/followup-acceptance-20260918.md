# Follow-up acceptance, 2026-09-18

This checkpoint covers the uncommitted working tree at
`/Users/jinpeng/workspace/aeris-token`, based on `26638a4f7`.
PR #447 still points to that base; its hosted checks do not cover these local
changes. GitHub currently has 45 open issues and two open PRs, confirmed by
inventory evidence at `/Users/jinpeng/.agents/tmp/aeris-inventory-20260918-luna/`.
The earlier checkpoints did not change Issue or Project status. The final
user-requested pause handoff updates issue #443 and related Issue/PR comments;
Project fields remain unchanged because the token lacks write scope.

## Executed gates

The evidence root below is
`/Users/jinpeng/.agents/tmp/aeris-followup-20260918/`.

| Scope | Executed result | Evidence relative to root |
| --- | --- | --- |
| PostgreSQL live inventory | 63 exact targets passed, zero failed or ignored; disposable PostgreSQL 15.19. Includes the otherwise-ignored video capture/claim/completion target. | `live-gates/postgres-live.log`, `live-gates/summary.json` |
| Gateway funds/recharge/refund inventory | 24 exact targets passed, zero failed or ignored. Includes real local HTTP, SMTP failure/recovery, process death/restart, and late synthetic receipts. | `live-gates/gateway-live.log`, `live-gates/summary.json` |
| Administrator audit native runner | 38 exact invocations passed, zero failed or ignored: 15 repository tests, 15 fresh migrations, five Gateway HTTP/crash parents and three memory targets. | `live-gates/admin-audit-live.log`, `live-gates/admin-audit-summary.json` |
| Capacity error semantics | One Rust regression passed across seven loopback HTTP cases: 429, 500, 502, 503, two mixed cases, and healthy 2xx responses. | `live-gates/capacity-non2xx.log` |
| Billing alert delivery | Real Prometheus 3.14.0 and Alertmanager 0.34.0 delivered both firing and resolved notifications for daily quota and RPM. Authenticated scrape passed and missing credentials failed. | `metrics-billing/result.json`, `metrics-billing/webhooks.json` |
| DLQ alert delivery | Real authenticated scrape, unchanged checked-in DLQ boundary rule, and firing/resolved webhook v4 delivery passed with preserved routing labels. | `metrics-dlq/result.json`, `metrics-dlq/webhooks.json` |

The current Cargo tests use the main working tree. An earlier Python arithmetic
fixture and build in the older issue212 worktree are exploratory evidence only;
they do not substitute for the executed Rust regression above. That regression
calls `run_http_load_probe`, `capacity_point` and `detect_saturation_point` on
real, complete HTTP responses. It checks exact successful/failed/rejected counts
and successful throughput. Fully read 429/500/502 responses cause saturation;
503 is an admission rejection. The healthy control does not saturate.
This is classifier coverage, not a deployment capacity measurement.

The native audit cluster was stopped normally after all 38 invocations. Its
per-target logs are in `aether-admin-audit-ci.rKzo9z/`. The HTTP parent also
executes the protected operator list/redrive checks. The two explicit operator
repository tests verify single-winner redrive, payload preservation, stale
lease fencing, and keyset stability across tied timestamps and new inserts.
The subprocess test kills before commit, after commit and after claim; recovery
in a new process waits for the original lease to expire and produces one audit
without replaying the business write.

## Three-node and component evidence

`three-node-pass17/evidence/drill/summary.json` reports `overall_pass=true`:

- Four relay cases each completed 200/200: direct owner, remote owner,
  streaming remote owner and recovered remote owner.
- Baseline completed 200/200, draining completed 100/100, and recovered Redis
  completed 50/50 with HTTP 200.
- Redis fault produced the expected 20 HTTP 503 responses. Lease loss ended
  32 SSE streams with `sse_incomplete`; these intentional failures are not
  counted as successful service.
- Financial reconciliation recorded 386 requests, 350 completed, 32 cancelled,
  four failed, zero duplicate requests, zero pending usage outbox and zero
  invariant violations. The synthetic settled cost total was 3.82.

The metrics authentication fix prevents `/_gateway/metrics` scrapes from
creating management-token usage deltas. Redis downtime still reports queue
health unavailable instead of inventing zero queue gauges. Recovery waits for
two healthy scrapes before the recovery load begins.

The #211 component logs under `/Users/jinpeng/.agents/tmp/issue211-tests/`
record video core 54 passed, Gateway workers six passed and runtime state
131 passed/one ignored timing test. PostgreSQL structural video/lease checks
passed nine/three tests respectively; their ignored live targets are not
counted as passes. The video live target subsequently passed in the 63-target
gate above; administrator audit lease tests passed in the native audit gate.
These counts do not establish all Gateway video recovery behavior.

The tunnel package test completed with 225 passed, zero failed and one ignored
documentation-regeneration target. Supply-chain fixtures retained under
`/Users/jinpeng/.agents/tmp/aeris-supply-chain-20260918/` passed the signature,
release-image gate, rotation workflow and installer-link checks.

## Acceptance boundaries and next work

Both monitoring scenarios use synthetic local sources. Billing uses a
10-second increase window and two-second pending period; it does not prove
production timing. DLQ retains the checked-in rule file SHA-256
`36e0f63a1e5483c20b6a96728837710f8087c1767a9630698bfa01eb41b1d92f`.
The downloaded Darwin archives were verified against the release checksums:

- Prometheus: `a9623f7f4fe65b1b171b423c1a72bbf23dfdf41a171dcb33e7dd302af80dc01c`.
- Alertmanager: `0cb31efe439c58ab77594b62a28f9de95f2d08beccf0f11eba4b822b2f549b82`.

The required billing failure producer call sites were subsequently confirmed
present; do not reschedule that code as missing. Deployment scrape/receiver evidence, actual
release-image scanning/publication, production PITR/RPO/RTO and deployed key
custody remain separate gates. Synthetic supplier receipts meet the selected
local testing scope without establishing actual supplier invoicing.

The automatic recharge collection contract is implemented and locally verified;
it is not waiting for the original finance-choice question. Signup/referral
policy and supplier cost provenance remain distinct tasks. The supplied
snapshot contains no referral reward rows or wallet transaction export, so it
does not complete the four-query #316 historical audit or authorize backfill.

The subsequent #255 retention batch passed 50 exact native audit invocations,
including six new retention targets. Four remaining administrator pages now
use the existing confirmation dialogs; frontend type checking passed. The
user's updated priority is to finish original core code requirements, so
additional reconciliation and blanket mutation-family expansion are deferred
hardening. Already implemented functionality must not be scheduled again.
Rust artifacts are reclaimed after each completed batch; raw evidence is
retained. These gates do not close the parent issues or synchronize local
changes to GitHub.

The latest upstream delta is also integrated into the local follow-up files,
including the PR #450 video conflict fixes while retaining local revision/CAS
behavior. Focused suites passed: formats 981, transport 528, Gateway
normalization 17, video core 55, Gateway video 115 and Gateway async 12.
Strict data/protocol/Gateway/video Clippy and formatting passed. Evidence is
in `/Users/jinpeng/.agents/tmp/aeris-retention-20260918/validation-summary.json`.


## Core delivery integration checkpoint (2026-09-18, afternoon)

The isolated PostgreSQL core batch passed three exact targets: fresh schema
bootstrap/migration, strict endpoint health-score decoding with legacy/NULL
compatibility, and provider-cost import/version/summary behavior. Each executed
one test with zero failures and zero ignored cases. The cost fixture covers
replay, conflicting import content, missing/expired price references, separate
price versions, known/estimated/unknown costs, same-currency margin, and unknown
or cross-currency NULL margin. Evidence:
`/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/pg-85kpb4n4/summary.json`.
The existing PostgreSQL CI runner now registers both new regression targets;
its gate fixture passed. Initial fresh-install duplicate-table and missing-test-
import failures were corrected before this passing run; their logs remain.
All three owned PostgreSQL clusters from this batch were stopped and removed,
reclaiming 165,291,561 bytes while retaining logs.

The current focused results are: notifications 10; auth registration 2 and
email-verification referral 2; referrals 17; scheduler 131; provider-cost
billing 5; provider-cost contracts 2; send admission 4; request attempt budget
37; trace context 6; trace metadata 10; and pool orchestration 53, all passed
with zero failures and zero ignored cases. The local HalfOpen selection passed
3 with one ignored case, its owned real-Redis probe passed 1, and the
self-managed Redis ignored target passed 1 without `AETHER_TEST_REDIS_URL`.
Evidence is in the corresponding `*-latest.json` files and
`redis-probe-dmfpyj9d/summary.json` under
`/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/`.

The public-models selection passed 33 tests with zero failures and zero ignored
cases (`public-models-latest.json`, with the executed log at
`public-models-20260918T173503.log`). Formats full passed 997; Gateway
classifier 31, recovery 9, fallback 27, history 4, and effects 61 all passed
with zero failures and zero ignored cases. Evidence is in the corresponding
`*-latest.json` files under
`/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/`. The authenticated
provider-cost HTTP acceptance passed its one exact target against a fresh
task-owned PostgreSQL cluster, including the authenticated readonly 403,
administrator import, replay/idempotency, invalid-window rejection, and
unknown-cost NULL behavior. The cluster was stopped and removed after the run,
reclaiming 55,571,143 bytes; see `pg-dzcqjsbu/summary.json` and
`pg-dzcqjsbu/cleanup.json`. Chat sync smoke passed its one exact target and
chat stream smoke passed four exact targets; see `chat-sync-smoke-latest.json`
and `chat-stream-smoke-latest.json`. Sync auth, sync 500, sync 429, and stream
429 routing selections each passed one exact target with zero failures and zero
ignored cases; see their `*-latest.json` files. Strict Clippy passed for the
seven-crate set (`clippy-20260918T180135.log`) and for `aether-ai-formats`
(`formats-clippy-20260918T180623.log`), both with `-D warnings`.

The fresh PostgreSQL gate at `pg-s6ud_mij/summary.json` passed both the fresh
migration and disabled-model declaration-query targets, one test each with zero
failures and zero ignored cases. Its owned cluster was stopped and removed,
reclaiming 55,236,451 bytes (`pg-s6ud_mij/cleanup.json`).

The #48 patch preserves the public immediate-memory behavior, with formats full
and strict Clippy recorded above; this record does not claim that every KV
failure is excluded from cache writes. #49 has sync and streaming trusted-origin
classification, while effects, replay, and WebSocket coverage remain pending.
These statements record local implementation state and do not complete parent
issues.

The batch cleanup removed 10,210,656,012 bytes from
`target/debug/incremental` while preserving shared dependencies; see
`/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/incremental-cleanup.json`.
Automatic supplier-cost capture for every new settlement remains an unfinished
#431 requirement. The accepted provider-cost checks use synthetic supplier
data, as explicitly authorized for this delivery; they validate code behavior
without asserting real production supplier invoices or historical production
margin/profit data.

## Provider-cost native restore extension (2026-09-18, evening)

The native full-ledger restore target passed one executed test with zero
failures and zero ignored cases after adding a nonempty provider-cost fixture.
It restored one versioned price and three separate synthetic cost snapshots:
Unknown with NULL cost fields, Estimated with frozen price provenance, and
Known with an explicit synthetic invoice reference. The existing full-table
comparison now covers 103 public tables. Three callback replays, four refund
replays, audit continuation and late settlement preserve the cost ledger.

Evidence is retained at
`/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/pg-_5ibtocy/summary.json`
and its `native-ledger-restore-e7a47b56bac24efcbb8b0982cb8c9d63/verified.json`.
The dedicated databases and stopped owned PostgreSQL cluster were removed;
`cleanup.json` records 64,861,663 bytes reclaimed. This extends local #223
acceptance to the imported provider-cost tables. It does not implement #431
automatic capture or establish production backup/PITR acceptance.

## Scheduler and cost reconciliation checkpoint (2026-09-18, evening)

The integrated #49 classifier now stops ordinary request errors by default,
retains explicit continue overrides, and permits default failover for trusted
credential 401/403 and upstream 408/429/5xx. Origin-aware sync/stream effects
avoid penalizing credentials for unproven authentication or request errors.
The real Router fixture verifies that a 422 reaches the primary once without
calling the backup, then confirms that a trusted 401 still reaches the backup.
Compact sync/stream candidate loops reject same-key and alternate-candidate
replay while preserving the fallback response. Other side-effect operation
policies remain unfinished.

Responses WebSocket quota retries require a trusted upstream quota admission.
For #46, a successful client socket write of a response or error event now
commits the logical turn and prevents a subsequent transparent quota retry.
Control events do not commit it. This is an application writer boundary;
the unified HTTP lifecycle remains unfinished.

#431 cost imports can now advance Unknown to Estimated/Known and Estimated to
Known without adding a second summary contribution. Sales facts stay fixed.
Immutable typed import receipts preserve the original estimate and allow an
old import to replay without downgrading the current invoice. PostgreSQL
acceptance covers price provenance, the invoice difference, replay/conflict,
unknown NULLs, and migration backfill repeated within a rolled-back fixture
transaction. Automatic runtime capture remains unfinished.

The final batch passed 414 tests across 18 selections: 403 Gateway focused
tests, six serving-loop tests, three PostgreSQL exact targets, one native
restore target, and one authenticated provider-cost HTTP target. Native restore
now compares 104 public tables, including nonempty immutable cost receipts.
The seven-crate strict Clippy run, workspace formatting, diff check and
PostgreSQL gate fixture passed. Initial compile errors and outdated default
retry assertions were corrected before these results; their logs remain.

Evidence: `/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/evening-checkpoint-summary.json`.
The final Clippy log is `clippy-20260918T185456.log`; PostgreSQL evidence is in
`pg-kqweznf3`, `pg-iegtejl1`, and `pg-g9cq0346`. The four owned PostgreSQL
clusters used during this evening batch were stopped and removed, reclaiming
240,330,015 bytes. Shared Rust dependencies remain cached. This checkpoint is
local and uncommitted; it does not represent a GitHub PR or deployment update.

## User-requested final closeout and pause (2026-09-18)

The already-started batch is finished; further implementation is paused until
a new user instruction. Do not continue the deferred queue or refill worker
slots. See `pause-handoff-20260918.md` for the preserved checkout and remaining
issue boundaries.

The closeout includes trusted origin/disposition trace persistence, HTTP
attempt lifecycle and request-wide body-handoff barriers, request-level cost
components with fixed-point estimation, and the default no-op post-settlement
capture hook. Gateway automatic provider-cost capture remains unconnected.
The heartbeat fixtures now meet production send-admission requirements and
exercise ordinary Responses retry separately from the existing Compact stop
policy. The Router readback exposed missing origin/disposition allowlist
entries; the final serializer retains only recognized enum values and drops
arbitrary nested fields.

The final focused selections passed **171 tests across 20 selections**:
48 billing/contract/settlement tests, 11 candidate-contract tests, 106 Gateway
tests, four exact PostgreSQL targets, one native restore target and one
authenticated cost HTTP target. The native restore compared 104 public tables.
These counts overlap with the earlier 414-test checkpoint and are not additive
unique coverage. Final strict eight-crate Clippy, workspace formatting, diff
check and PostgreSQL gate results are listed in
`/Users/jinpeng/.agents/tmp/aeris-core-delivery-20260918/pause-checkpoint-summary.json`.

Initial failed attempts and their corrected reruns are retained. Final
PostgreSQL evidence is in `pg-msk86tjt`, `pg-e5ha67w1` and `pg-9o6k0_ik`;
the preliminary `pg-07oim08t` fixture records the corrected quantity/amount
failure. All four owned clusters were stopped and removed after their run.
The worktree remains uncommitted and unpushed; GitHub receives the final
status and deferred-work handoff, not a code publication or deployment claim.
