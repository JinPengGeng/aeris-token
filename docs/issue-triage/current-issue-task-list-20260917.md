# Current issue execution queue — updated 2026-09-21

## Current delivery batch

This checkpoint supersedes the historical counts and local-only status below.
GitHub Issue #443 and delivery PR #473 hold the current integration result.
Main includes #463 (exact cookie parsing), #464 (Tunnel diagnostics), #468
(notification deployment runbook), and #469 (release image configuration).

PR #473 combines #462, #465, #466, #467, #470, #471, and #472: public error
redaction, strict release checksums, multiplier validation, production encryption
key validation, signed release tags, and synchronous target admission. Original
PR branches are preserved and their automatic merges are paused while the combined
change passes the unchanged required checks. This avoids repeating a full Rust
and database run for every newly merged base. Close superseded PRs only after
verifying the corresponding changes merged through #473.

The batch also permits the supported zero-cost multiplier in the provider UI,
removes the duplicate PostgreSQL feature-check alias, and keeps narrative API
guides out of full Rust compilation while retaining all compiled documentation
fixtures and required gates. The first combined CI found one target-admission
fixture failure; its gate initialization is fixed and the exact regression now
passes locally. The new head still requires the complete hosted checks.

The original issues still contain development work. #222 retains trait/provider
registration architecture work. #241's administrator API lifecycle contract and
#225's missing Messages/Videos behavior documents and historical PoC notice are
implemented in this batch. Runtime configuration readers and their semantic documentation also
remain a separate development scope. Compose CPU, memory, and PID limits already
merged through #455; do not schedule that implementation again.

Recharge-triggered debt collection is implemented. Local supplier acceptance uses
the synthetic data authorized by the user. Actual deployment, historical records,
and unsupported paid-path contracts retain their own acceptance boundaries.
Do not describe all open issues as either undeveloped code or production-only work.

## Current delivery status

PR #447 contains the gateway implementation and its CI repair batch: audit
inventory 146, scoped history, fresh catalog fixtures, English public errors
and trace assertions, bounded waits for the two hanging tests, and the explicit
cyber-policy failover setting. Image heartbeat tests now preserve the existing
no-replay rule for sent image-generation operations. Live validation and merge
results are recorded on PR #447 and Issue #443.

The 14 issues pending closure are #44-#49, #51-#53, #247, #254, #255, #256,
and #307. They close only when the integrating change merges. The next batch
implements #218 Compose resource limits and #226 rustls feature selection in
an isolated worktree. Required CI and review remain part of delivery; waiting
for them does not block independent implementation work.

## Execution resumed on 2026-09-20

The user explicitly resumed development of the remaining queue on 2026-09-20.
The earlier pause is historical; its acceptance and handoff remain in
`followup-acceptance-20260918.md` and `pause-handoff-20260918.md`.
The initial four Luna agents prepared independent patches for automatic supplier-cost
capture (#431), shared HTTP/WebSocket attempt lifecycle (#46), operation-aware
replay (#49), and emergency-chain production integration (#44). The batch expanded to six
independent Luna assignments, then returned to four while the main agent
reviewed results. The emergency persistence patch required a Terra repair
after its first draft failed patch validation and code review.
The main agent owns upstream integration, patch review, and serialized Rust
validation. Publication was already authorized and has been performed;
Issue closure follows the actual PR merge, not local-only validation.
The original requirements of #44–#49 and #51–#53 are covered by this batch;
close them after #447 merges with its required checks, along with #254 once
the English-message follow-up passes. Deployment work remains separate.

The live upstream check found four commits after `fb25dde4`, ending at
`ba7c9f8b270cce63b0515299076b30129d7d64b4` (2026-09-19): user-group usage
views and Responses reasoning-channel correction. PR #452 resolved the test
import/fixture conflict and merged after hosted CI passed, at 02:15:26 UTC on
September 20. Fork main is `5534ba54f6dc93c6ce4aae276d556edd2ead49e0`; a fresh
fetch verifies the upstream tip is its ancestor and both merges have two
parents. Conflict #451 closed automatically. The development commit `4ce1d4c7` has the prior development head and
`5534ba54` as parents, preserving the reviewed upstream integration.

Repository: `JinPengGeng/aeris-token`. The September 20 hosted refresh returns
**45 open issues** and two open PRs: **#447** and **#440**. This is a
read-only inventory snapshot, not a count of independent executable work
packages.
Upstream conflict alerts **#446, #448 and #449 are closed**. The inventory label
refresh on **2026-09-20** contains 28 `status:in-progress`, 16
`status:triage`, and one `status:blocked` label. The current read-only snapshots are
`/Users/jinpeng/.agents/tmp/aeris-resume-20260920/hosted-refresh/issue-open.json`
and `.../pr-open.json`.

PR #452 is merged. Fork `main` is `5534ba54f6dc93c6ce4aae276d556edd2ead49e0`,
and upstream `ba7c9f8b270cce63b0515299076b30129d7d64b4` is its ancestor. PR #447
and #440 remain open. #447 is now conflict-free; its required checks must
pass on the final follow-up head. Successful checks from earlier heads remain
historical evidence and do not replace that requirement.
The earlier local checkpoint did not change GitHub labels or Project cards.
The subsequent user-directed upstream priority change set #446, #448 and
#449 to `priority:P0` / `status:in-progress` before their closure. Project field updates remain
unapplied: the current OAuth token has `read:project` but lacks `project`;
GitHub rejected `updateProjectV2ItemFieldValue` with `INSUFFICIENT_SCOPES`.

The earlier local results are in `followup-acceptance-20260918.md`: 63 PostgreSQL,
24 Gateway and 38 native administrator-audit exact invocations passed;
three-node pass17 and both real local Prometheus/Alertmanager delivery drills
passed. The protected audit delivery operations slice is implemented and
locally accepted. The remaining tasks below retain deployment and parent-issue
boundaries instead of treating these partial results as issue closure.
The subsequent retention batch now passed 50 native administrator-audit exact
invocations, including six retention cases with fresh migrations. See the
paired retention checkpoint in `issue-255-durable-audit-delivery.md` and
`/Users/jinpeng/.agents/tmp/aeris-retention-20260918/native-audit-verification.json`.

## Code delivery policy — user correction on 2026-09-18

Prioritize finishing the original issues' core code requirements. Implement
confirmed missing behavior and ordinary failure handling with the existing
components. Do not expand an issue into speculative reconciliation, self-healing,
exhaustive concurrency coverage or repeated fault drills. Keep focused tests
and required project checks proportional to the actual change.

Deliver independent agent patches through one integration batch. Keep a submitted
batch unchanged while its checks run; repair actual failures together before the
next push. Do not rebase every open PR after each merge. Run local Cargo checks
from one stable integration worktree with the same target directory and build
settings, so changing agent worktrees does not rebuild every local dependency.

Track code completion separately from PR integration, production deployment
and external acceptance. Those later stages must not block unrelated code
development. Additional hardening stays deferred unless a concrete defect or
an explicit requirement makes it necessary. The standing daily upstream-first
priority remains in effect.

The team remains **2–10 active subagents**. The main thread owns coordination,
verification, integration, final delivery, and write-boundary assignment.
A subtask that exceeds its role or write scope must return a concrete question
and evidence for the main thread to decide. Select models by task characteristics,
ambiguity, judgment difficulty, error consequences, and verifiability, rather
than by domain or a fixed escalation chain: use Luna for repetitive, readily
verifiable work; Terra for clear routine execution; Sol for high ambiguity,
conflicting evidence, or complex tradeoffs; and Astra for high-impact decisions
or independent review of critical disputes. The prior Astra call failed with
`invalid_model`; do not repeat it in this batch. Rust builds remain serialized
with two build jobs and a shared target; agent concurrency does not imply
simultaneous Cargo builds.

Completed under this policy: four administrator pages now use the existing
confirmation component (#255); frontend type checking and targeted lint passed
(only eight pre-existing formatting warnings remain). The #307/#217 enrichment,
terminal settlement, video settlement and quota/RPM fail-open production
counter call sites are already present, so no additional implementation or
fault-injection work is needed for those code requirements. Avoid adding a
confirmation inventory/CI framework or another alert-delivery drill.

The subsequent core batch has also completed the #218 runtime configuration
reference (including existing aliases and defaults; generator check passed)
and #256 contributor setup/fork instructions. A fresh #214 source review
confirmed ordinary PostgreSQL statement/lock timeouts and owner-forward total
timeouts already exist; these are not additional implementation work.

The first-paid referral callback correction passed 17 focused data-layer tests.
A source review on 2026-09-20 confirmed the #253 email-verification reward
callback and #208 strict provider-catalog row decoding are already implemented
in the local worktree; do not schedule duplicate development for these paths. #247 low-balance threshold production and its
`email_notifications`/`usage_alerts` preference consumers are implemented and
the focused notification suite passed 10 tests; deployed delivery remains a
separate acceptance boundary. #254 now covers unknown-model 404, known-but-
unavailable 503, aliases and directives. The latest local chat-fixture
regression selection passed 42 tests, with 0 failed and 2 ignored; it is local
evidence, not hosted CI. See [followup-acceptance-20260918.md](followup-acceptance-20260918.md).
Issue #431 now has fixed-point pricing and snapshot persistence with restricted
admin access. Local PostgreSQL checks cover immutable import receipts,
Unknown-to-Estimated/Known and Estimated-to-Known upgrades, one current-row
aggregate, and replay that cannot downgrade a newer current state; automatic
cost capture is now implemented locally and passed the real PostgreSQL Gateway
acceptance `pg-u2rc8q9a` for effective-price settled usage and replay.
The Gateway writer resolves exact effective supplier prices and creates one request
receipt after completed settlement; funded-image capture waits for the completed
parent lifecycle and records Unknown. See `resumed-development-20260920.md`.

The local scheduler foundation is delivered: #53 has a request-wide attempt
budget, #51 send admission is connected to production sends, and #52 HalfOpen
claims use the shared runtime lease primitives. #48, #51, #52, and #53 are
currently labeled `status:in-progress`. Their local acceptance does
not close parent deployment or multi-instance boundaries: retain #46 lifecycle
review, #44 emergency-mode review and #47 shared-backend scheduler acceptance.

## Immediate execution order

On 2026-09-18 the user changed the priority: check upstream every day and
merge available upstream changes before starting further issue slices.
The existing GitHub workflow `sync-upstream-minimal.yml` is active and
`AERIS_UPSTREAM_SYNC_ENABLED=true`; its schedule is `17 3 * * *`
(11:17 Asia/Shanghai, subject to GitHub scheduling delays). Resolve conflicts
in an isolated worktree, preserve fork behavior, run the required checks and
use a true merge commit. Resume the issue queue after upstream is integrated.
The #255 retention patch resumed after upstream integration and has now passed
the native audit runner. It does not displace the daily upstream priority.

PR **#450** is merged as **`af63f065b7737c7f3d3009a643b4e100f17134c2`**.
The candidate `56caadd838f5be2d77d99a26df544bdf87a6fe01` has the exact
fork/upstream tips as its two parents, and the hosted PR merge also has two
parents. Upstream `fb25dde4c9783eef7017383f49cf4362b5904e59` is an ancestor
of current main; no upstream commits are missing. Local validation passed:
formats 973, transport 528,
usage runtime 376, video core 43; Gateway video 103 (including PostgreSQL),
async 9, normalization 17, architecture 209 and OAuth 80; frontend 111 tests,
type checking and changed-file lint; strict affected-crate/Gateway Clippy,
format and diff checks. The ordinary funded-image selection passed four tests
and ignored 17 live targets; hosted Gateway job `105498654958` subsequently
executed all 17 exactly once with one pass and zero failures/ignored per target.
Its readiness and synthetic authenticated restore drills passed. All four
required contexts passed: Rust CI `35312970960`, Frontend CI `35312970751`,
Dependency Audit `35312970743` and Automation Policy `35312970736`.
Post-merge daily-sync run **`35314735571`** succeeded and reported that the
exact upstream tip is already an ancestor of main, with nothing to do.
This merge is based on fork main and does not publish the separate local
follow-up worktree. Evidence is retained at
`/Users/jinpeng/.agents/tmp/aeris-upstream-evidence-20260918/`.

| Order | Issues | Current evidence | Next implementation and acceptance |
| --- | --- | --- | --- |
| 1 | #451, #449, #448, #446; PRs #452, #450 | Completed: latest PR #452 merged as `5534ba54f` after hosted checks passed; upstream `ba7c9f8b` ancestry is verified and #451 is closed. The earlier #450 merge and its three conflict alerts remain completed. | Keep the enabled daily schedule and prioritize any new upstream commits or conflict alerts before the issue queue. GitHub may delay the scheduled start. |
| 2 | #216, #211 | The adopted upstream candidate passed 63 PostgreSQL and 24 Gateway exact targets, Gateway video 115 and async 30, plus strict Clippy and formatting. The new PostgreSQL target checks body-capture states through three actual repository read APIs. Gateway already starts the video poller under its background supervisor; two repository-backed poller tests cover due OpenAI tasks with and without an execution-runtime override. | Preserve these gates when integrating local work after upstream. Require one executed non-ignored test per exact target. Preserve video revision/fencing and successful-upstream accounting semantics, with drain-before-migration rollout. |
| 3 | #255 | Original core code requirements are implemented: audit persistence, protected forensic UI, Docker upgrade safeguards and consistent confirmation dialogs on the four remaining admin pages. Delivery operations and paired retention also passed the 50-invocation native audit runner. | Prepare the existing code for integration. Broad cross-table reconciliation and expansion across all mutation families are deferred hardening; deployment remains a separate stage. |
| 4 | #443, #225 | All 45 open issue mappings are retained. Upstream PR #452 is merged; the separate local follow-up changes remain uncommitted and undeployed. | Refresh the candidate and evidence after the audit/integration batch, retain logs and reclaim owned Rust artifacts after validation. Carry remaining release gates explicitly into the handoff. |

The latest complete inventories are **63 PostgreSQL / 24 Gateway**, executed
after the upstream and protocol corrections. See
`upstream-candidate-evidence/final/rust-summary.json` under the retained task
log root. The earlier 62/24 audit run and its separately corrected formatting
checkpoint remain in `audit-final.summary.json` and
`audit-final-recheck.summary.json`. Claimed-worker crash acceptance is in
`audit-worker-crash.summary.json`, with one executed, non-ignored target.
Earlier successful 57/22 and 58/22 checkpoints keep their historical scope.

## Next concrete audit slices

These five events were selected from current transaction boundaries. Session,
group and wallet slices are integrated and locally accepted. They are not the entire remainder of the
140-event inventory.

| Order | Issue and events | Implementation | Required acceptance |
| --- | --- | --- | --- |
| A | #255: single/all user-session revocation, two events | Locally accepted: three PostgreSQL targets, authenticated HTTP and memory fallback passed. Revoke-all returns a typed NotFound with no success intent for a concurrently deleted target; original JWTs survive rollback and fail after commit. | Preserve stable IDs, unchanged revoked timestamps and audit-only delivery retries. A new login ordered after revocation remains permitted. Hosted/deployed acceptance remains outside this checkpoint. |
| B | #255: user-group membership replacement, one event | Locally accepted: five PostgreSQL targets, authenticated HTTP and memory fallback passed. Concurrent whole-group replacements preserve complete requested sets, with late-member relocking and bounded contention failure. | Preserve rollback and delivery-only retry behavior. Default-group policy across transactions and cross-Gateway cache consistency remain separate. |
| C | #255: wallet balance adjustment/manual recharge, two events | Locally accepted: actual PostgreSQL, authenticated HTTP and memory fallback passed, including deferred recovery COMMIT failure, reused audit IDs with fresh order numbers, and audit retry without extra money/recovery writes. | Preserve the existing HTTP replay contract: even after a post-commit quota-read 502, repeated manual recharge creates another monetary operation. Audit delivery is independently replay-safe; client request idempotency is not added by this slice. |

The audit CI runner now registers A, B and C with isolated database setup: 13
PostgreSQL, five authenticated HTTP/crash and three no-database memory targets.
Its native socket-only path passed all 34 exact invocations locally using
PostgreSQL 15.19, including 13 migrations and 18 unique owned databases. Evidence
is in `audit-wallet-integration/final-20260917T112159Z/`; the stopped native
cluster was removed and its per-target logs retained.
The runner fixture passes and rejects empty/ignored results and command failures.
The eight new PostgreSQL targets passed with fresh migrations in
`audit-families-integration/postgres-20260917T104642Z/`; four new Gateway exact
targets passed in `gateway-20260917T104740Z/`. The final 63 PostgreSQL/24 Gateway
inventories, strict Gateway Clippy including tests, data all-target/all-feature
Clippy, formatting and diff checks passed in `final-20260917T105235Z/` under the
same evidence root. That earlier batch used the Docker wrapper; the wallet
batch subsequently executed the native socket-only launcher. The group's four
concurrency targets cover
concurrent empty-group replacement with late-member relocking, deletion before
an empty replacement, a later per-user CAS, and contention exhaustion without
business or audit writes. Initial failed fixture/compilation checkpoints remain
in the same evidence directory; they are not substituted for the passing run.

The protected delivery operations and paired-retention slices are now locally
accepted. The updated native audit runner has 21 PostgreSQL targets with 21
fresh migrations, five Gateway HTTP/crash parents and three memory targets:
50 exact invocations, each with one pass and no ignored result. Cross-table
reconciliation remains; expired-lease recovery already exists. Keep shared
PostgreSQL/Cargo execution serialized.

## Local slices already accepted

| Issues | Accepted scope and retained evidence | Remaining parent boundary |
| --- | --- | --- |
| #247, #206 | Refund notification outbox, consent/retry/restart and non-PostgreSQL fallback passed real loopback SMTP acceptance. Both MIME bodies now retain safe refund details when no template is configured. The final batch passed 34 ordinary refund tests and all 62 PostgreSQL/24 Gateway exact targets, including the two otherwise-ignored SMTP targets. #247 also has the low-balance threshold producer and `email_notifications`/`usage_alerts` consumers wired; `important_notification::tests` passed 10/0/0. See [followup-acceptance-20260918.md](followup-acceptance-20260918.md). | Deployed notification delivery remains separate. SMTP delivery is at least once; notification retries do not repeat money movement. |
| #206, #253, #300 | Automatic recharge-triggered collection uses credited-principal budgets, candidates frozen at credit, FIFO, owner/key checks, holds/gifts, partial collection and atomic replay-safe receipts. Twenty recovery PostgreSQL targets and authenticated wallet/API/SMTP acceptance passed in the recharge checkpoint. See `issue-206-recharge-recovery.md`. | Supported API/adapter coverage, orphaned paid-attempt recovery SLO, referral rules, costs and deployment are separate. |
| #300, #206 | Three synthetic supplier receipt tests passed with real loopback HTTP and PostgreSQL: full/partial/over-ceiling, frozen quote after catalog changes, and truncation followed by late receipt/replay. | User selected synthetic data. No supplier account is needed for this local scope; fixtures do not prove real supplier invoices or historical margin. |
| #223 | Native dump/restore passed 1/0/0 with 104 public tables, schema/sequence/trigger comparisons, nonempty recharge/refund/audit state, provider-cost prices, current Unknown/Estimated/Known snapshots and historical immutable snapshot-import receipts, active leases/hold, three callback replays, four refund replays and late-receipt settlement. Audit replay preserved the full configuration row and deduplicated the same event. Dedicated databases and the stopped owned cluster were removed, reclaiming 64,861,663 bytes. | Extend the nonempty fixture when new ledgers are added. Production backup objects, PITR and RPO/RTO remain unverified. JSONL omits complete request-fund, recharge, refund-notification and audit-delivery ledgers. |
| #220 | Existing automation checks, 15 image-gate fixtures and a synthetic dual-architecture local registry exercise passed. | Scan the actual release image and verify that exactly that digest is published; synthetic image evidence does not finish release acceptance. |

Detailed checkpoint results and artifact paths are in
`followup-acceptance-20260917.md` and
`../operations/native-financial-ledger-restore.md`.

## Next P1 and release work packages

Priority below is execution order, not a rewrite of GitHub labels.
Select bounded slices after the immediate batch; do not start every row at once.

| Issues | Remaining task | Implementation and completion evidence |
| --- | --- | --- |
| #300, #206 | Supported paid API/adapter matrix and orphaned-attempt recovery | Enumerate request/stream/multi-stage contracts and explicit recovery SLO. Rehearse restart, lease, late receipt and no duplicate charge. Add a durable worker only if the verified operator recovery procedure cannot satisfy the SLO. Unsupported paid paths stay fail-closed until their contracts exist. |
| #254, #247 | Public compatibility and notification deployment coverage | #254 now has unknown-model 404, known-but-unavailable 503, aliases and directives covered by the public-models selection (33 passed, 0 failed, 0 ignored); retain missing-credential 401 and the established balance-denial/route/header contracts. #247 low-balance threshold production and `email_notifications`/`usage_alerts` consumers are complete (notifications 10/0/0). See [followup-acceptance-20260918.md](followup-acceptance-20260918.md). Deployed notification delivery and remaining public compatibility acceptance stay open. |
| #214, #224, #212 | Deployment capacity and remaining scheduler assertions | Synthetic three-node pass17 passed owner/remote/stream/recovery relay, LB drain and Redis lease loss/recovery, with zero financial invariant violations. A Rust loopback regression now checks 429/500/502/503 and mixed responses through the actual HTTP probe and capacity classifier. Deployment-specific capacity/SLOs and #47 scheduler assertions remain. |
| #307, #217 | Integrate completed metrics code; deployment tracked separately | Core producer wiring is complete: worker/direct/video enrichment and settlement, plus quota/RPM fail-open, use the existing low-cardinality counters. Parser/rule tests and prior local delivery evidence exist. Do not repeat those implementations or add fault drills; production scrape/receiver evidence belongs to deployment acceptance. |
| #205, #303, #210 | Tunnel signing, crypto and key lifecycle | Validate formal signing configuration, signed release assets, deployed verification and rotation/recovery. Preserve existing Fernet/Python fixed-salt compatibility. Recheck the RSA advisory each release or every 30 days; the reviewed exception expires 2026-10-12 and is removed when a patched dependency is available. |
| #220, #218, #225, #256 | Release and deployment acceptance | Complete remaining runtime variable documentation, actual image scanning and same-digest publication checks. Exercise non-root/resource/TLS/Redis-persistence migration and rollback. Verify fork tunnel release assets before switching installer origin. |
| #223 | Production recovery and data lifecycle | Keep local native-restore and DLQ evidence, extend it for new outboxes, and validate actual backup/PITR/retention and measured RPO/RTO in the intended environment. |
| #255 | Integrate completed original audit/admin code | Confirmed confirmation-UI gaps are fixed. Keep optional additional atomic mutation families and cross-table reconciliation in a deferred hardening list; track deployment separately from code completion. |

## 2026-09-20 follow-up audit

The resumed development pass checked the remaining ordinary implementation paths
before adding new work. #211 already maps OpenAI `in_progress`/`processing`/
`running`, preserves sparse poll fields, rejects stale active polls after a
terminal state, retains video URLs, and runs the bounded poller/registry
retention path. #223 already exposes authenticated, bounded DLQ listing and
idempotent redrive with a seven-day marker TTL in both Redis and memory runtime
implementations. #224 already ships the three-node compose reference, three
node env templates, preflight, asset verifier, connection-pool budget and
failure-drill instructions. The verifier passed locally.

These three issues therefore have no newly confirmed ordinary code gap in this
pass. Their remaining work is environment evidence: real provider/task
acceptance and lease-loss drills for #211, production backup/PITR and Redis
replay evidence for #223, and real multi-node capacity/load-balancer/Redis
failure evidence for #224. Do not add another poller, DLQ worker, compose
topology or large test harness without a new reproducible failure.

## P2, evaluations and structural work

| Issues | Planned bounded task | Completion boundary |
| --- | --- | --- |
| #431, #253 | #431 locally accepted immutable snapshot-import receipts: Unknown can upgrade to Estimated or Known, Estimated can upgrade to Known, the current snapshot remains a single aggregate row, and replaying an old receipt cannot downgrade it. Automatic Gateway capture has one fresh PostgreSQL pass for effective-price settled usage and replay (`pg-u2rc8q9a`). | Token pricing can be Estimated; image pricing remains Unknown. Synthetic/local evidence does not establish historical production margin, issue completion, integration, or production acceptance. |
| #253 | Code present: zero default signup gift, first-paid and verified-email referral triggers, exact local-user eligibility, and idempotent reward keys. | Preserve existing acceptance; integration and deployment remain separate. Do not repeat the implemented callback or expand fraud policy without a concrete requirement. |
| #208, #316 | Provider-catalog strict row decoding is implemented; successful SQL NULL decoding retains its documented defaults while schema/type errors propagate. Historical rebate audit remains. | Reuse delivered NUMERIC/provider-key fixes. Historical rebate impact needs actual historical evidence; synthetic data verifies the audit tool only. Optional cross-table reconciliation stays deferred. |
| #215 | Completed the bounded SSE filter measurement: a prefilter already exists. The proposed JSON-key scanner regressed short mixed workloads by about 17–24%, so it was not applied. See `issue-215-sse-measurement-20260920.md`. | Retain existing semantics, nonblocking logging and RPM cleanup. No extra parser/cache complexity is justified by the measured result. |
| #207, #213, #221 | The Sub2API parser extraction is implemented locally with 26 passing tests. | Preserve endpoint/error behavior; integration and parent-issue acceptance remain open. Existing redaction, shutdown, bounded Windsurf buffering and crypto Rustdoc are already delivered. |
| #222, #226, #241 | #222 now has the five ProviderCost/emergency-grant tables in logical/generated schema plus the required-table guard; `compose_schema.sh check` and eight focused tests passed. #226's stage `+Inf` bucket passed seven focused tests. For the remaining scope, use one real provider/repository change to measure extension and migration cost; assess current TLS/transport dependencies and record deployment/cache/management-API consistency decisions. | A measured, bounded architectural change; avoid an unsupported general rewrite. |
| #179 | The two unused Writer client methods are deleted locally and the focused Node selection passed 12 tests. Retain the Automation Policy gate and disabled Writer sentinel pending a separately reviewed replacement contract. | Remote asset retirement and parent-issue acceptance remain open. |
| #157, #158 | Refreshed September 20: six #157 capabilities are already adopted or deliberately redesigned; upstream #750 is closed without merge, while the future group-pricing decision remains tracked. #158's provider-key scope is still absent and its frozen design retains the original activation conditions. | Both remain open under the recorded owner decisions. Current evaluation comments are [#157](https://github.com/JinPengGeng/aeris-token/issues/157#issuecomment-5748617539) and [#158](https://github.com/JinPengGeng/aeris-token/issues/158#issuecomment-5748617692); evaluation does not cancel their future scope or authorize automatic cherry-picks. |

## Ten roadmap dependency records

The bodies were refreshed. The September 18 snapshot labeled all ten blocked;
on September 20, #44/#45/#46/#47/#49 were changed to `status:in-progress` as
their remaining implementation resumed. These are primarily internal
design and acceptance dependencies; the bodies do not establish an external
hard blocker. Current implementation coverage still needs individual review.
As of the 2026-09-20 label refresh, #44/#45/#46/#47/#48/#49/#51/#52/#53 are
`status:in-progress`; only #1 remains `status:blocked`.
PR #40 and #41, named as initial implementations by #48 and #49, are both
CLOSED without a merge commit on GitHub. This does not establish whether
equivalent code reached the current branch by another route.

| Issues | Dependency review and intended scope |
| --- | --- |
| #1 | Revalidate the shared scheduling state machine and request-scoped immutable snapshot; coordinate the child acceptance matrix. |
| #51 | The body explicitly depends on scheduling snapshot and AttemptBudget (#53). After both are verified, recheck circuit/health/quota/RPM/concurrency immediately before send, returning Admit/Skip/Stop. |
| #52 | Review shared RuntimeState lease semantics, then validate TTL, monotonic fencing, acquire/renew/release and fail-closed behavior with a real shared backend. This can proceed alongside #46. |
| #49 | Locally accepted: ordinary request errors stop by default; credential rotation for 401/403 requires a trusted origin. 408/429/5xx remain eligible under policy, and explicit continue rules remain supported. Origin-aware effects and Responses WebSocket quota admission are connected. The compact-only gate blocks same-key and changed-candidate retries in sync/stream candidate loops. Focused validation passed fallback 28, attempt loop 40, compact routing 2, Responses WebSocket 215, classifier 32, policy 7, recovery 9, effects 66, four HTTP checks, and serving loop 6. Strict seven-crate Clippy passed with `-D warnings`; see `evening-checkpoint-summary.json` under the core-delivery evidence directory. The resumed slice now blocks automatic replay for image generation, video mutations and Gemini file mutations after dispatch. Tool declarations alone are not treated as executed side effects. |
| #46 | Responses WebSocket sets the `client_committed` barrier after a successful socket send for `response.*` and error events; control frames are excluded. The pause batch adds HTTP per-attempt Prepared/SentButUncommitted/ClientCommitted/Terminal tracking and a request-wide retry barrier at the response-body handoff. Headers, data and trailers are preserved. HTTP and Responses WS now share the lifecycle contract and logical request barrier, including the initial WS binding. Other transport-specific coverage remains a separate boundary. Final acceptance is in the pause checkpoint. |
| #53 | Verify one request budget across all retry paths, distinguishing policy stop, candidate exhaustion and budget exhaustion. Treat this as an early foundation review. |
| #45 | The pause batch adds trusted `failure_origin` and nested `classifier_disposition` to sync/stream error-flow trace data, with real Router candidate readback for 422 and credential 401. Classifier advice is distinct from the final replay decision. The JSON overlay has one fresh PostgreSQL readback pass (`pg-j_gwdpub`). Only observed Prepared/send phases are written; candidate terminal status remains distinct from client body handoff. |
| #47 | Real Redis exact targets passed shared affinity changes with 257 candidates, 16 contenders for one key permit, first-frame stream delivery followed by no replay, and a request-local 32-attempt ceiling with another Gateway request succeeding. HalfOpen and send-time authority checks were already accepted. New selectors are registered in required CI; retain hosted/deployment boundaries separately. |
| #48 | The native-scope repair is integrated locally; focused Gateway history (5) and formats history (42) tests passed. Native/hydrate/translate/unsupported contracts, tenant/API-key history isolation and native `previous_response_id` independence remain the acceptance boundary. |
| #44 | The local admin v1 matches #44's core request-scoped immutable chain, authorization, audit, expiry, rollback, and non-default behavior. Grant issue/revoke persistence plus consume each have one fresh PostgreSQL pass (`pg-scqt_ss0`); the Gateway fixed-order text model-test route passed actual HTTP/PG acceptance (`pg-p6f7xu54`), including declared target order, success stop, one-time consume and persisted audit. Public/tenant ledger and CAS extensions are deferred, not #44 core blockers; local work is uncommitted and unpublished. |

Planning order: verify current code and PR equivalence first; review #49/#53
foundations, then snapshot/#51; validate #46 and #52 in parallel; complete #45
and optional #44; execute #47 last. #48 runs as a separate protocol track.
Only #51 names snapshot and AttemptBudget as explicit prerequisites; the wider
ordering above is an implementation plan inferred from the acceptance contracts.

All 45 remaining open issue numbers are mapped above; the three resolved
upstream alerts retain their completed row. Parent issues remain open when
only a local slice has passed. Planned code, executed local tests, hosted CI,
release publication and deployed acceptance are separate states.
