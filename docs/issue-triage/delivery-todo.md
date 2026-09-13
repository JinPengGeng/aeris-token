# Issue delivery TODO and community workflow

Scope: fork `JinPengGeng/aeris-token`. Runtime changes go through reviewed PRs;
no upstream writes are part of this delivery workflow.

## Current delivery snapshot — 2026-09-13

Verified main: `1087c867e08aa517b28a5acfaf162c16ebf298e4` (PR #383 merged).
There are **44 open issues**: 21 P1 and 23 P2; lifecycle labels are
16 in progress, 18 triage, and 10 blocked. Parent and child issues overlap, so
these are inventory counts, not 44 independent implementation packages.

There are **15 open PRs**, including this checkpoint: #367–#369,
#371–#374, #376–#382 and #387. All fifteen have protected squash auto-merge enabled;
none is Draft. After each merge, remaining branches need current-base verification and
fresh required checks. GitHub is authoritative after this timestamp.

### Review decisions and next delivery actions

1. **Money correctness (#362/#300/#206):** the exact-hold refund/adjustment
   regression is fixed with integer-unit arithmetic. Eleven live PostgreSQL
   targets and twenty memory settlement tests passed. #362 passed all required
   checks on `f7b84853` and merged as `ab1dfa968`. Next integrate Gateway admission, dispatch, durable
   terminal settlement and recovery. Review identified independent attempt
   funding, authoritative output-cost evidence and unresolved-usage retention
   as required work; facade methods alone do not implement that lifecycle.
2. **Upgrade safety (#376/#255):** review fixes are complete in `dd18dd037`.
   Rollback selects the old immutable image, private backup bundles are checked
   before recreation, and health failures stop the upgrade. Automatic rollback
   requires an explicit schema-compatibility declaration. Stateful tests and
   hosted real Compose override merging passed; all four required checks passed
   on that head. Auto-merge is enabled, with branch synchronization still
   required. This does not establish a live app upgrade or full database restore.
3. **Historical audit (#380/#383/#316):** independent review found #383 used a
   PostgreSQL 16-only helper although deployment uses PostgreSQL 15.19. Commit
   `d3c0bee38` replaces it with real casts and a specific overflow handler.
   The original failure was reproduced on 15.19; the corrected standalone SQL
   and full migration/audit regression passed. The regression also passed on
   17.11; each run executed one test with zero ignored. Reviewed head `cbb285782`
   passed all required checks and merged as `1087c867e`. #380 records
   unknown historical impact; production aggregates remain missing.
4. **Reuse and queue cleanup:** #385 is closed because #318/#312 already supplied
   the durable Redis profile and hosted recovery test. #364 is superseded by
   this PR, which includes its full checkpoint changes. Close duplicate work
   with linked evidence instead of counting it as new implementation.
5. **Remaining verification:** #382's independent review verified the real
   daily-quota execution in job `103652493498` (one passed, zero ignored).
   #371 passed real Prometheus scraping and Alertmanager firing/resolved webhook
   delivery in hosted job `103661227086`; #384 is superseded. These PRs still
   require current-head protected checks. #382 is being synchronized after
   #383's merge. #377 covers peer-error retry timing.
6. **Upstream conflict completed (#276/#386):** all required checks passed on
   `ec47ebb59`; the merge commit has two parents and the exact reported upstream
   `60b89cc840d6d99972c15423c7655335c011c7ae` is now an ancestor of fetched fork
   main. #276 is closed; its stale lifecycle label was removed and its Project
   Decision is Accepted. Both Issue and PR cards are Done.
7. **Release-key rotation scope corrected (#205/#375):** #375 was closed after
   checking ADR-0045, the actual release envelope and verifier, and completed
   #345. Release `key_id` already exists; the missing work is multiple embedded
   public keys and staged overlap/retirement. Gateway tables and a new handshake
   header are not prerequisites for this requirement. The next P1/M slice is
   implemented in #387 with old/new/retired-signature tests and offline recovery.
   The production verifier is reused for release validation; actual compiled
   trust sets and real signatures prove overlap and retirement locally. Review
   also added a separate required advisory audit and Dependabot coverage for
   the helper's independent Cargo lockfile, with no RSA exception. #387 is
   reviewed and auto-merge enabled; complete platform builds and production
   rotation are not claimed. #375 is Done / Won't fix on the Project.
8. **Implicit signup grant removed (#253/#379):** absent configuration now means
   zero initial gift in local/LDAP/OAuth/admin fallback, settings and the admin
   form. Explicit grants and existing wallets are preserved; the form submits
   its displayed zero rather than inheriting a global promotion. The real HTTP
   registration matrix passes after merging #362, existing LDAP/admin nonzero
   regressions pass, and six frontend tests/type-check/lint pass. Independent
   review accepted the change; #379 is Ready with protected auto-merge enabled.
   HTTP fixtures use test repositories, not a live PostgreSQL deployment;
   financial lifecycle, promotion eligibility and provider-cost work remain.

Project #1 was re-read for all 44 open issues: every card has Status, Priority,
Area, Risk, Size and Decision. #362/#383 PR cards are Done / Accepted;
#376/#379/#382/#387 have complete fields and remain In review. Parent #253 is
In progress / Accepted, while #316 remains Inbox / Planned. A child slice does
not complete its parent. Closed
duplicate PRs #364/#384/#385 retain their linked closure reasons on GitHub.

A subagent mistakenly opened upstream PR fawney19/Aether#816. It was closed
without merging, recreated in the fork as #385, then rejected as redundant.
The mistake and correction are recorded in #218/#385; upstream received no code
merge. All further GitHub mutations must name `JinPengGeng/aeris-token` explicitly.

## Operating rules

Every item follows the same path: (1) revalidate the report against current
source and runtime evidence; (2) record scope, benefit, complexity, owner and
acceptance criteria in an issue/decision note; (3) split implementation into a
small PR; (4) add focused regression and integration tests; (5) request review
and wait for `Rust CI / check`, `Frontend CI / check` and `Automation Policy /
gate` plus `Dependency Audit / check`; (6) squash-merge only after all required
checks pass (upstream synchronization uses a merge commit and verified ancestry
as the documented exception); (7) update the
issue labels, Project status and this queue with the merge commit and residual
work. Mixed review issues are never closed merely because one child PR lands.

Labels remain the auditable lifecycle source: `status:triage` means validated
but not scheduled, `status:ready` means acceptance criteria are approved,
`status:in-progress` means a PR is under active implementation, `status:blocked`
means an explicit dependency. Fully accepted issues are closed and use Project
Done, with stale open-lifecycle labels removed. Project cards must mirror the
same decision; the repository currently has no `status:done` label.

## Ordered delivery queue

| Order | Work package | Issues | Benefit / complexity | Exit criteria | State |
| --- | --- | --- | --- | --- | --- |
| 0 | Protect video-task secrets at rest | #211 | security and privacy / M-L | PR #293 covers debug/file redaction; remaining acceptance is encrypted-or-redacted headers/prompts/provider credentials across every persistence and error path, permission tests, and a clean-log/registry audit | Merged slice; residual acceptance |
| 1 | Durable privileged-mutation audit | #255 | incident accountability / L | each mutation writes a queryable `audit_logs` row before success is returned; timeout/failure semantics, authorization coverage, and integration tests are documented | In progress (#294/#329 merged; residual acceptance) |
| 2 | DLQ operator lifecycle | #223 | recoverability and billing correctness / M | PR #292 covers bounded retention, authenticated listing and idempotent redrive; remaining acceptance is duplicate/poison-message verification, marker TTL policy, capacity evidence, and a real Redis replay drill | Merged slices (#333); residual acceptance |
| 3 | CI and supply-chain gates | #216, #220 | catches regressions and CVEs / M | live DB tests are intentionally gated, VSCodex is a real required check, build fan-out is measured, and Cargo/npm advisory policy runs in CI | In progress (#296/#319; residual coverage pending) |
| 4 | Public API compatibility matrix | #247, #254 | prevents client retries and integration breakage / S-M | PR #290 defines the baseline OpenAI error/endpoint contract; remaining acceptance is OpenAI/Claude status-code, error-code, envelope and retry-header fixtures plus explicit balance and notification transitions | In progress (#290/#334/#335 merged; #374 refund notification awaits checks) |
| 5 | Operations reference and recovery runbook | #217, #218, #224, #225 | reproducible deployment and observability / M | metrics/alerts, environment table, multi-node topology, Redis failure semantics and restore drill are executable from published docs | In progress (#370 merged; #369/#371 and residual production acceptance remain) |
| 6 | Billing integrity follow-up | #206, #253, #300 | protects revenue and abuse boundary / M-L | enrichment failure, cancellation, signup credit, quota and image authorization policies have explicit tests and owner sign-off | In progress (#362 funds data layer merged; #379 implements zero-default gift; #368 records lifecycle boundary; #300/#206 integration remains) |
| 7 | Scheduler and protocol roadmap slices | #179, #205 | correctness and upgrade safety / M-L | each slice has a bounded ADR, dependency/rollback plan and acceptance test | In progress (#365 contract and #363 identity guard merged; #268 and #276 completed; release-key integration and scheduler residuals remain) |
| 8 | Developer and architecture debt | #221, #222, #226, #235, #241 | lowers long-term change cost / S-L | ADR index and one measured extraction slice are merged; #229 formula consistency and module navigation are accepted separately | In progress (#304/#305/#351 merged; residual architecture work) |
| 9 | Fork installation URL | #256 | avoids wrong-origin installs / S | runtime installer points to fork only when the fork publishes the artifact; otherwise documented as intentionally upstream | Planned |

## Per-issue disposition

The following is the complete open-issue intake. “Keep” means the issue remains
open; “split” means the parent stays open while child issues/PRs carry delivery.

| Issue | Priority | Decision | Next action |
| --- | --- | --- | --- |
| #256 | P2 | Split, active | PR #331 merged the no-fork-release policy and first-release migration gate; retain parent for first fork release evidence |
| #255 | P1 | Split, active | PR #376 review fixes and hosted Compose merging passed; auto-merge enabled, branch update/current-head checks pending. Retain mutation coverage/reconciliation and real restore acceptance. |
| #254 | P1 | Split | PR #335 merged the chat/images and Claude compatibility fixtures; retain parent for remaining endpoint coverage |
| #253 | P1 | Split, active | PR #379 implements the accepted zero-default gift policy, passes local HTTP/frontend regressions and independent review, and has protected auto-merge enabled. Keep the parent In progress for funding, delivered debt, promotion eligibility, referrals and cost accounting. |
| #247 | P1 | Split, active | PR #334 merged idempotent refund terminal notifications; PR #374 adds user refund-completion notification and awaits protected checks; retain parent for balance and transition coverage |
| #241 | P2 | Split | #305 merged removal of the silent `with_redis_url` no-op builder; parent remains open for broader architecture consistency |
| #235 | P2 | Planned | create ADR index and ownership/rollback records |
| #226 | P2 | Planned | measure dependency tree and pair allowlist work with #229 |
| #225 | P1 | Split, active | #367 completes the gateway environment reference; #378 updates the tunnel audit baseline. Retain remaining operational and ADR consistency acceptance. |
| #224 | P1 | In progress | #302 and PR #370 (`c860eec66`) provide multi-node preflight/baseline; retain parent for capacity and Redis failure evidence |
| #223 | P1 | Split, active | PR #333 merged Redis redrive idempotency/retention coverage; PR #369 adds the PostgreSQL backup/restore drill and awaits protected checks |
| #222 | P2 | Planned | measure one provider/repository extension slice before generic rewrite |
| #221 | P2 | Planned | produce call/dependency graph and extract one tested boundary |
| #220 | P1 | Split, active | installer checksum/signature and Cargo/npm advisory gates are merged; PR #373 records residual supply-chain controls and awaits protected checks, while container permissions and hosted updater evidence remain |
| #218 | P2 | Split | PR #385 was closed as redundant: #318/#312 already delivered `docker-compose.redis-durable.yml` and the hosted crash/redrive drill. Remaining configuration work should extend existing assets. |
| #217 | P1 | In progress | Readiness/RED slices are merged. Retain #307's real producer fault injection, Prometheus scrape/rule evaluation and receiver firing/resolved evidence. |
| #307 | P1 | In progress | #371 now passed real Prometheus scraping, rules and Alertmanager firing/resolved delivery in hosted CI; #384 superseded. Retain remaining production caller-path fault injection and deployment acceptance. |
| #216 | P1 | Split, active | PR #382 adds the existing entitlement-concurrency test to the required live PostgreSQL harness. Retain broader ignored-test coverage and the parent CI acceptance. |
| #215 | P2 | Planned | measure synchronous logging/SSE filtering/lock contention before changes |
| #214 | P1 | Split, active | PR #366 merged tunnel response-relay permit lifetime as `8fb31ea6`; PR #377 adds peer-error timing coverage. Retain remaining sync/stream admission and Redis fault-contract acceptance. |
| #213 | P2 | Planned | split giant handler and standardize error payload boundaries |
| #212 | P2 | Planned | consolidate cross-cutting capacity and dependency tests |
| #211 | P1 | Split, active | PR #372 adds admin video-field redaction; complete its current-head Gateway checks and retain persistence, lease and secret-path acceptance. |
| #210 | P2 | Planned | verify cryptographic/OAuth contracts with current dependency evidence |
| #208 | P2 | Planned | The NUMERIC adapter fix, historical audit #325 and NaN/infinity fixture #383 are merged. The compatibility correction passed on PostgreSQL 15.19 and 17.11; broader data-layer residuals remain. |
| #207 | P2 | Planned | bound internal errors and Windsurf buffering; add graceful shutdown slice |
| #206 | P1 | Split | PR #362's funding holds and checked-unit debit boundary correction merged after real PostgreSQL/memory tests and required CI. #300 carries Gateway integration; recharge/retention and reconciliation remain explicit acceptance. |
| #300 | P1 | Keep | PR #362 is merged. Independent lifecycle review identified admission/dispatch/terminal, per-attempt funding, reliable actual-cost facts, cancellation and recovery work; facade methods remain unintegrated. |
| #205 | P1 | Split, active | PR #366 merged response-relay lifetime. #375 was closed as a scope mismatch; #365's handshake contract does not implement release rotation. #387 implements the actual release trust set and build inputs with overlap/retirement fixtures, offline recovery and an independently audited helper; protected CI and platform/deployment acceptance remain. |
| #316 | P2 | Split | #380 records unknown historical impact; #383 merged after PostgreSQL 15.19/17.11 migration/audit tests and required CI. Toolkit completion cannot prove that no production repair is needed. |
| #303 | P1 | Split, active | reviewed RSA Marvin exception is time-bounded and fail-closed; retain parent for dependency release/removal evidence |
| #179 | P2 | Deferred | retain as roadmap; move actionable slices into child issues |
| #158 | P2 | Deferred | upstream provider-scoped allowlist evaluation only |
| #157 | P2 | Deferred | refresh upstream billing/quota registry; no blind cherry-pick |
| #53 | P1 | Blocked | scheduler roadmap dependency; do not implement until contract is approved |
| #52 | P2 | Blocked | scheduler roadmap dependency; define distributed probe lease contract |
| #51 | P1 | Blocked | scheduler roadmap dependency; define admission revalidation contract |
| #49 | P1 | Blocked | scheduler roadmap dependency; pair with #268 failure-origin decision |
| #48 | P2 | Blocked | protocol capability contract remains deferred |
| #47 | P2 | Blocked | multi-instance acceptance is covered by #224 plan |
| #46 | P1 | Blocked | attempt/client lifecycle depends on scheduler contract |
| #45 | P2 | Blocked | structured scheduling trace follows approved lifecycle model |
| #44 | P2 | Blocked | emergency-chain behavior remains deferred and expiring |
| #1 | P2 | Blocked | umbrella only; do not duplicate child implementation |


## Historical checkpoints

The records below describe earlier observations only. They do not override the
current snapshot, per-issue queue, live GitHub checks or recorded review findings.

The records below retain the observations and decisions made at their stated
times. They do not override the latest checkpoint and current intake above.

### 2026-09-12 live checkpoint

The fork main branch currently includes #290, #293, #294, #295, #297, #298,
#299, #301, #302, #304 and #310. Their merge commits and required-check evidence are
available from the linked PRs; each is a slice of its parent Issue. #305 is
still open with squash auto-merge enabled and is being revalidated after the
#304 base moved.

The observability parent #217 is OPEN again because #301 was a scope review
that was accidentally treated as a closing reference. It now has three
bounded children: #307 (metrics and alerts), #308 (readiness/health), and #306
(RED dimensions and telemetry). #224 remains open for capacity and Redis
recovery evidence after its #302 preflight slice.

Issue #218 now has two ready children: #311 for remote PostgreSQL TLS defaults
and #312 for the production Redis durability profile. These children capture
the remaining P1 decisions without changing local development defaults.

Issue #205 now has #314 (service identity) and #315 (signed provenance) ready
children. Issue #208 has #316 ready for a read-only historical data audit after
the new-write adapter fix.

The dependency gate #296 is held with auto-merge disabled. Its current review
found that `Dependency Audit / check` is not yet in ruleset 21984327, and the
RSA active-graph guard can miss a match when `rg -q` closes a pipe under
`pipefail`. The exact evidence and remediation are recorded in the PR; no
advisory is silently ignored.

## Completed slices and residual links

PRs #282–#310 include merged slices into the fork. They cover contributor entry
points, watchdog health feedback (#92), finite billing formula values,
governance records, TaskSupervisor drop cleanup, sparse OpenAI video polling,
bounded DLQ retention, CI/release-gate decisions, the baseline OpenAI API error
contract, and video-task secret protection. These merges are evidence for the
corresponding child slices only; #211, #216, #220, #223, #247, #254 and #255
remain open until their residual acceptance criteria above are met.

PR #290 merged as `f29a393a44d36ccc16fc003b988e5cc5d8b7dfdd` after all required
checks passed. PR #293 merged as
`fe25a3545b8be0944f2f3e12b7206994407fc5cd` after all required checks passed.
PR #294 is merged as `d08fe6ebe11951e9390037893efc8ef6b77ea4a7`; the durable
audit parent #255 remains open for residual failure and integration semantics.

## 2026-09-13 live checkpoint (after PRs #323, #325 and #326)

The authoritative remote inventory is **50 open issues**: 26 P1 and 24 P2.
Their lifecycle labels are 1 `status:ready`, 28 `status:triage`, 11
`status:blocked`, and 10 `status:in-progress`. This count is deliberately
separate from the historical snapshots above.

Completed fork-only slices since the previous checkpoint:

- PR #323 merged billing failure counters and the Prometheus alert contract
  (`883c3c78…`); #307 remains open for external parse/alert rehearsal and its
  dependency on the #306 producer.
- PR #329 merged durable audit persistence across client disconnects
  (`42d02830…`); #255 was intentionally reopened for broader mutation and
  reconciliation acceptance.
- PR #325 merged the read-only historical NUMERIC audit (`c58ccc85…`); #316
  remains open pending an explicitly reviewed backfill decision.
- PR #326 merged the previous delivery TODO/workflow refresh (`042504cf…`).
- PR #335 merged the public API compatibility fixture slice (`ace87b52…`);
  #254 remains open for any uncovered endpoints.
- PR #332 merged the dev-profile build-performance slice (`99225356…`);
  #216 remains open for live-DB and VSCodex gate coverage.
- Earlier PRs #317–#322 remain merged; their parent Issues retain only the
  residual acceptance documented above.

Current delivery order:

1. **#306 / #307** — finish the provider production-source, cancellation,
   retry and streaming lifecycle review; PR #324 is merged and the parent
   stays open until the RED producer contract is consistent with #307.
2. **#256 / #209** — verify first-release install evidence and credential
   error redaction residuals; PRs #331 and #337 are merged and their parents
   remain open only for the acceptance documented above.
3. **#255, #211, #303 and #308 residual acceptance** — expand mutation-audit
   coverage, verify remaining sensitive-field paths, govern the RSA exception,
   and run readiness isolation drills.
4. **#229 and #256** — complete the remaining P2 ready/community adoption work
   after CI; keep runtime-install policy tied to the first fork release.
5. **Triage/blocked backlog** — review the remaining 27 triage and 11 blocked
   issues, split only when acceptance criteria and ownership are concrete.

PRs are squash-only and fork-only. A parent issue is closed only when its full
acceptance criteria have evidence; a merged child slice changes the parent to
`status:triage` when residual scope remains. Every next slice must add focused
tests, pass the four required contexts (Rust, Frontend, Automation Policy and
Dependency Audit), and record the merge SHA and residual risks here and on its
Issue.

## 2026-09-13 child slice: isolated PostgreSQL live-DB harness (#339)

Issue #339 is accepted as a P1/Ready child of #216. The fork-only slice adds
`tools/ci/run_postgres_live_tests.sh` and a required `Data DB Live (selected
ignored tests)` job. Each job receives a disposable PostgreSQL service; the
canonical URL is `AETHER_TEST_DATABASE_URL`, while the legacy migration-test
name is mapped to the same URL and rejected when it differs. The harness first
runs the migration smoke test, then serially and explicitly runs:

- `live_first_byte_reads_provider_contribution_after_waiting_for_canonical_lock`
- `live_usage_policy_window_aggregates_preserve_exact_admission_and_idempotency`
- `live_stale_terminal_event_is_a_full_transaction_noop`

Every invocation uses `--exact --include-ignored --nocapture --test-threads=1`,
so the log proves discovery and execution while avoiding cross-test races. A
non-zero exit fails the aggregate `Rust CI / check`; the service database is
destroyed with the job, providing cleanup and isolation. The remaining ignored
tests stay explicitly ignored because they require different fixtures or
additional review. During local validation, the candidate
`live_pending_batch_and_terminal_upserts_count_each_provider_request_once`
was intentionally excluded: its 32 terminal writes plus one pending batch
exceed the production preparation admission limit of 32 and fail with the
explicit `usage preparation capacity exhausted` error. It needs a separate
capacity/fixture decision rather than a CI waiver. Local evidence for this
slice: `bash -n
tools/ci/run_postgres_live_tests.sh`, `cargo fmt --all -- --check`, and
`git diff --check`; Docker was unavailable in the development environment, so
live execution must be confirmed by the CI service job.

## 2026-09-12 live revalidation (after main `acb022247`)

The current fork inventory was re-read from GitHub rather than inferred from
this document: **50 open issues** (26 P1, 24 P2), with 10 `status:in-progress`,
1 `status:ready`, 28 `status:triage`, and 11 `status:blocked`. The Project #1
cards retain the same lifecycle decisions and priority/area/risk fields.

Three squash PRs remain open and have native auto-merge enabled: #324 (RED
telemetry), #331 (fork tunnel install policy), and #337 (Antigravity OAuth
error redaction). Their current base is `main@3d9a1116`; required checks are
queued or running, and no new failure is treated as accepted until the
corresponding head SHA completes CI. #328, #333 and #334 merged after the
previous checkpoint; #327 and #305 were also already merged.

The immediate exit condition is therefore CI completion followed by automatic
squash merges. After each merge, re-read the PR head/base and parent Issue
acceptance, record the merge SHA, and keep the parent open when residual evidence
is still missing. No user action is required while these checks run.

The merged API contract is a baseline only: balance notification behavior in
#247 and the chat/images compatibility fixtures for #254 are intentionally not
claimed by #290. Likewise, #293 does not close #211 until the residual
registry, lease, persistence and end-to-end log checks are evidenced.

## 2026-09-12 state-change record

The maintainer comments carrying the above transitions are recorded on
[#211](https://github.com/JinPengGeng/aeris-token/issues/211#issuecomment-5640656268),
[#216](https://github.com/JinPengGeng/aeris-token/issues/216#issuecomment-5640656263),
[#220](https://github.com/JinPengGeng/aeris-token/issues/220#issuecomment-5640656258),
[#223](https://github.com/JinPengGeng/aeris-token/issues/223#issuecomment-5640656368),
[#229](https://github.com/JinPengGeng/aeris-token/issues/229#issuecomment-5640656269),
[#247](https://github.com/JinPengGeng/aeris-token/issues/247#issuecomment-5640656271),
[#254](https://github.com/JinPengGeng/aeris-token/issues/254#issuecomment-5640656257),
[#255](https://github.com/JinPengGeng/aeris-token/issues/255#issuecomment-5640656296),
and [#256](https://github.com/JinPengGeng/aeris-token/issues/256#issuecomment-5640656260).

The current PR evidence is [#290](https://github.com/JinPengGeng/aeris-token/pull/290),
[#293](https://github.com/JinPengGeng/aeris-token/pull/293), and
[#294](https://github.com/JinPengGeng/aeris-token/pull/294). On 2026-09-12,
#294 received the import/format fix `cad0b0b33fc175ce03cf0cf9fd10a4fd4f043536`;
GitHub reports the PR as open, auto-merge enabled, and merge state `BLOCKED`
until the in-progress required checks complete. This record intentionally does
not claim a merge or close #255.

## 2026-09-13 authoritative live checkpoint (main `2734843c`)

This section supersedes earlier live-checkpoint paragraphs when they conflict;
older paragraphs remain as an audit trail. The fork inventory was re-read from
GitHub on 2026-09-13: **50 open issues** (26 P1, 24 P2), with 27
`status:triage`, 11 `status:in-progress`, 11 `status:blocked`, and 1
`status:ready`.

Three open fork PRs remain, all targeting `main`, using squash auto-merge and
currently blocked only by queued/running required checks: [#324](https://github.com/JinPengGeng/aeris-token/pull/324)
(RED telemetry, head `4219072c`), [#331](https://github.com/JinPengGeng/aeris-token/pull/331)
(fork tunnel install policy, head `e717409c`), and [#337](https://github.com/JinPengGeng/aeris-token/pull/337)
(Antigravity OAuth error redaction, head `3a3d012b`). No PR is claimed merged
until GitHub reports a merge commit and all required contexts pass.

The following slices are confirmed merged into the fork main branch and are no
longer pending CI: #305 (`7593bfdc`, no-op Redis builder removal), #327
(`78894d41`, bounded readiness probes), #328 (`8c7cc23f`, terminal registry
retention), #333 (`c2c417a8`, Redis DLQ recovery drill), and #334 (`2734843c`,
refund terminal notifications). Their parent Issues remain open where the
documented residual acceptance is incomplete. The next queue is therefore
#324/#331/#337 CI completion, followed by residual acceptance for #211, #217,
#223, #247, #254, #255, #303 and #308; triage/blocked issues remain deferred
until evidence and ownership are concrete.

## 2026-09-13 authoritative live checkpoint (main `f663b48a90d65c148b827506cc8a4cb17c85ebba`)

This section supersedes earlier live-checkpoint paragraphs when they conflict;
those paragraphs remain as an audit trail. The inventory was re-read from the
fork GitHub API on 2026-09-13: **49 open issues** (26 P1, 23 P2), with 28
`status:triage`, 9 `status:in-progress`, 1 `status:ready`, and 11
`status:blocked`. Project #1 retains the corresponding lifecycle cards. A
secondary GraphQL rate limit prevented a fresh aggregate of all custom fields;
individual Project field values therefore remain the authoritative record.

There are **no open pull requests** in `JinPengGeng/aeris-token`. Recent
fork-only squash merges and their merge SHAs are:

- #346 `f663b48a90d65c148b827506cc8a4cb17c85ebba` — inject release key id for
  manifest verification (2026-09-12 18:46 UTC).
- #344 `a63a4a2f6d778e2e5f8d8e76a947ded62e3997be` — classify control dependency
  failures (2026-09-12 17:48 UTC).
- #342 `e7c17b6381aa40c1d2dda031e5ae145e4c84c073` — isolated PostgreSQL live
  tests (2026-09-12 18:29 UTC).
- #341 `bc9e20f41061fd425da85c997b964637318fc2de` — scope the RSA advisory
  gate (2026-09-12 18:13 UTC).
- #338 `931cc6be6f25c77a873eb8da865d2ba1cf794267`, #337
  `c00b79d147a309c802a5cc2eb45e78245119ba9c`, #331
  `276dde25d8cbbb21d571df903e5495eebf8a1a3b`, and #324
  `a5bbfd4bfbe14ad3d92a40891fb889610f9a09fa` — delivery checkpoint,
  OAuth redaction, fork install policy, and RED telemetry respectively.

Issue #343 (`[247-A] 定义余额与配额拒绝的 OpenAI/Claude 兼容契约`) is OPEN,
P1 and `status:ready` (Project `Ready`). Its maintainer decision records
OpenAI wallet exhaustion as 429/`insufficient_quota` without `Retry-After`,
Claude billing exhaustion as 402/`billing_error`, provider rate limits as
429 with `Retry-After` only when a wait is known, and tenant permission as
403/`permission_error`. The next slice is a table-driven contract PR covering
the stated envelopes and fixtures; Gemini semantics remain out of scope.

Issue #205 is OPEN/P1/`status:triage` with the critical upgrade, redirect,
replay, private-target and service-identity slices implemented. Its remaining
acceptance is limited to signing-key rotation overlap and recovery evidence,
nightly tunnel artifacts, service UID non-root hardening, and the low-risk
scheduler/admission design questions. Child #345 (release workflow key-id
input) is closed after PR #346; the parent must stay open until these residual
items have evidence. No duplicate security PR is warranted.

Current delivery order is therefore: (1) implement and verify #343; (2) close
residual acceptance for #205, #211, #217, #223, #247, #254, #255, #303 and
#308; (3) finish the remaining ready/community adoption slice #229/#256; and
(4) triage the remaining 28 triage and 11 blocked issues. Every new slice
remains fork-only, squash-only, test-backed, reviewed under the required CI
contexts, and recorded here plus on its parent Issue.

## 2026-09-13 parallel implementation checkpoint (2026-09-12 19:36 UTC)

This timestamped checkpoint supersedes earlier statements about the live queue.
PR #347 merged as `095538a407dcbda8228dbbe747bc8491a5939568`. The next
intake contains 50 open issues (26 P1, 24 P2): 12 labelled in progress,
27 triage, 11 blocked, and none ready. In-progress parent issues include
partially delivered work; these counts do not mean twelve simultaneous code
implementations.

Four independent fork PRs have completed implementation and independent review:

| PR / issue | Accepted change and evidence | Remaining merge/acceptance work |
| --- | --- | --- |
| #349 / #348, parent #223 | Strict Redis harness; 123 local tests passed with 55 real Redis starts and one explicitly retained timing benchmark; missing-binary/startup/connection failures verified; existing Compose/AOF drill preserved with an artifact | Current-head required CI and its Redis artifact; close #348 only after verified merge, keep #223 open |
| #350 / #307, parent #217 | Fixed duplicate Prometheus HELP/TYPE metadata that the official parser rejected; 47 runtime tests, real exporter parsing and five alert lifecycle scenarios passed; supplemental Prometheus CI artifact verified | Four required checks; #307 still needs production event-path and deployment acceptance |
| #351 / #229 | Shared formula allowlist consistency, safe unsupported fallback, numeric/arity tests, module navigation and model mapping alias explanation; 12 billing and 2 admin tests passed | Current-head required checks then close #229; #226 remains independent |
| #352 / #343, parents #247/#254 | Typed exhausted-credit contract across local denial, finalization and streams; 924 format and 277 gateway tests passed, with 128 finalization combinations | Current-head required checks then close #343; parent notification/refund and other endpoint work remain |

All four PRs use squash auto-merge after review; pending or failed checks are
not acceptance. Base updates are reviewed and rechecked. PR #349's first
automation failure exposed an omitted action in the Rust CI pin inventory;
the existing repository-approved artifact action SHA was added, and all 203
automation tests then passed. No gate was weakened. Superseded CI runs were
cancelled to release capacity, retaining their logs.

### Revalidated residual work

- #223: contrary to an earlier audit comment, the Compose/AOF crash drill
  already runs in `Shell security fixtures`. #348 fixes optional runtime-test
  skips and adds artifacts; it does not duplicate the drill or prove Postgres
  backup restore, marker lifetime or end-to-end billing recovery.
- #307: the existing rules parsed, but real billing exposition did not. The
  parser failure and successful fix are documented in PR #350. Alert examples
  now retain routing labels and cover usage/video settlement; provider labels
  are bounded types, not arbitrary configured provider identifiers.
- #218: missing encryption keys are still permitted by
  `validate_gateway_data_encryption_key(None)`. Supplied-key validation is
  not proof of a mandatory production-key policy. This residual remains open;
  JWT, TLS and Redis-profile deliveries remain independently valid.
- #316: the tested historical NUMERIC audit tool does not prove that production
  history needs no backfill. Actual redacted aggregate evidence and any
  remediation decision remain outstanding; no production writes are authorized
  by its read-only assessment scope.

### Project and next work

All 50 open issues were reconciled to Project #1. Every issue has Status,
Priority, Area, Risk, Size and Decision. Twenty-two missing Area values were
filled from their single existing area label; #316 received Medium risk,
size M and Planned with a recorded read-only financial-history rationale.
Issue cards mirror lifecycle labels; the four reviewed PR cards use In review.

Next: verify/merge these four PRs and synchronize closed child issues and the
board; then continue #223 marker/restore acceptance, #308 dependency withdrawal
and recovery, #307 event-path coverage, #205 signing/identity residuals, and the
remaining triage/deferred decisions in the ordered queue. Parent closure still
requires every remaining acceptance item or an explicit documented disposition.

## 2026-09-13 authoritative parallel checkpoint (remote revalidation)

The fork inventory was re-read after the parallel acceptance merges: **47 open
issues** (24 P1, 23 P2), with 12 `status:in-progress`, 24 `status:triage`, and
11 `status:blocked`. This supersedes earlier counts while retaining them as
history. PR #350 merged as `f11d9ff3bc27aee42e54982cbe24343c1627ab5e`, #351
merged as `5c620e7681cba50451847c3d163076614ddf8e83`, and #352 merged as
`6c2ba493131c6fcf59dcda32779b1fce219912ce`; #229 and #343 are closed after
their accepted slices. #349 merged as `b044b1292694c1850a6efb7eccc9da63451be7bd`
and closed #348. Open delivery PRs are #354
(environment reference), #355 (candidate lifecycle), #356 (readiness draft),
and #357 (Dependabot coverage), plus this #353 documentation PR. Their merge status and CI remain authoritative
until each reaches a terminal state.

The #308 readiness/lifecycle residual is broader than a deployment rehearsal:
it must cover single-flight and cache semantics, dependency withdrawal,
gateway lifecycle transitions, an overall probe deadline, and a real
failure/recovery drill. The #357 dependency slice must count only the five
real npm projects and three Docker build directories; an orphan root lockfile
without a project manifest is not an additional project. Issue #220 therefore
remains open for container permissions, remaining image supply-chain work,
the tracked dependency exception and hosted updater evidence. Review correction
`3761872aa` removes the invalid root npm target; YAML/inventory checks now use
tracked manifest/lockfile pairs. Ready PRs #354, #355 and #357 may proceed under
the normal four required checks;
parent issues remain open until all acceptance evidence is recorded.

PR #349's reviewed head `c65e035e04b9e06f0c43f39b6a6c51c2da2aa8c9` passed
all four required checks in [Rust CI run 34716866543](https://github.com/JinPengGeng/aeris-token/actions/runs/34716866543).
The downloaded `redis-runtime-tests` and `redis-durable-crash-drill` artifacts
confirm 123 passing runtime tests (one existing ignored benchmark), strict
missing-binary/startup/connection failures, an explicit optional local skip,
AOF persistence after kill/restart and idempotent redrive to one final stream
entry with an empty DLQ. Issue #348 and its PR card are Done; #223 remains open
for marker retention/capacity, PostgreSQL restore and end-to-end billing recovery.

## 2026-09-13 live delivery checkpoint (fork-only)

The fork was re-read with `gh` on 2026-09-13. The repository has **45 open
issues**: 22 P1 and 23 P2, with 15 `status:in-progress`, 19 `status:triage`,
and 11 `status:blocked`. This supersedes earlier 47/49/50-item counts while
keeping them above as history. The delivery queue tracks **18 active open
implementation/documentation PRs**: #362, #365-#369, #371-#380, and #382-#383.
PR #364 is the prior checkpoint, #370 is merged, and #381 is this refresh PR;
those records are excluded from the active queue count.

### Focused issue and PR mapping

| Issue | Current fork evidence | Delivery state |
| --- | --- | --- |
| #253 | Open/P1/triage. Audit PR #379 records configurable $10 gift and Turnstile defaults, reservation/row-lock evidence, and the `insufficient_quota` accounting gap. | #379 is draft and blocked by current-head required checks; keep policy and production evidence open. |
| #255 | Open/P1/in-progress. Unauthenticated audit finding is disproved; durable mutation audit, retry/reconciliation, broader events and Docker safety remain. | #376 is non-draft but blocked while checks run; do not close parent on partial persistence slices. |
| #225 | Open/P1/in-progress. Tunnel env/reference and ADR consistency work is split across delivered and pending slices. | #378 is non-draft and blocked by required checks; review before acceptance. |
| #214 | Open/P1/in-progress. Listener/accept recovery and probe/lifecycle contract remains under implementation. | #377 is draft/blocked; its test contract is not full recovery acceptance. |
| #205 | Open/P1/triage. Key rotation overlap, recovery evidence, nightly artifacts and non-root identity hardening remain. | #375 is draft and behind `main`; rebase and rerun required checks. |
| #300 | Open/P1/in-progress. Quote slice #361 merged; funds data slice #362 is draft. Gateway lifecycle façade remains unintegrated. | #368 records the lifecycle boundary; #362 and gateway admission/dispatch/terminal integration remain open. |

All five focused PRs (#375-#379) remain open. Required checks are current-head
gates; a green subset, draft PR or blocked merge state is not acceptance. No PR
is auto-merged by this refresh.

### #300 façade boundary (revalidated)

`origin/main` still has no verified gateway calls to `reserve_request_funds`,
`mark_request_funds_dispatched`, or `finalize_request_funds`. The façade must
connect admission, retry, sync/stream terminal finalization, unknown-dispatch
hold retention, and recharge recovery/crash evidence. Keep #300 and #206 open
until those paths are integrated and exercised against real PostgreSQL and
failure/recovery scenarios.

### Earlier pre-consolidation records

## 2026-09-13 historical refresh (after PR #363 and new delivery slices)

The fork inventory was re-read from GitHub on 2026-09-13: **45 open issues**
(22 P1, 23 P2), with 15 labelled `status:in-progress`, 19 `status:triage`,
and 11 `status:blocked`. Counts include parent and child issues and are not a
count of independent implementation packages.

Labels were synchronized after review: #217 and #224 moved from `status:triage`
to `status:in-progress`; closed #345 has no stale lifecycle label and its
Project card is Done.

PR #361 merged as `1831852e9abecfdc267ddbf783d2203cd2d39e14`; #356 merged as
`86bc4d8b88311ac3ddd400d9e72a2d68bdc4c9f6` and closed #308 (Project **Done**).
PR #358 merged as `1bcff1a5271aebefa96253417f4707574a7a2994` from reviewed head
`7de0188d44118f7e1c1fc2930bc6aacadfa01070`. PR #363 (Refs #205) merged as
`e9ca10ab2d28a78cb4f265cace17260679b048bc`; #205 remains open for key rotation,
recovery, nightly artifacts and remaining upgrade evidence.
PR #370 merged as `c860eec66d9edfb39f7536111bd7e8f6d1b2ca66`; its three-node
deployment slice is accepted and its Project card is Done.

There are **11 open PRs**: Draft #362 (head
`2d5abae6143eeb931ebf96906aa5ba0681f22e17`), plus nine non-draft PRs with
squash auto-merge enabled: #364 (`897b1ee9affc5c9ac45b5ccb98b03e00c99352a2`),
#365 (`b2b99ed7b6273220b8666204b322ed0e2e2e8ed2`), #366
(`472484b325e5d85ba7841d71df9f2307bf8a3c29`), #367
(`0b452aebce4f0d4f49b115b1e534439016a057ad`), #368
(`03a12764a2f488c7c45fcf28bfdf5d492a7a7d76`), #369
(`cc0fb5ddabe95924206022f687b840d97e95a362`) and #371
(`f9921213c07ce814643c58814f5c4e7413b04551`), plus #373
(`c185a15570af455b849844c1245acc659496b20d`) and #374
(`4474ae0b77070328483c1335c200f6c162912f5d`). #372 is also open (head
`5db5675c63211136d6159ffbd385d52f037a72cf`) without auto-merge; all eleven
currently report `BLOCKED` while protected checks run. Auto-merge remains
enabled on #364–#369, #371, #373 and #374.

The bounded slices retain their parents: #366 covers tunnel response-relay
admission (#205/#214), #367 the gateway environment reference (#225), #368
the #300/#206 lifecycle boundary, #369 PostgreSQL backup/restore (#223), and
#371 isolated metrics scrape
and alert delivery (#307/#217). PR #372 redacts sensitive admin video fields
(#211), but its targeted build is blocked by an unrelated existing refund
notification compile error. PR #373 records #220 supply-chain residuals and
PR #374 adds the #247 refund-notification slice; both await protected checks.
Draft #362 still needs independent review and required
CI before funds-hold integration with gateway admission/dispatch/terminal,
cancellation/partial output, retry and crash recovery. Parent issues remain
open until residual acceptance has evidence.

The next transition is current-head review and required checks for #362 and
#364–#369 and #371–#374; #372 needs the unrelated refund compile break resolved before it can
be accepted. All transitions remain fork-only and are recorded on each PR, parent
issue, Project card and this queue.

### Earlier checkpoint after PRs #356, #358 and #361

The fork inventory was re-read from GitHub on 2026-09-13: **45 open issues**
(22 P1, 23 P2), with 13 labelled `status:in-progress`, 21 `status:triage`,
and 11 `status:blocked`. Counts include parent and child issues and are not a
count of independent implementation packages.

PR #361 merged as `1831852e9abecfdc267ddbf783d2203cd2d39e14`. PR #356 merged as
`86bc4d8b88311ac3ddd400d9e72a2d68bdc4c9f6`; Issue #308 is closed and its
Project card is **Done**. PR #358 merged as
`1bcff1a5271aebefa96253417f4707574a7a2994` from reviewed head
`7de0188d44118f7e1c1fc2930bc6aacadfa01070`; its protected checks, including
the Gateway test, passed.

Draft PR #362 remains open at head `27682e86ff50a18a69ec78f22ecb8c216df0305d`;
its required CI and independent review are pending. #362 is the
funds-reservation slice for both #300 and #206, but it does not close either
parent. Remaining #300/#206 acceptance is gateway admission/dispatch/terminal
lifecycle integration, cancellation and partial-output handling, retry and
crash recovery, plus the documented recharge/retention follow-up.

PR #363 (Refs #205) remains open with squash auto-merge enabled at head
`7095bcca993aa541eca07fa4fe3f59c630aca745`; it is behind current main
(`1bcff1a5271aebefa96253417f4707574a7a2994`) and its required checks are still
running. Its root-service-identity guard is a bounded residual; signing-key
rotation/recovery, nightly artifacts and remaining upgrade evidence stay open
in #205. Keep all parent issues open until their residual acceptance has tests,
review and protected-check evidence.

The next transition is to finish current-head review and required checks for
#362 and #363, then integrate #362's funds hold with gateway lifecycle behavior.
All transitions remain fork-only and are recorded on the PRs, parent issues,
Project cards and this queue.

## 2026-09-13 historical status after PR #353

Current intake: 47 open issues (24 P1, 23 P2), with 13 in progress, 23 triage
and 11 blocked after #214 entered implementation. PR #353 merged as
`b2d2b0fca0bb96916880f8061e5d03d2c79fa501` and its Project card is Done.
PR #355's two remaining usage fixtures now preserve both planner observations;
all 14 local usage tests passed and the corrected branch awaits complete CI.
PR #356 passed main review, all four required checks and its hosted 12-phase
drill (run 34717153949); it is Ready with auto-merge enabled, pending checks
after main synchronization. #354 and #357 also retain protected auto-merge.

Draft PR #358 implements #214's automatic per-process target capacity and
corrects stale audit findings. Local target/parser/admission tests passed;
independent review and protected CI remain. The parent retains permit-lifetime,
Redis fault-contract and SQL live-timeout acceptance. This snapshot supersedes
the historical checkpoints below; live GitHub state governs later transitions.

### Earlier checkpoint after PR #349

Latest verified intake: 47 open issues (24 P1, 23 P2), with 12 in progress,
24 in triage and 11 blocked after #220 entered implementation and #348 closed.
PR #349 merged as `b044b1292694c1850a6efb7eccc9da63451be7bd`; its four required
checks passed and its real Redis artifacts were downloaded and verified.
PR #352 merged as
`6c2ba493131c6fcf59dcda32779b1fce219912ce`; #343 is closed and its issue/PR
cards are Done. #247 and #254 retain their separate residual acceptance.
#225's generated tunnel configuration reference is ready in PR #354, and
#268's candidate identity correction is ready in PR #355. Both are reviewed
with automatic merge enabled under the protected checks. #308's complete
readiness contract is in draft PR #356 for final review and CI. #357's reviewed
Dependabot coverage is also awaiting protected checks with automatic merge
enabled. Decisions are recorded in the individual PRs and issue documents.

PR #350's new CI exposed a fallback-recovery test race: persistence completed
before the lifecycle worker decremented its pending count. The test now waits
at most two seconds for lifecycle accounting, retaining all capacity and
persistence assertions. Independent review and six pinned Rust 1.95.0 runs
passed; #350 then passed CI and merged as
`f11d9ff3bc27aee42e54982cbe24343c1627ab5e`. #307 and #217 remain open for
production producer fault injection, scraping and actual alert delivery evidence.

### Earlier status after PR #351

Live GitHub verification: 49 open issues (26 P1, 23 P2), with 11 in progress,
27 in triage and 11 blocked. Counts include parent and child issues and must
not be interpreted as 49 independent implementation packages. Removed the
duplicate triage label from #348. PR #351 merged as
`5c620e7681cba50451847c3d163076614ddf8e83`, closing #229; its Project #1 card is
Done. Removed its stale in-progress label; the repository has no status:done
label, so the closed issue state and Project Done are the completion record.
Updated PRs #349, #350 and #352 against the new main using their expected head
SHAs. All three retain squash auto-merge and require fresh checks before merge.

This top checkpoint and the intake table below describe the latest verified
state. Timestamped checkpoints after the intake table are historical records.

## 2026-09-13 latest live checkpoint (fork-only)

Remote state was re-read with `gh`: **44 open issues** (21 P1, 23 P2) and
**15 open PRs**. Fourteen open PRs have squash auto-merge enabled; #388 is the
sole Draft PR. These counts supersede earlier intake counts while preserving
the historical records above.

PR #387 merged after reviewed head `9c9880bf8673da2eadc1100ee038526bc6df0c61`
passed all required checks; merge commit is
`c7563fd962ab2b08dc9136be6ff8f38702e1290d`. It delivers release signing-key
overlap/retirement fixtures and verifier dependency audit coverage. Remaining
#205 recovery/nightly-artifact acceptance stays open.

Draft PR #388 (head `c4f6647213c9d3566ba220bb0e632b2d3cfdec9b`) preserves usage
rows needed for `prepared`, `dispatched`, `reconciliation_pending`,
`insufficient_quota`, and outstanding recovery balances while allowing raw
bodies/headers to expire normally. PostgreSQL 17.11/Rust 1.95 validation passed
7 cleanup tests and 12 required live targets plus Clippy; independent review and
current-head checks remain pending. This retention prerequisite does not claim
merge or complete Gateway lifecycle integration (#300/#206/#223).

Issue #216 has a parallel selective Rust CI regression repair in progress. The
detector reports false normally, but execution jobs lose `needs`/`if`; the
proposed correction restores conservative filtering and a fail-closed gate. No
PR number is asserted until GitHub publishes one.
