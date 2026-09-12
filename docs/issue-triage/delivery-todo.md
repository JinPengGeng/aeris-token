# Issue delivery TODO and community workflow

Snapshot: 2026-09-12, fork `JinPengGeng/aeris-token`. This queue is based on
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
| 1 | Durable privileged-mutation audit | #255 | incident accountability / L | each mutation writes a queryable `audit_logs` row before success is returned; timeout/failure semantics, authorization coverage, and integration tests are documented | In progress (#294 merged slice; residual acceptance) |
| 2 | DLQ operator lifecycle | #223 | recoverability and billing correctness / M | PR #292 covers bounded retention, authenticated listing and idempotent redrive; remaining acceptance is duplicate/poison-message verification, marker TTL policy, capacity evidence, and a real Redis replay drill | Merged slice; residual acceptance |
| 3 | CI and supply-chain gates | #216, #220 | catches regressions and CVEs / M | live DB tests are intentionally gated, VSCodex is a real required check, build fan-out is measured, and Cargo/npm advisory policy runs in CI | In progress (#296; advisory gate deployment and guard fixes pending) |
| 4 | Public API compatibility matrix | #247, #254 | prevents client retries and integration breakage / S-M | PR #290 defines the baseline OpenAI error/endpoint contract; remaining acceptance is OpenAI/Claude status-code, error-code, envelope and retry-header fixtures plus explicit balance and notification transitions | In progress (#290 merged; fixtures pending) |
| 5 | Operations reference and recovery runbook | #217, #218, #224, #225 | reproducible deployment and observability / M | metrics/alerts, environment table, multi-node topology, Redis failure semantics and restore drill are executable from published docs | In progress (#217 split to #306–#308; #224 preflight merged) |
| 6 | Billing integrity follow-up | #206, #253 | protects revenue and abuse boundary / M-L | enrichment failure, cancellation, signup credit, quota and image authorization policies have explicit tests and owner sign-off | Planned |
| 7 | Scheduler and protocol roadmap slices | #268, #276, #179, #205 | correctness and upgrade safety / M-L | each slice has a bounded ADR, dependency/rollback plan and acceptance test; unresolved upstream sync conflict is handled separately | Deferred / blocked |
| 8 | Developer and architecture debt | #229, #221, #222, #226, #235, #241 | lowers long-term change cost / S-L | one billing allowlist, no-op builder removed or deprecated, ADR index and one measured extraction slice are merged | In progress (#304 merged allowlist; #305 pending) |
| 9 | Fork installation URL | #256 | avoids wrong-origin installs / S | runtime installer points to fork only when the fork publishes the artifact; otherwise documented as intentionally upstream | Planned |

## Per-issue disposition

The following is the complete open-issue intake. “Keep” means the issue remains
open; “split” means the parent stays open while child issues/PRs carry delivery.

| Issue | Priority | Decision | Next action |
| --- | --- | --- | --- |
| #276 | P1 | Keep blocked | resolve the real upstream sync conflict with a reviewed merge commit |
| #268 | P1 | Keep | decide terminal telemetry semantics with scheduler failure-origin work |
| #256 | P2 | Split | migrate runtime install URL only if fork artifacts are published |
| #255 | P1 | Split | design durable audit writer and failure contract |
| #254 | P1 | In progress | #290 merged; add chat/images compatibility fixtures and validate the status/error/envelope/retry matrix |
| #253 | P1 | Keep | approve signup-credit, overdraft and abuse-control policy before code |
| #247 | P1 | Split | finalize balance status/code/retry policy and notification triggers |
| #241 | P2 | Split | #305 removes the silent `with_redis_url` no-op builder; parent remains open for broader architecture consistency |
| #235 | P2 | Planned | create ADR index and ownership/rollback records |
| #229 | P2 | Split | #304 merged the shared formula allowlist; add the remaining DX map and keep parent open |
| #226 | P2 | Planned | measure dependency tree and pair allowlist work with #229 |
| #225 | P1 | Split | generate tunnel env table and publish operations runbooks |
| #224 | P1 | Split | #302 merged multi-node preflight; publish capacity smoke test and Redis failure runbook |
| #223 | P1 | Split | #292 merged list/auth/idempotent redrive; run duplicate/poison, marker-TTL, capacity and real Redis replay drills |
| #222 | P2 | Planned | measure one provider/repository extension slice before generic rewrite |
| #221 | P2 | Planned | produce call/dependency graph and extract one tested boundary |
| #220 | P1 | Split | add Cargo/npm advisory scan and explicit policy fixture |
| #218 | P1 | Split | #297 merged JWT startup validation; decide non-loopback environment/TLS and Redis durability slices |
| #217 | P1 | Split | reopened after accidental auto-close; deliver #307 metrics/alerts, #308 readiness, and #306 RED/log/trace contracts |
| #312 | P1 | Ready | implement the durable Redis production overlay and replay drill under #218 |
| #311 | P1 | Ready | implement remote PostgreSQL TLS default hardening under #218 |
| #308 | P1 | Ready | define and implement readiness/health semantics under #217 |
| #307 | P1 | Ready | define metrics, billing counters and alerting foundation under #217 |
| #306 | P1 | Ready | define request RED dimensions and log/trace telemetry under #217 |
| #216 | P1 | Split | implement live-DB, VSCodex and build-performance child gates |
| #215 | P2 | Planned | measure synchronous logging/SSE filtering/lock contention before changes |
| #214 | P1 | Planned | add probe, graceful shutdown and accept-error acceptance tests |
| #213 | P2 | Planned | split giant handler and standardize error payload boundaries |
| #212 | P2 | Planned | consolidate cross-cutting capacity and dependency tests |
| #211 | P0/P1 | Split | #293 merged debug/file protection; verify all sensitive-field persistence, registry bounds, lease failure semantics and clean-log evidence |
| #210 | P2 | Planned | verify cryptographic/OAuth contracts with current dependency evidence |
| #209 | P2 | Planned | remove credential Debug/Serialize exposure and URL key residue |
| #208 | P2 | Planned | reproduce NUMERIC/f64 paths and add adapter regression coverage |
| #207 | P2 | Planned | bound internal errors and Windsurf buffering; add graceful shutdown slice |
| #206 | P1 | Split | #300 tracks image authorization cost bypass; implement fail-closed unknown paid-image estimate, then bounded pricing |
| #205 | P1 | Split | #299 merged opt-in/default-off and anti-downgrade; design signed provenance and non-root service slices |
| #315 | P1 | In progress | PR #320 defines ADR-0045; merge the decision gate, then implement the verifier and release signing slices |
| #314 | P1 | Ready | implement least-privilege systemd/OpenRC service identities after the #315 trust/upgrade decision and permission audit |
| #316 | P2 | Ready | audit historical NUMERIC rebate records and decide idempotent backfill under #208 |
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

## 2026-09-12 live checkpoint (after PRs #313, #317–#319)

The authoritative remote inventory is **52 open issues**: 28 P1 and 24 P2.
Their lifecycle labels are 15 `status:ready`, 26 `status:triage`, and 11
`status:blocked`; no open issue is currently marked `status:in-progress`.
This count is deliberately separate from the historical snapshot above.

Completed fork-only slices since the previous checkpoint:

- PR #313 merged the live delivery queue refresh (`dd43f597…`).
- PR #317 merged remote PostgreSQL TLS defaults (`b2dfa30…`); #311 is closed
  and its stale in-progress label was removed.
- PR #318 merged the opt-in durable Redis profile (`0c022c0…`). Protected CI
  ran the isolated Redis AOF kill/recovery and shipped-Lua idempotent redrive
  drill; #312 is closed with the evidence recorded in its final comment.
- PR #319 merged dependency-audit hardening (`6c92d879…`), including the
  required `Dependency Audit / check` ruleset context. #220 remains open for
  install checksum verification, image digest policy, container identity and
  broader dependency coverage.

Current delivery order:

1. **#315 signed release provenance** — PR #320 is the ADR decision gate;
   after it merges, implement the embedded-key verifier and protected release
   signing as separate PRs with offline rejection fixtures.
2. **#314 least-privilege tunnel service** — perform the systemd/OpenRC
   permission and upgrade-path audit before changing service identities.
3. **#306/#307/#308 observability** — keep the three bounded children ready;
   do not close parent #217 until runtime readiness, metrics and alert evidence
   exists.
4. **#316 historical NUMERIC audit** — read-only inventory and an idempotent
   dry-run decision under #208; no production backfill is implied.
5. **#220 residual supply-chain slices** — install checksum consumption,
   container digest/non-root decisions, and Dependabot/Docker coverage remain
   independently planned after the merged CI gate.

PRs are squash-only and fork-only. A parent issue is closed only when its full
acceptance criteria have evidence; a merged child slice changes the parent to
`status:triage` when residual scope remains. Every next slice must add focused
tests, pass the four required contexts (Rust, Frontend, Automation Policy and
Dependency Audit), and record the merge SHA and residual risks here and on its
Issue.

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
