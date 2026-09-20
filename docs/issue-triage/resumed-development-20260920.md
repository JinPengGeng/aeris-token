# Resumed development — 2026-09-20

## Current integration checkpoint

The #51 send-time admission, #53 retry-budget, and #254 local-error-contract
patches are integrated in the active local batch. This includes precise initial
`Pending` exclusion, revalidation that preserves RPM accounting, explicit
WebSocket release, HTTP OAuth/Windsurf cross-task budgets, WebSocket logical
budgets, top-level local error trace IDs, and consistent model-404 usage. This
is distinct from the hosted inventory of 45 open parent/issues and two open
PRs. The integrated code was committed and pushed to #447 as
`4ce1d4c7bfcc276a6d4083857274e56a43075fb3`; conflicts with main are resolved.
#440 is covered by this implementation and will close after #447 merges.
No development issue has been closed before the actual merge.

The three local implementations and their final targeted Gateway validation
are complete; publication is done and hosted CI integration is in progress.
The subsequent #254 follow-up makes locally generated execution and IP access
errors English, without translating upstream payloads. CI found stale migration
and recharge-copy assertions, an integration-test stack setting omitted from one
runner, and the funded-image memory fixture missing its provider catalog.
These follow-up fixes await hosted verification on the published head. The preceding local run reports
456 passed, 0 failed, and 2 ignored:
`/Users/jinpeng/.agents/tmp/aeris-delivery-20260920/final-three-slices-20260920T175230.log`.
The two ignored Redis targets were not rerun and are not counted as passes.
An integration compile also exhausted the remaining `LocalAdmission` callers
and repaired the missing `server.rs` case. Earlier failures remain historical.
This count is for this final selection only and is not combined with prior
results. The separate `aether-ai-serving` `attempt_loop` addition passed 6,
failed 0, and ignored 0; evidence is
`/Users/jinpeng/.agents/tmp/aeris-delivery-20260920/serving-attempt-loop-final.json`.
The full-repository format and diff checks also passed.

The issue tracker now records this state in #443 and individual #51/#53/#254
comments, verified by readback. The current upstream SHA remains `ba7c9f8b`;
`git rev-list --left-right --count upstream-snapshot...origin/main` returns
`0 314`, confirming that fork main contains it.

The user explicitly resumed the remaining development queue. The September 18
pause checkpoint remains historical. The initial batch used Luna for bounded
implementation and reached six independent assignments. The current team uses
the user's revised allocation rule: Luna for clear, repetitive, readily checked
work; Terra for ordinary execution toward a clear goal; Sol for ambiguity,
conflicting evidence or difficult tradeoffs; Astra for independent review of
high-impact decisions or critical disputes. Assignment depends on the task and
its consequences, not its domain or a mandatory escalation ladder.

The main agent owns coordination, verification, integration and final delivery,
with separate write boundaries. Agents return concrete questions and evidence
when a task exceeds their role or scope. The active target remains 2–10 agents;
model/service failures reduce concurrency and cause reassignment. The earlier
Astra review call returned `invalid_model`, so it is unavailable in this batch.
Cargo runs remain serialized with two build jobs and one shared target directory.

The follow-up #225 environment-reference slice is now implemented locally. The
reference generator records source-confirmed compatibility precedence for the
database, Redis, encryption, JWT, instance, and Windsurf bridge settings, and
includes the current test Redis URL readers. The generated reference and
checker agree after correcting the encryption mismatch behavior: a conflicting
gateway-specific key logs a warning and wins; startup does not reject it.
Deployment validation remains a separate acceptance boundary.

## Upstream first

Upstream advanced four commits to `ba7c9f8b270cce63b0515299076b30129d7d64b4`.
The sync workflow run `35481748824` stopped on a memory-usage-test import conflict
and opened #451. The isolated resolution retains both the fork daily-cost
imports and upstream time-series import. The new user-group stats fixture also
supplies the fork daily-usage-limit fields.

PR #452 merged on September 20 at 02:15:26 UTC as
`5534ba54f6dc93c6ce4aae276d556edd2ead49e0` after all hosted checks passed.
Its candidate `5f83f008c7aec9e1ec81d535a7f392a61307df90` has exact parents
`af63f065b7737c7f3d3009a643b4e100f17134c2` and the upstream tip. Both the
candidate and hosted merge have two parents; a fresh fetch verifies upstream
ancestry of main. Conflict #451 closed automatically. Local validation passed formats 974,
usage-memory 47, admin-stats 8, Gateway stats 44 and reasoning 28 tests; the
additional authenticated stats selection passed 30 and overlaps the 44.
Frontend tests 3, type checking, changed-file ESLint, format and diff checks
passed. The published development commit retains the prior development head
and the synced main as its two parents.

## Implemented and locally checked

- #46: Responses WebSocket now uses the same attempt lifecycle and logical-turn
  commit barrier as HTTP. Upstream send callbacks mark SentButUncommitted only
  for PossiblySent/Sent. Successful public socket handoff commits; detach,
  termination and guard drop finish the attempt without clearing request commit.
- #49: image generation, video mutations and Gemini file mutations cannot be
  automatically replayed after dispatch. Ordinary generation and read-only
  file/video operations remain eligible under the existing failure policy.
- #47/#52: two independent Gateway states sharing real Redis race for one
  HalfOpen probe; exactly one wins, and a later owner rejects the old fencing
  token. Send-time admission also passed a real two-Gateway Redis check with
  authority revocation and an isolated Redis outage. Explicit positive key
  concurrency limits now acquire the existing runtime semaphore before Admit;
  the guard owns its TTL, renewal and release. Real Redis acceptance now passes
  16 competing requests across two Gateways (one admission, 15 denied, then
  admission after release), shared affinity changes across 257 candidates,
  post-commit stream no-replay and a request-local 32-attempt ceiling.
- #45: scheduler generation/page, budget counters/reasons, lifecycle phase and
  the actual replay-admission boolean now survive candidate persistence and
  public/admin sanitization. Decision-point producers now record Prepared,
  send handoff, budget exhaustion and the actual replay decision. PostgreSQL
  JSON overlays preserve seeded ordering fields and existing diagnostics;
  classifier advice is kept separate from the actual replay decision. Fresh
  PostgreSQL migration and repository readback passed, including legacy JSON
  cleanup and terminal diagnostics preservation. The real budget fixture also
  caught a missing stable error-type allowlist entry: request budget exhaustion
  now retains `request_attempt_budget_exhausted` instead of `unclassified_error`.

The initial local checkpoint passed 408 tests in six selections: lifecycle 7,
Responses WebSocket 215, attempt loop 43, real Redis HalfOpen 1, candidate
contracts 11, scheduler core 131. These are local results for this checkpoint,
not hosted checks of PR #447. Evidence:
`/Users/jinpeng/.agents/tmp/aeris-resume-20260920/local-validation/checkpoint-summary.json`.

## Additional local implementation and acceptance

#431 automatic provider-cost capture now queries exact supplier/provider/model
prices at the request's occurrence time. It stores one aggregate request
estimate or explicit Unknown receipt; funded-image capture waits for the
completed parent lifecycle. Raw token field presence is required to infer a
trusted usage marker, and explicit false markers remain false. The new PG
fixture reads both current and immutable receipts and checks that replay cannot
downgrade a Known invoice. The real PostgreSQL Gateway fixture
`pg-u2rc8q9a` passed on September 20, using occurrence-time prices and
preserving the single current aggregate across replay and a later invoice.

#44 issue/read/revoke persistence and transaction-coupled audit passed two
contract tests and a real fresh PostgreSQL fixture, including audit-failure
rollback. The Gateway operations route now preserves explicit target order,
uses a five-minute administrator-owned grant, consumes it once before sending,
and rechecks grant/catalog authority before every target. Only synchronous
text model tests are supported. Fresh PostgreSQL tests passed issuance/revoke
audit atomicity and concurrent single-use consumption. The actual HTTP/PG
fixture also passed (`pg-p6f7xu54`): declared target order, 429-to-200 failover,
stop after success, consumed grant, persisted target order and audit readback. The original Issue #44 asks for operations-only routing;
public/tenant emergency scheduling and its optional ledger/CAS scaffold are
outside that scope and do not block this implementation.

The subsequent local validation passed strict Clippy for eight affected crates
(lib and tests with `-D warnings`), 67 usage-write tests, nine funded-attempt
tests and 45 attempt-loop tests. The PostgreSQL CI shell gate passed with two
separate owned provider-cost databases. All task-owned PostgreSQL clusters were
stopped and their data removed. The remaining Gateway/PG selections continue.

The OAuth batch importer now separates Sub2API account decoding into a small
module while retaining the existing entry point. #179 removed two unused
Writer Environment API methods and their dedicated tests; the remaining 12
client tests passed. The current policy gate still consumes the Writer sentinel
configuration and remains intact. #215's measured scanner proposal was rejected
because it regressed common mixed workloads; the existing prefilter remains.

#47 shared-backend acceptance found that affinity writes reached Redis while
the selector only read its process-local cache. Selectors now hydrate once per
request, retain one target snapshot across ranking and both page-cache keys,
and keep the captured generation when remembering affinity. Direct/eager
ranking also reads the shared backend. The live fixture warms Gateway B,
changes the preferred target through A, then confirms B sees the new target
without duplicate or missing candidates across the 256/1 page boundary.
Real HTTP/Redis fixtures passed client-observed first-frame delivery followed
by a disconnected stream with one actual runtime send, and 32 failed sends
followed by budget stop while a separate Gateway request succeeds. Candidate
indices and lazy trace materialization are not used as proxies for sends.
All three new ignored selectors are registered in the required Redis runner.

The final focused results are planner 224, model-test 52 and candidate contracts
11, plus one executed pass for each real Redis exact target. The #44 HTTP/PG
fixture passed with fresh migration, issue/revoke and single-use consumption
(`pg-p6f7xu54`). Counts from overlapping selections are not added together.
See `local-validation/verified-checkpoint-20260920.json` under the retained
September 20 scratch evidence directory. Owned PostgreSQL data was removed;
the duplicate nested Rust target accounted for 299,812 KiB before removal.
The shared Cargo target remains available and incremental output is disabled.
Production deployment, actual release-image publication, PITR/RPO/RTO and
historical production margin remain separate acceptance work.

## Confirmed existing local scope

- #211's Gateway video poller is started by the background supervisor and has
  two repository-backed tests for due OpenAI tasks.
- #48's native no-local-scope repair passed the focused Gateway history (5)
  and formats history (42) selections.
- #222's five ProviderCost/emergency-grant tables now have logical/generated
  schema and a required-table guard; `compose_schema.sh check` and eight
  focused tests passed.
- #226's stage `+Inf` bucket passed seven focused tests.

The 2026-09-20 follow-up audit also checked the remaining ordinary paths before
starting another implementation batch. #211's OpenAI video status mapping,
sparse-field projection, terminal fencing, bounded registry retention and
background poller are present. #223's authenticated DLQ page/redrive API,
bounded page size, idempotency marker and Redis/memory backend implementations
are present. #224's three-node compose/env assets, preflight, verifier and
failure-drill runbook are present; `tools/operations/verify_multi_node_assets.sh`
passed. No additional ordinary code gap was confirmed in these three issues.
Their open boundaries are real deployment/provider evidence only, so no new
poller, DLQ worker, compose topology or broad test harness was added.

These local corrections do not close their parent issues. #44 HTTP/PG and #47
shared-Redis acceptance have passed locally; integration and deployment remain
separate from those results.

A fresh source audit confirmed #253 email-verification referral callbacks and
#208 strict provider-catalog row decoding already exist in the local worktree;
no duplicate implementation is scheduled for those paths.

The initial resume comment remains at
https://github.com/JinPengGeng/aeris-token/issues/443#issuecomment-5747091812.
The validated checkpoint now replaces the obsolete pause at the top of
https://github.com/JinPengGeng/aeris-token/issues/443; the prior pause is preserved
as history. The full issue body was read back and matched the published draft.
The labels on #44/#45/#46/#47/#49/#431/#443 now reflect active development.
These local changes remain uncommitted; PR #452 only contains the upstream merge.

## Per-issue status synchronization

The September 20 follow-up updated #48, #51, #52 and #53 from
`status:blocked` to `status:in-progress` and published individual implementation
updates. #216 and #225 also received their own current local progress comments.
These issues remain open because the new local changes have not been published;
the updates do not claim completion of their entire acceptance scope. Project
fields still require the missing `project` write scope and can retain older
Blocked/Inbox values even when issue labels are current. Read-back of the issue
labels and comment URLs is the synchronization evidence.

The source/document review corrected two obsolete API descriptions: authenticated
Chat and Images requests reaching candidate selection distinguish an unknown
public model (`404/model_not_found`) from a declared but unavailable model
(`503`). The #431 ledger record now describes the existing automatic token-cost
estimate/Unknown capture path instead of claiming that only explicit imports
exist. These are documentation corrections to the local implementation.

Two old audit statements must not create new implementation work. The
`issue-300-gateway-attempt-funds.md` historical hard-quota paragraph predates the
implemented per-attempt hard plan quota; user/key daily actual-cost limits retain
their documented soft behavior. `docs/operations/usage-header-capture.md` and
commit `7113d04f8` explicitly preserve original captured HTTP headers; the
older sensitive-header finding cannot authorize reversing that behavior.

The current shell-only verification passed the environment-reference generator,
the PostgreSQL live-runner gate fixture and `git diff --check`. No new full Rust
suite was needed for these documentation and GitHub status updates.

## Chat request validation follow-up

#254 now validates the authenticated `/v1/chat/completions` body as a JSON object
with a nonempty `messages` array. Invalid input returns the existing OpenAI
`400/invalid_request_error` envelope before upstream execution; authentication
still takes precedence. Default-model and alias behavior is retained.

The existing invalid-request fixture is now exercised through the actual Router.
Success, runtime-error and failover fixtures use a minimal valid user message.
The failed-usage fixture now asserts one failed candidate for its single endpoint:
the accepted provider-5xx policy excludes the failed endpoint instead of retrying
the same key. Its usage, billing and error assertions remain in place. The wallet
fixture uses the same 16 MiB test-thread stack as the other usage fixtures.

The live GitHub read-back confirmed 45 open Issues and two open PRs. #447 remains
`DIRTY` and #440 remains `BEHIND`; issue-label/comment updates do not publish the
local source changes. The remote fork main includes upstream `ba7c9f8b`, with
zero upstream commits missing, via merged PR #452; its conflict Issue #451 is
closed. Project fields require the absent `project` write scope.

The final focused Chat/usage/failover selection passed 42 tests with zero
failures. Two isolated-Redis tests were ignored in this selection and were not
rerun for this fixture-only follow-up. Evidence is retained in
`local-validation/chat-fixture-regression-20260920T160144.log` under the September
20 scratch directory. Formatting and `git diff --check` passed. These results
are local; they are not hosted PR checks or a merged delivery.
