# Project board audit

Scope: GitHub Project v2 `JinPengGeng/aeris-token Development` (project 1,
`PVT_kwHOAiijc84BgApR`), with no upstream changes.

## 2026-09-13 current live refresh (after PR #363 and new delivery slices)

GitHub issue labels were re-read on 2026-09-13. The fork has **45 open issues**
(22 P1, 23 P2): 15 `status:in-progress`, 19 `status:triage`, and 11
`status:blocked`. Issues #217 and #224 moved from triage to in-progress;
closed #345 has its stale lifecycle label removed and its Project card is Done.
Older counts below remain timestamped audit records.

PR #361 merged as `1831852e9abecfdc267ddbf783d2203cd2d39e14`; #356 merged as
`86bc4d8b88311ac3ddd400d9e72a2d68bdc4c9f6` and Issue #308 is **Done**.
PR #358 merged as `1bcff1a5271aebefa96253417f4707574a7a2994`; PR #363 (Refs
#205) merged as `e9ca10ab2d28a78cb4f265cace17260679b048bc`.
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

Issues #300 and #206 remain **Open**; their Project cards must not be marked
Done. PR #362 records the funds-reservation scope, while gateway
admission/dispatch/terminal lifecycle, cancellation and partial-output
behavior, retry/crash recovery, and recharge/retention remain to be accepted.
The bounded slices #366–#371 retain their parents (#205/#214, #225, #300/#206,
#223 and #307/#217); #370 is merged as recorded above. PR #372 redacts sensitive admin video fields (#211),
but its targeted build is blocked by an unrelated existing refund-notification
compile error; PR #374 adds user refund-completion notification for #247 and
awaits protected checks. No Project schema, ruleset or upstream repository was
changed by this refresh.

## 2026-09-12 decisions

Issue labels are the source for project synchronization: `status:blocked` maps
to Blocked, `status:in-progress` maps to In progress, `status:ready` maps to
Ready, and other open issues map to Inbox. Closed work can enter Done only
after closure and acceptance evidence are checked.

Existing Status option IDs were retained. Status now contains Inbox, Ready, In
progress, In review, Blocked, Done, and Deferred. Size (`XS` through `XL`) and
Decision (Needs validation, Planned, Accepted, Deferred, Won't fix) were added.
`Type` is a GitHub-reserved content field, so it cannot be a custom field.

Area now also provides Billing, Core, Data, Security, and Operations. Content
review supplied area labels for previously unclassified issues: Billing
#206/#253; Core #207/#211/#213/#214/#215/#221/#222/#226/#241; Data #208;
Security #210/#218/#220/#255; Operations #217/#223/#224/#247; Docs #229/#235.

#1/#44-#49/#51-#53 are Deferred. Their conflicting lifecycle labels and
`agent-ready` were removed and replaced by the single label `status:blocked`.
#92, #206, #211, #225, #226, #229, and #256 are In progress. #225, #229, and
#256 are active documentation work and are not Done simply because a partial
document change exists. #200/#201/#202/#204/#111 were closed by the maintainer
after source/PR evidence and their project items are Done.

## Verification

The final refresh found all 45 open repository issues in the project. Every
open item has Status, Priority, Size, Risk, Decision, owner, and milestone; no
required field is missing. Current Status distribution is Deferred 10,
In progress 4, Blocked 1, and Inbox 30. Current Priority distribution is P0 1,
P1 21, and P2 23. Priority labels are normalized to exactly one P0/P1/P2 on
all 45 open issues.

Keep labels as the auditable state source. Do not use a stale card, partial PR,
or historical automation result as evidence that an issue is Done.

## 2026-09-13 reconciliation (2026-09-12 19:36 UTC)

The live intake contained 50 open fork issues. All 50 have a Project card and
nonempty Status, Priority, Area, Risk, Size and Decision after reconciliation.
This supersedes the earlier 45-item field-completeness statement.

Area was missing for #316, #255, #253, #247, #241, #235, #226, #224, #223,
#222, #221, #220, #218, #217, #215, #214, #213, #211, #210, #208, #207 and
#206. Each had exactly one existing `area:*` label, so the corresponding
Project option was copied without reclassifying the issue. #316 also lacked
Risk/Size/Decision; its existing historical-audit scope supports Medium/M/Planned,
recorded on the issue. It remains open pending real aggregate evidence.

#348 is a real GitHub sub-issue of #223, accepted as P1/CI/Medium/M. #343,
#229 and #348 entered implementation; their labels and issue cards were
synchronized. Reviewed PRs #349, #350, #351 and #352 are In review and use
squash auto-merge subject to CI; #347 is Done after its verified merge.

The fork enforces four strict required status checks through repository
ruleset `21984327`, not the legacy branch-protection endpoint: Rust CI / check,
Frontend CI / check, Automation Policy / gate and Dependency Audit / check.
Conversation resolution is required. The supplemental Prometheus check was
also verified before enabling #350 auto-merge. No ruleset was changed.

These are timestamped observations. Re-read remote issue/PR state before the
next transition; a field-complete board does not prove the issues are complete.

After PR #351 merged, #229 is closed and Project #1 shows Done. Its obsolete
in-progress label was removed; no status:done label exists in this repository.
The remaining 49 open issues have one lifecycle label each after removing
#348's duplicate triage label: 11 in progress, 27 triage, 11 blocked. The current
delivery intake was compared by issue number and priority against all 49 live
GitHub issues, with no missing, extra or mismatched rows.

## 2026-09-13 parallel acceptance revalidation

The current fork now has 47 open issues: 24 P1 and 23 P2, with lifecycle labels
deduplicated to 12 in progress, 24 triage, and 11 blocked after #220 entered
implementation and #348 closed. #229, #343 and #348 are
closed and their Project cards are Done; #350, #351, and #352 are merged with
the merge SHAs recorded in the delivery TODO. #349 also merged with all four
required checks and verified Redis runtime/crash-drill artifacts; its card is
Done, while parent #223 remains open. Open PR cards remain subject to
current-head checks and must not be marked Done before merge and acceptance.

The active readiness work (#356, child of #308) is accepted only when its
single-flight/cache behavior, dependency withdrawal, lifecycle transitions,
overall deadline, and real failure/recovery drill are evidenced. This is not a
deployment-only rehearsal. The dependency audit slice #357 is scoped to five
actual npm projects and three Docker build directories; an orphan root
lockfile is excluded, and #220 remains open for container permissions, remaining
image supply-chain work, the tracked dependency exception and hosted updater
evidence. #354, #355, #356 and #357 cards have Status, Priority, Area, Risk,
Size and Decision populated; #356 remains draft during final review and CI.
The repository's four required checks remain Rust CI / check, Frontend CI /
check, Automation Policy / gate, and Dependency Audit / check; no gate was
weakened or changed in this reconciliation.

## Checkpoint after PR #353 and #214 implementation

PR #353 merged as `b2d2b0fca0bb96916880f8061e5d03d2c79fa501`; its card is
Done. #214 is now In progress / Accepted, bringing the 47 open issues to
13 in progress, 23 triage and 11 blocked (24 P1 / 23 P2). Draft PR #358 carries
the bounded per-process target-capacity slice for independent review and CI;
the parent retains its lifecycle and failure/recovery acceptance. #356 is now
Ready after code review and hosted drill verification, with protected checks
required again after main synchronization. #355's hosted fixture failures were
corrected and the failed run is retained as evidence; it is not marked Done.

## 2026-09-13 live Project reconciliation (fork-only)

Remote revalidation found **45 open issues** in Project #1: 22 P1 and 23 P2;
15 In progress, 19 Triage and 11 Blocked by lifecycle labels. No open issue is
marked Done. The 18-item active delivery queue is #362, #365-#369, #371-#380,
and #382-#383. Historical checkpoint PR #364, merged PR #370 and this refresh
PR #381 are excluded from that queue count. These counts supersede earlier
47/49/50-item observations without rewriting history.

Focused cards remain linked to live state: #253 -> draft #379 (revenue-risk
audit), #255 -> #376 (Docker safety and durable-audit residuals), #225 -> #378
(tunnel audit baseline), #214 -> draft #377 (listener recovery), and #205 ->
draft #375 (key-rotation boundary, behind main). Each remains subject to the
four required checks and review; none is a terminal Done transition.

Issue #300 remains Open/P1/In progress. Quote #361 is merged, funds contracts
are in draft #362, and #368 documents the gateway lifecycle boundary, but no
current-main evidence shows reserve/dispatch/finalize calls wired through
admission, retries and sync/stream terminal paths. Keep the Project card In
progress until integration, unknown-dispatch retention, recharge recovery and
real PostgreSQL failure/recovery evidence are recorded. #206 remains open.

This is a timestamped fork-only audit. Re-read GitHub before changing cards,
labels or merge state; do not infer completion from a partial PR or green
subset of checks, and do not modify upstream.
