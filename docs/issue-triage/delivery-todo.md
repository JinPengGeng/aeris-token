# Issue delivery TODO and community workflow

Snapshot: **2026-09-12**, fork `JinPengGeng/aeris-token`, based on the
`origin/main` tree after PR #322. This file is a current-state queue, not a
claim that an upstream issue is fixed. It is maintained from the fork's live
GitHub Issue/PR state and never changes `fawney19/Aether`.

## Current inventory

- **50 open Issues**: 26 P1, 24 P2; labels are 9 `status:ready`, 4
  `status:in-progress` (#306, #307, #308, #316), 26 `status:triage`, and
  11 `status:blocked`. Project #1 mirrors these labels; explicitly deferred
  roadmap work uses its Deferred column. Open parents #217 and #223 have
  been removed from the erroneous Done column.
- **Three open implementation PRs**: [#323](https://github.com/JinPengGeng/aeris-token/pull/323)
  (#307 metrics, lockfile, exporter and failure-path review corrections),
  [#324](https://github.com/JinPengGeng/aeris-token/pull/324) (#306 RED
  telemetry, runtime producer and lifecycle acceptance still incomplete), and
  [#325](https://github.com/JinPengGeng/aeris-token/pull/325) (#316 historical
  NUMERIC audit, review corrections tested on isolated PostgreSQL and new CI
  pending). #323/#324 auto-merge is paused until review findings are resolved;
  #325 auto-merge is enabled for the corrected toolkit. It does not close
  #316 without historical impact evidence. #308 has an initial implementation
  under review for bounded concurrency, lifecycle and failure tests, with no
  PR open yet.
- Every merge must pass the four protected contexts: `Rust CI / check`,
  `Frontend CI / check`, `Automation Policy / gate`, and `Dependency Audit /
  check`. A skipped path-specific job is acceptable only when the aggregate
  context succeeds.

## Delivery rules

Every issue follows the same auditable path:

1. Revalidate the report against current source, CI and runtime evidence.
2. Record scope, benefit, complexity, owner, acceptance criteria and residual
   risk in the Issue or a linked decision note. A parent remains open while a
   child slice is incomplete.
3. Move the Issue to `status:ready` only after the acceptance boundary is
   agreed; use `status:in-progress` when an implementation PR is active and
   return to `status:triage` when a merged slice leaves residual scope.
4. Implement one bounded slice in a short-lived fork branch. Do not modify
   upstream. Add focused regression/integration tests and update the relevant
   runbook or ADR.
5. Open a PR that links the Issue, documents risk/rollback and records the
   exact verification commands. Request review and wait for all four required
   contexts.
6. Squash-merge only after review and checks pass. Enable auto-merge where the
   branch protection policy allows it; never bypass a failing or missing
   context.
7. After merge, record the merge SHA, evidence and residual scope in the
   Issue, reconcile labels and Project status, and update this file. Close a
   parent only when its complete acceptance criteria have evidence.

## Ordered delivery queue

| Order | Work package | Issues | Benefit / complexity | Exit criteria | State |
| --- | --- | --- | --- | --- | --- |
| 0 | Observability and data evidence already in review | #306, #307, #308, #316 | production diagnosis and billing safety / S-M | #323/#324/#325 pass review plus four required contexts; #308 has real readiness tests and a PR | Active: three PRs open; #308 implementation underway |
| 1 | Secret persistence and privileged audit | #211, #255 | prevents credential disclosure and untraceable admin mutation / M-L | verify remaining persistence, registry and lease paths; exercise durable audit authorization and failure semantics against real repositories | Next security lane after current work in progress; earlier child PRs do not close these parents |
| 2 | Supply-chain exception and residual gate | #220, #303 | prevents silent advisory regressions / S-M | maintain the advisory gate and expiring RSA exception; decide remaining container identity and dependency update coverage from current evidence | Checksum consumption and production Dockerfile digest pins confirmed in source; no duplicate implementation |
| 3 | Runtime security and configuration parent follow-up | #205, #218 | removes upgrade/configuration attack paths / M-L | signed provenance and least-privilege service slices are recorded as complete; remaining env, TLS and privileged-upgrade boundaries have tests/docs | Parent triage; #314/#315 closed child slices |
| 4 | Billing authorization and settlement | #206, #253, #300, #247 | protects revenue and client-visible billing semantics / M-L | record conservative billing defaults and atomic reservation policy in a reviewed ADR, then test bounded estimates, wallet floors, signup credit and notifications | Triage/ready split; existing unknown-cost fail-closed behavior remains the baseline |
| 5 | Operations recovery and horizontal scale | #217, #223, #224, #225 | recoverability and deployability / M | readiness, metrics, DLQ replay/restore, capacity and tunnel environment runbooks are executable | Parent triage; child slices scheduled |
| 6 | CI and API contracts | #216, #254, #256 | catches regressions and prevents integration breakage / S-M | live-DB/build gates and compatibility fixtures pass; contributor/install docs reflect fork artifacts | Ready with residual acceptance |
| 7 | Runtime correctness and data debt | #214, #207, #209, #210, #208 | protects availability and data integrity / M-L | each issue has a bounded reproduction and regression evidence; historical NUMERIC audit is read-only unless a reviewed migration is justified | Triage; #316 is the current #208 child |
| 8 | Architecture and developer experience | #229, #221, #222, #226, #235, #241, #212, #213, #215 | reduces future change cost / S-L | an ADR/ownership record and one measured extraction or performance slice exists for each accepted item | Triage with selected ready slices |
| 9 | Scheduler/protocol roadmap | #268, #276, #179, #157, #158, #1, #44–#53 | correctness and upstream adoption / M-L | upstream conflict and lifecycle contracts are resolved before implementation; blocked Issues stay blocked with the dependency recorded | Deferred/blocked |

## Open issue disposition

The table below is the complete 50-item open-issue intake. “Keep” means the
report remains valid or needs evidence; “split” means the parent stays open
while the named child carries a bounded slice; “defer” means no implementation
is scheduled until the stated dependency is resolved.

| Issue | Priority | Decision | Next action / acceptance boundary |
| --- | --- | --- | --- |
| #308 | P1 | Keep, active | Implement readiness versus liveness semantics, dependency failure mapping and probe tests; open a PR before changing the parent #217 state. |
| #307 | P1 | Split, active | Review PR #323; require request/billing counters, bounded labels, alert thresholds and failure-path tests. |
| #306 | P1 | Split, active | Review PR #324; require RED dimensions, cardinality limits, structured log policy and trace correlation tests. |
| #303 | P1 | Keep, ready | Maintain an exact RUSTSEC-2023-0071 exception with owner, review date, exposure assessment and removal trigger; never disable fail-closed audit. |
| #300 | P1 | Keep, triage | Record the image price upper bound, wallet floor and atomic reservation policy in an ADR, using the current fail-closed baseline; implement with pricing and concurrency tests. |
| #276 | P1 | Blocked | Resolve the real upstream sync conflict with a reviewed merge commit; no blind conflict resolution. |
| #268 | P1 | Keep, triage | Decide terminal telemetry semantics and failure-origin mapping; coordinate with scheduler Issues #49/#53. |
| #255 | P1 | Split, ready | Finish durable audit authorization/failure semantics and Docker upgrade safety; #294 is only a merged child slice. |
| #254 | P1 | Split, ready | Add chat/images compatibility fixtures for status, error code, envelope and retry headers; #290 is only the baseline. |
| #253 | P1 | Keep, triage | Record signup credit, overdraft and `insufficient_quota` policy with abuse controls before coding. |
| #247 | P1 | Split, ready | Define balance error/status/retry and notification transitions; add API and settlement tests. |
| #225 | P1 | Keep, triage | Reconcile tunnel environment names, operational runbooks and ADR status against current manifests. |
| #224 | P1 | Split, triage | Extend #302 preflight with capacity evidence, cache consistency and Redis failure/restore drills. |
| #223 | P1 | Split, ready | Run duplicate/poison DLQ replay, marker-TTL, capacity and real Redis recovery drills after #292. |
| #220 | P1 | Split, triage | Current source already consumes checksum manifests and pins production image digests; address Docker/npm updater coverage and container identity separately. Published artifact verification remains distinct. See evidence below. |
| #218 | P1 | Split, triage | Keep the parent open for environment documentation, non-loopback/TLS policy and privileged upgrade boundaries; #311/#312 are closed children. |
| #217 | P1 | Split, triage | Do not close until #306, #307 and #308 provide runtime, metric and alert evidence. |
| #216 | P1 | Split, ready | Complete live-DB, build performance and VSCodex gate slices; dependency audit is now a required context. |
| #214 | P1 | Keep, triage | Add probe failure, graceful shutdown, accept-error and default-isolation acceptance tests. |
| #211 | P1 | Split, ready | Audit every secret persistence/error path, registry bounds, lease failures and clean-log evidence after #293. |
| #206 | P1 | Split, triage | Keep billing parent open; #300 covers image authorization policy while enrichment, cancellation and wallet semantics remain. |
| #205 | P1 | Split, triage | Keep scheduler/upgrade parent open for remaining admission and rollback contracts; #314/#315 signed-service slices are closed. |
| #53 | P1 | Blocked | Approve the scheduler attempt-budget contract before implementation. |
| #51 | P1 | Blocked | Approve dynamic admission revalidation and failure behavior before implementation. |
| #49 | P1 | Blocked | Pair failure-origin and replay policy with #268's terminal telemetry decision. |
| #46 | P1 | Blocked | Define attempt/client-commit lifecycle and replay boundaries first. |
| #256 | P2 | Split, ready | Keep contributor/fork documentation residuals explicit; migrate installer URLs only when fork artifacts are published. |
| #241 | P2 | Keep, triage | Record architecture identity, management API evolution and consistency model; #305's no-op builder fix is only a child slice. |
| #235 | P2 | Keep, triage | Build an ADR index, ownership map and rollback records; do not claim bus-factor resolution from documentation alone. |
| #229 | P2 | Split, ready | Extend #304's formula allowlist with the remaining developer-experience navigation and feedback-loop work. |
| #226 | P2 | Keep, triage | Measure the dependency tree and feature-risk boundary before changing `wreq`/BoringSSL/rustls choices. |
| #222 | P2 | Keep, triage | Measure one provider/repository extension and migration-drift path before a generic rewrite. |
| #221 | P2 | Keep, triage | Produce a dependency/call graph and one tested extraction boundary; avoid a broad crate split without measurements. |
| #215 | P2 | Keep, triage | Measure synchronous logging, SSE prefiltering and lock contention before optimization. |
| #213 | P2 | Keep, triage | Split the giant handler and standardize error payload boundaries with API regression coverage. |
| #212 | P2 | Keep, triage | Consolidate cross-cutting capacity/dependency/frontend tests and record duplicate-version evidence. |
| #210 | P2 | Keep, triage | Verify cryptographic/OAuth contracts against current dependencies and add focused misuse tests. |
| #209 | P2 | Keep, triage | Remove credential `Debug`/`Serialize` exposure and URL key residue with redaction tests. |
| #208 | P2 | Split, triage | Keep the parent open while #316 inventories historical rows and records whether any reviewed backfill is needed. |
| #316 | P2 | Split, active | Review PR #325's read-only historical NUMERIC inventory; decide explicitly whether an idempotent backfill is needed. |
| #207 | P2 | Keep, triage | Bound internal error exposure and Windsurf buffering; add graceful shutdown coverage. |
| #179 | P2 | Defer | Retain the automation-v2 roadmap; move actionable slices into bounded child Issues. |
| #158 | P2 | Defer | Evaluate provider-scoped allowlists only against an updated upstream baseline. |
| #157 | P2 | Defer | Refresh the upstream billing/quota registry before considering selective adoption. |
| #52 | P2 | Blocked | Define a distributed HalfOpen probe lease contract before implementation. |
| #48 | P2 | Blocked | Approve conversation-history capability contracts and compatibility rules. |
| #47 | P2 | Blocked | Keep multi-instance acceptance under the #224 topology plan until its prerequisites exist. |
| #45 | P2 | Blocked | Emit structured scheduling traces only after the lifecycle model is approved. |
| #44 | P2 | Blocked | Defer the expiring deterministic emergency chain until scheduler contracts are stable. |
| #1 | P2 | Blocked | Umbrella scheduler issue only; do not duplicate implementation in this parent. |

## Closed child slices and residual scope

The following Issues are **closed** and intentionally absent from the open
table: #311 (PostgreSQL TLS defaults), #312 (durable Redis profile and replay
drill), #314 (least-privilege tunnel service), and #315 (signed tunnel release
provenance). Their merge evidence and residual boundaries remain in their final
Issue comments and linked PRs. Closing a child does not close #205 or #218.

Other merged child slices (#282–#310, #317–#322) likewise prove only their
documented acceptance boundary. Parent Issues remain open wherever residual
criteria are listed above.

## #220 evidence note

Source revalidation at `5b0999f4c0cc0ba588d1139715b8c8696c942b66` confirms
`install.sh:1475-1476` downloads the canonical `SHA256SUMS` and calls
`verify_release_checksum` before archive validation/extraction, including when
the archive comes from a custom URL. `Dockerfile.app:13,30` pins BusyBox and
distroless by digest. `bash tests/release_supply_chain_test.sh` passes for the
production pins and provenance workflow. These are source/fixture checks;
they do not establish the contents of a published image or release bundle.

The same source explicitly uses `USER 0:0` in `Dockerfile.app`, and the current
container security fixture locks that behavior. A change therefore needs a
volume/upgrade compatibility decision and tests, not a blind USER edit.
`.github/dependabot.yml` covers Cargo, frontend npm, automation npm and
Actions; root/vscodex npm and Docker update coverage still need a bounded
follow-up. The advisory workflow's broader lockfile scan is a separate
capability already enforced by the fourth required context. #220 stays open
for these evidenced residuals; duplicate checksum/digest implementation is
not planned.

## Recordkeeping

Every state change is recorded in the Issue/PR and, when the decision affects
multiple Issues, in a linked ADR or note under `docs/issue-triage/`. The next
refresh must repeat the live inventory command and replace this checkpoint in
one edit; do not append contradictory historical snapshots.
