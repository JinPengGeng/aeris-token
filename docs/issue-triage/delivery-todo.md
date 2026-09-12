# Issue delivery TODO and community workflow

Snapshot: 2026-09-13, fork `JinPengGeng/aeris-token`. This queue is based on
the current fork tree and GitHub issue state; it does not modify upstream
`fawney19/Aether`.

## Operating rules

Every item follows the same path: (1) revalidate the report against current
source and runtime evidence; (2) record scope, benefit, complexity, owner and
acceptance criteria in an issue/decision note; (3) split implementation into a
small PR; (4) add focused regression and integration tests; (5) request review
and wait for `Rust CI / check`, `Frontend CI / check` and `Automation Policy /
gate`; (6) squash-merge only after all required checks pass; (7) update the
issue labels, Project status and this queue with the merge commit and residual
work. Mixed review issues are never closed merely because one child PR lands.

Labels remain the auditable lifecycle source: `status:triage` means validated
but not scheduled, `status:ready` means acceptance criteria are approved,
`status:in-progress` means a PR is under active implementation, `status:blocked`
means an explicit external dependency, and `status:done` is reserved for a
fully accepted issue. Project cards must mirror the same decision.

## Ordered delivery queue

| Order | Work package | Issues | Benefit / complexity | Exit criteria | State |
| --- | --- | --- | --- | --- | --- |
| 0 | Protect video-task secrets at rest | #211 | security and privacy / M-L | PR #293 covers debug/file redaction; remaining acceptance is encrypted-or-redacted headers/prompts/provider credentials across every persistence and error path, permission tests, and a clean-log/registry audit | Merged slice; residual acceptance |
| 1 | Durable privileged-mutation audit | #255 | incident accountability / L | each mutation writes a queryable `audit_logs` row before success is returned; timeout/failure semantics, authorization coverage, and integration tests are documented | In progress (#294/#329 merged; residual acceptance) |
| 2 | DLQ operator lifecycle | #223 | recoverability and billing correctness / M | PR #292 covers bounded retention, authenticated listing and idempotent redrive; remaining acceptance is duplicate/poison-message verification, marker TTL policy, capacity evidence, and a real Redis replay drill | Merged slices (#333); residual acceptance |
| 3 | CI and supply-chain gates | #216, #220 | catches regressions and CVEs / M | live DB tests are intentionally gated, VSCodex is a real required check, build fan-out is measured, and Cargo/npm advisory policy runs in CI | In progress (#296/#319; residual coverage pending) |
| 4 | Public API compatibility matrix | #247, #254 | prevents client retries and integration breakage / S-M | PR #290 defines the baseline OpenAI error/endpoint contract; remaining acceptance is OpenAI/Claude status-code, error-code, envelope and retry-header fixtures plus explicit balance and notification transitions | In progress (#290/#334/#335 merged; residual fixtures pending) |
| 5 | Operations reference and recovery runbook | #217, #218, #224, #225 | reproducible deployment and observability / M | metrics/alerts, environment table, multi-node topology, Redis failure semantics and restore drill are executable from published docs | In progress (#217 split to #306–#308; #323/#327 merged) |
| 6 | Billing integrity follow-up | #206, #253 | protects revenue and abuse boundary / M-L | enrichment failure, cancellation, signup credit, quota and image authorization policies have explicit tests and owner sign-off | Planned |
| 7 | Scheduler and protocol roadmap slices | #268, #276, #179, #205 | correctness and upgrade safety / M-L | each slice has a bounded ADR, dependency/rollback plan and acceptance test; unresolved upstream sync conflict is handled separately | Deferred / blocked |
| 8 | Developer and architecture debt | #229, #221, #222, #226, #235, #241 | lowers long-term change cost / S-L | one billing allowlist, no-op builder removed or deprecated, ADR index and one measured extraction slice are merged | In progress (#304/#305 merged; residual architecture work) |
| 9 | Fork installation URL | #256 | avoids wrong-origin installs / S | runtime installer points to fork only when the fork publishes the artifact; otherwise documented as intentionally upstream | Planned |

## Per-issue disposition

The following is the complete open-issue intake. “Keep” means the issue remains
open; “split” means the parent stays open while child issues/PRs carry delivery.

| Issue | Priority | Decision | Next action |
| --- | --- | --- | --- |
| #276 | P1 | Keep blocked | resolve the real upstream sync conflict with a reviewed merge commit |
| #268 | P1 | Keep | decide terminal telemetry semantics with scheduler failure-origin work |
| #256 | P2 | Split, active | PR #331 records the no-fork-release policy and first-release migration gate; retain parent for release evidence |
| #255 | P1 | Split, active | PR #329 persists audit after client disconnect; retain parent for broader mutation coverage and retry/reconciliation evidence |
| #254 | P1 | Split | PR #335 merged the chat/images and Claude compatibility fixtures; retain parent for remaining endpoint coverage |
| #253 | P1 | Keep | approve signup-credit, overdraft and abuse-control policy before code |
| #247 | P1 | Split, active | PR #334 merged idempotent refund terminal notifications; retain parent for balance and notification transition coverage |
| #241 | P2 | Split | #305 merged removal of the silent `with_redis_url` no-op builder; parent remains open for broader architecture consistency |
| #235 | P2 | Planned | create ADR index and ownership/rollback records |
| #229 | P2 | Split | #304 merged the shared formula allowlist; add the remaining DX map and keep parent open |
| #226 | P2 | Planned | measure dependency tree and pair allowlist work with #229 |
| #225 | P1 | Split | generate tunnel env table and publish operations runbooks |
| #224 | P1 | Split | #302 merged multi-node preflight; publish capacity smoke test and Redis failure runbook |
| #223 | P1 | Split, active | PR #333 merged Redis redrive idempotency/retention coverage and a recovery drill; retain parent for restore/backup evidence |
| #222 | P2 | Planned | measure one provider/repository extension slice before generic rewrite |
| #221 | P2 | Planned | produce call/dependency graph and extract one tested boundary |
| #220 | P1 | Split | add Cargo/npm advisory scan and explicit policy fixture |
| #218 | P1 | Split | #297 merged JWT startup validation; decide non-loopback environment/TLS and Redis durability slices |
| #217 | P1 | Split | reopened after accidental auto-close; #307 metrics slice merged, #308 readiness and #306 RED remain under review |
| #308 | P1 | In progress | PR #327 merged bounded readiness probes; complete deployment drill and parent acceptance |
| #307 | P1 | In progress | PR #323 merged runtime-owned billing/fail-open counters; complete Prometheus parse/alert drill and link #306 producer |
| #306 | P1 | In progress | PR #324 adds bounded request RED producer and JSON/pretty evidence; complete provider source and lifecycle review |
| #216 | P1 | Split, active | PR #332 adds the dev profile build-performance gate; retain parent for live-DB and VSCodex required checks |
| #215 | P2 | Planned | measure synchronous logging/SSE filtering/lock contention before changes |
| #214 | P1 | Planned | add probe, graceful shutdown and accept-error acceptance tests |
| #213 | P2 | Planned | split giant handler and standardize error payload boundaries |
| #212 | P2 | Planned | consolidate cross-cutting capacity and dependency tests |
| #211 | P0/P1 | Split, active | PR #328 merged bounded terminal registry retention; verify lease failure semantics and remaining sensitive-field paths |
| #210 | P2 | Planned | verify cryptographic/OAuth contracts with current dependency evidence |
| #209 | P2 | Planned | remove credential Debug/Serialize exposure and URL key residue |
| #208 | P2 | Planned | reproduce NUMERIC/f64 paths and add adapter regression coverage |
| #207 | P2 | Planned | bound internal errors and Windsurf buffering; add graceful shutdown slice |
| #206 | P1 | Split | #300 tracks image authorization cost bypass; implement fail-closed unknown paid-image estimate, then bounded pricing |
| #300 | P1 | Keep | validate bounded image cost estimation and wallet credit checks before changing authorization behavior |
| #205 | P1 | Split | #299 merged opt-in/default-off and anti-downgrade; design signed provenance and non-root service slices |
| #316 | P2 | Split | PR #325 read-only historical NUMERIC inventory merged; decide whether reviewed backfill is needed |
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

## 2026-09-12 live checkpoint

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

1. **#324 / #306** — finish the provider production-source, cancellation,
   retry and streaming lifecycle review; auto-merge remains enabled and the
   parent stays open until the RED producer contract is consistent with #307.
2. **#331 / #337** — finish the fork install-policy and credential error
   redaction slices; both remain open with squash auto-merge enabled.
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

## 2026-09-13 authoritative live checkpoint (main `3d9a1116`)

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
