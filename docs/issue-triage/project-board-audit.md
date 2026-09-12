# Project board audit

Scope: GitHub Project v2 `JinPengGeng/aeris-token Development` (project 1,
`PVT_kwHOAiijc84BgApR`), with no upstream changes.

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
