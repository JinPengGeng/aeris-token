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
| 0 | Protect video-task secrets at rest | #211 | security and privacy / M-L | headers, prompts and provider credentials are redacted or encrypted, file permissions are tested, and no plaintext appears in logs, registry or error paths | Planned |
| 1 | Durable privileged-mutation audit | #255 | incident accountability / L | each mutation writes a queryable `audit_logs` row before success is returned; timeout/failure semantics and integration tests are documented | Planned |
| 2 | DLQ operator lifecycle | #223 | recoverability and billing correctness / M | bounded retention, authenticated listing, idempotent redrive, duplicate/poison tests and a real Redis replay drill | Planned |
| 3 | CI and supply-chain gates | #216, #220 | catches regressions and CVEs / M | live DB tests are intentionally gated, VSCodex is a real required check, build fan-out is measured, and Cargo/npm advisory policy runs in CI | Planned |
| 4 | Public API compatibility matrix | #247, #254 | prevents client retries and integration breakage / S-M | OpenAI/Claude status-code, error-code, envelope and retry-header matrix is documented and covered by fixtures; balance and notification transitions are explicit | In review (#290) |
| 5 | Operations reference and recovery runbook | #217, #218, #224, #225 | reproducible deployment and observability / M | metrics/alerts, environment table, multi-node topology, Redis failure semantics and restore drill are executable from published docs | Planned |
| 6 | Billing integrity follow-up | #206, #253 | protects revenue and abuse boundary / M-L | enrichment failure, cancellation, signup credit, quota and image authorization policies have explicit tests and owner sign-off | Planned |
| 7 | Scheduler and protocol roadmap slices | #268, #276, #179, #205 | correctness and upgrade safety / M-L | each slice has a bounded ADR, dependency/rollback plan and acceptance test; unresolved upstream sync conflict is handled separately | Deferred / blocked |
| 8 | Developer and architecture debt | #229, #221, #222, #226, #235, #241 | lowers long-term change cost / S-L | one billing allowlist, no-op builder removed or deprecated, ADR index and one measured extraction slice are merged | Planned |
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
| #254 | P1 | In review | finish #290, then add compatibility fixtures |
| #253 | P1 | Keep | approve signup-credit, overdraft and abuse-control policy before code |
| #247 | P1 | Split | finalize balance status/code/retry policy and notification triggers |
| #241 | P2 | Split | remove or deprecate the silent `with_redis_url` no-op builder |
| #235 | P2 | Planned | create ADR index and ownership/rollback records |
| #229 | P2 | Split | share formula allowlist with parser and engine; add DX map |
| #226 | P2 | Planned | measure dependency tree and pair allowlist work with #229 |
| #225 | P1 | Split | generate tunnel env table and publish operations runbooks |
| #224 | P1 | Planned | publish three-node reference topology and capacity smoke test |
| #223 | P1 | Split | implement list, auth, idempotent redrive and replay drill |
| #222 | P2 | Planned | measure one provider/repository extension slice before generic rewrite |
| #221 | P2 | Planned | produce call/dependency graph and extract one tested boundary |
| #220 | P1 | Split | add Cargo/npm advisory scan and explicit policy fixture |
| #218 | P1 | Planned | document production defaults, JWT checks and environment contract |
| #217 | P1 | Planned | define actionable metrics, alerts and readiness semantics |
| #216 | P1 | Split | implement live-DB, VSCodex and build-performance child gates |
| #215 | P2 | Planned | measure synchronous logging/SSE filtering/lock contention before changes |
| #214 | P1 | Planned | add probe, graceful shutdown and accept-error acceptance tests |
| #213 | P2 | Planned | split giant handler and standardize error payload boundaries |
| #212 | P2 | Planned | consolidate cross-cutting capacity and dependency tests |
| #211 | P0/P1 | Split | protect sensitive video fields, bound registry and define lease failure semantics |
| #210 | P2 | Planned | verify cryptographic/OAuth contracts with current dependency evidence |
| #209 | P2 | Planned | remove credential Debug/Serialize exposure and URL key residue |
| #208 | P2 | Planned | reproduce NUMERIC/f64 paths and add adapter regression coverage |
| #207 | P2 | Planned | bound internal errors and Windsurf buffering; add graceful shutdown slice |
| #206 | P1 | Split | preserve billing on enrichment failure and close authorization bypasses |
| #205 | P1 | Planned | require signed tunnel upgrades and scheduler admission tests |
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

## Completed slices and residual links

PRs #282–#289 are merged into the fork. They cover contributor entry points,
watchdog health feedback (#92), finite billing formula values, governance
records, TaskSupervisor drop cleanup, sparse OpenAI video polling, bounded DLQ
retention, and CI/release-gate decisions. These merges are evidence for the
corresponding child slices only; #211, #216, #220, #223, #247, #254 and #255
remain open until their residual acceptance criteria above are met.

PR #290 remains under required-check review. Its API documents and error
contract changes must be merged before #254 is marked complete; balance
notification behavior in #247 is intentionally not claimed by that PR.

