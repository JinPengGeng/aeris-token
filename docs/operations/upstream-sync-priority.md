# Daily upstream integration

The execution priority selected on 2026-09-18 is to check upstream daily and
integrate available updates before starting further issue implementation.

The existing `Sync upstream (minimal)` GitHub Actions workflow checks
`fawney19/Aether:main` against this fork's `main`. Its schedule is `17 3 * * *`
(11:17 Asia/Shanghai); GitHub may start a scheduled run later. The workflow
must remain active with `AERIS_UPSTREAM_SYNC_ENABLED=true`.

On each check:

1. Compare the actual upstream and fork commit ancestry. When upstream is
   already an ancestor of fork main, finish without a new synchronization PR.
2. For new commits, prioritize the synchronization PR and required checks.
   Preserve fork-owned workflows, dependency decisions and financial/security
   behavior while incorporating upstream changes.
3. If the automatic merge encounters conflicts, handle its alert before new
   issue slices. Resolve in an isolated worktree based on current fork main;
   preserve unfinished work in other checkouts. Validate both conflict
   resolutions and affected automatically merged code.
4. Merge with a true merge commit after the required checks pass. Never squash
   or rebase upstream synchronization. Re-read main and verify that the exact
   upstream tip is an ancestor; then rerun synchronization to check convergence.
5. Resume the remaining issue queue and integrate its local changes against
   the updated main. Report local validation, pushed changes, merged history
   and deployment status separately.

The schedule is hosted on GitHub and does not depend on a local desktop app
remaining open. A failed/conflicted run is pending integration work, not a
successful synchronization.
