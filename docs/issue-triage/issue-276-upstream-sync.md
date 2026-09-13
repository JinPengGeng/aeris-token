# Issue #276: resolve the pinned upstream synchronization conflict

Date: 2026-09-13

## Decision and scope

The reported conflict is reproducible against fork main
`8fb31ea620c3cc98ff06fb2ca148dd6da8429b96`. Merge the exact upstream revision
`60b89cc840d6d99972c15423c7655335c011c7ae` named in the issue. This brings four
upstream commits covering PostgreSQL recharge callbacks and DeepSeek reasoning
compatibility. Only the fork receives commits, PRs and issue updates; fetching
upstream objects is read-only.

Priority remains P1. The change unblocks scheduled synchronization and fixes
payment callbacks attempting `FOR UPDATE` on the nullable side of a LEFT JOIN.
The production changes are bounded to six existing/new upstream paths, but the
payment and provider behavior require actual database and planner verification.

## Conflict resolution

The only textual conflict is `WalletCenter.vue::refreshWallet`. Preserve both
behaviors: first finish the upstream order refresh so an older order snapshot
cannot overwrite the later wallet balance; then refresh wallet balance,
transactions and the fork's entitlements together. Keep the existing fork daily
quota reset UI and its entitlement refresh.

The imported recharge component tests initially failed because the fork also
loads billing entitlements during mount. Mock that real dependency, scope order
controls to their tab, and add a deferred-order regression that checks the final
wallet/package balances and the retained entitlement refresh. Do not remove the
fork feature to satisfy the upstream fixture.

## Verification and acceptance

- Wallet recharge component suite: 7 tests passed, including the combined
  conflict-resolution regression, pending/credited transitions, hidden-page
  polling, pagination, transient failure and unmount cleanup.
- Frontend TypeScript check and ESLint on the three changed frontend files pass.
- PostgreSQL 17.11: bootstrap migration passes; all 3 payment callback tests
  execute and pass, covering user/key wallets, duplicate callbacks and wrong or
  missing owners. The upstream callback job remains in required Rust CI.
- DeepSeek planner regression tests and the four required GitHub checks must
  pass on the delivered revision before acceptance.

Use a merge commit for this synchronization PR, not squash/rebase: the pinned
upstream commit must become an ancestor of fork main so later sync attempts
recognize it as integrated. The repository currently permits merge commits and
the main ruleset still requires all four checks and resolved review threads.
Close #276 only after merge and remote ancestry verification. This resolves the
reported revision; it does not claim synchronization with future upstream heads.

If a runtime regression requires rollback, use a reviewed revert of the merge
with main as the retained parent. Coordinate the next upstream sync explicitly;
do not rewrite either repository's history or retry an automatic sync blindly.
