# Historical closures and tracker audit

Audited 2026-09-12 against `origin/main` at `12a1d265c090f2666e35ddbe7f13f5b842cf5ff5`. This is a read-only GitHub and ancestry audit of the fork `JinPengGeng/aeris-token`; it does not make a claim about the current upstream tip.

## Scope and method

The repository currently has 115 issues: 65 closed and 50 open. Closure is classified from GitHub's issue-to-PR relationship, the closing comment, and `git merge-base --is-ancestor <commit> origin/main` after a read-only `git fetch origin --prune`.

An issue is only treated as integrated when the linked merge commit is an `origin/main` ancestor. A newer unrelated PR, a successful workflow, or a bot comment alone is not treated as proof.

## Closed-issue disposition

| Disposition | Count | Evidence and result |
| --- | ---: | --- |
| Exact duplicate of a still-open canonical issue | 25 | Five batches of five are closed duplicates: `#230-#234 -> #229`, `#236-#240 -> #235`, `#242-#246 -> #241`, `#248-#252 -> #247`, and `#257-#261 -> #256`. Do not reopen or reimplement these copies. |
| Directly closed by a merged PR reachable from `origin/main` | 14 | `#112 -> PR #127`, `#113 -> #121`, `#114 -> #144`, `#115 -> #155`, `#116 -> #150`, `#117 -> #151`, `#122 -> #135`, `#125 -> #152`, `#142 -> #171`, `#153 -> #160`, `#154 -> #156`, `#174 -> #278`, `#175 -> #180`, and `#181 -> #194`. GitHub reports every PR as `MERGED`; all 14 merge commits are `origin/main` ancestors. |
| Sync/workflow alert no longer active | 17 | Eleven alert source SHAs are direct `origin/main` ancestors: `#15`, `#137`, `#172`, `#183`, `#185`, `#190`, `#191`, `#219`, `#262`, `#265`, and `#266`. `#271` is a bounded-fetch failure fixed by merged `PR #272` (`158b9608f`) and a succeeding no-op sync run. `#28` records manual workflow review resolved by merged `PR #32`. `#140` was consciously declined because its upstream workflow policy does not apply to this fork. The remaining historical workflow-review records `#133/#148` and failed-check alert `#193` have closure comments but their old source objects are no longer locally retained; preserve them as historical evidence rather than using them as an ancestry proof. |
| Explicitly superseded, completed by recorded commit, research conclusion, or informational close | 9 | `#11` is superseded by `#104`, then `#179`; `#50` was integrated by `PR #105` (`c9026d881`); `#68` by `PR #72` (`45b362c6d`); `#104` moved the v2 work to `#179`; `#108` by `PR #120` (`a06510f08`); `#109` by `PR #119` (`31bcd39ff`); `#118` concluded equivalent non-2xx semantics already exist; `#143` closed after its review items landed; and `#169` is an answered informational question. |

This accounts for all 65 closed issues. The 14 direct PR closures and 11 SHA-proven closed sync alerts have the strongest integration evidence. The remaining closed records should stay closed unless their specific behavior regresses; none is a generic template for closing a new issue.

## Open sync alerts

| Issue | Alert source | Current evidence | Disposition |
| --- | --- | --- | --- |
| #200 | `882bb43125745dcb8b88281776b0fc017eed795e` | Direct ancestor of `origin/main`. | Stale alert; eligible to close with the SHA ancestry evidence. |
| #201 | `dba5e6e9e99efe869a625851360986c979af37a5` | Direct ancestor of `origin/main`. | Stale alert; eligible to close. |
| #202 | `a5c3699ae9e1dc4e2eadd611f2a3a2e72e99cac9` | Direct ancestor of `origin/main`. | Stale alert; eligible to close. |
| #204 | Alert was caused by PR #203 occupying `sync/upstream`; its captured upstream tip `7aa0c892443e377b4b1cbe7fc3cc41f37d3af584` is an `origin/main` ancestor. PR #203 is now `MERGED` at `275f55418063`. | The reserved-branch state is historical. | Eligible to close. |
| #276 | `60b89cc840d6d99972c15423c7655335c011c7ae` | GitHub cross-fork compare reports divergence: fork `main` is 4 commits ahead and 180 commits behind this upstream point, with merge base `531f53b4437c`. The conflict remains at `frontend/src/views/user/WalletCenter.vue`. | Real outstanding synchronization conflict; keep open and handle by the declared manual merge-commit workflow. |

`#268` is not a sync alert. It is an unprioritized scheduler telemetry defect: `Skipped(auth_api_key_concurrency_limit_reached)` is terminal before a later execution succeeds, leaving successful traffic recorded as skipped. Its behavior must be decided with #49 rather than closed as stale automation noise.

## Open trackers and roadmaps

| Issue | Current state | Audit conclusion |
| --- | --- | --- |
| #111 | Its last maintainer checkpoint says all original child intake work is complete, with #142 delivered by `PR #171` (`52831c8bf`). It retains upstream observation and parked scheduler notes. | The implementation-tracking purpose is complete. It is eligible to close as a completed umbrella after moving any ongoing upstream monitoring links to #157/#158 and patch-policy documentation. |
| #157 | Intake record for seven upstream billing/quota PRs. Current upstream states: #732 merged 2026-09-05; #750 closed unmerged; #727/#714/#634/#607/#638 remain open. Fork-local replacement of the #727 area shipped in `PR #278` for closed #174. | Keep open only as an evaluation registry, but update its stale seven-open-PR framing. It is not a product implementation issue and should remain out of the delivery queue. |
| #158 | Intake record for upstream `PR #736`; upstream still reports it as open, last updated 2026-08-18. | Keep open as deferred evaluation. No fork implementation or closure evidence exists. |
| #179 | Automation v2 roadmap. Its linked Phase 1/2/3 PRs #195/#197, #196, #198, and #199 are merged and reachable from `origin/main`; #184 is also merged. Phase 0 was explicitly scheduled after the upstream billing group and has no completion proof. | Keep open. It is a roadmap, not proof that all migration/revocation work is done. |

## Recommended remote state changes

No remote mutations were made by this audit. The evidence supports closing only `#200`, `#201`, `#202`, and `#204` now. `#111` is a reasonable fifth candidate, but its closure should first preserve pointers to the ongoing #157/#158 evaluation records and the patch-policy tracker.

Do not close `#276`, `#157`, `#158`, `#179`, or `#268`. They each retain a concrete unresolved conflict, evaluation decision, roadmap phase, or behavior decision.
