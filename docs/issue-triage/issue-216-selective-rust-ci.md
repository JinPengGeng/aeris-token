# Issue #216: Restore selective Rust CI execution

## Finding and decision

The shared change detector correctly classified documentation PRs, but the Rust
and database execution jobs no longer consumed its result. Upstream sync commit
`275f5541806377af4700643e6b7fd3f49f60e524` removed their `needs: changes` and job
conditions. The existing aggregates also required unconditional success, so
restoring only the leaf conditions would make an intentional skip fail CI.

Observed examples:

| PR | Rust CI run / detection job | Detector result |
| --- | --- | --- |
| #367 | [34729324909 / 103649090569](https://github.com/JinPengGeng/aeris-token/actions/runs/34729324909/job/103649090569) | Four documentation files, `rust=false`, `data=false` |
| #368 | [34729326268 / 103649093494](https://github.com/JinPengGeng/aeris-token/actions/runs/34729326268/job/103649093494) | One Markdown file, `rust=false`, `data=false` |
| #381 | [34734898659 / 103664442747](https://github.com/JinPengGeng/aeris-token/actions/runs/34734898659/job/103664442747) | Three documentation files, `rust=false`, `data=false` |

This is an accepted CI-cost and queue-latency regression fix under #216. It
restores the documented path selection policy with one conservative `rust`
scope for all Rust and database jobs. A separate data-only optimization is
deferred because callers and shared contracts cross crate boundaries.

## Implementation contract

- Ordinary documentation and frontend PRs skip all 12 Rust/database leaf jobs.
  Shell security fixtures still run for every PR.
- Rust sources, Cargo/toolchain inputs, `tools/ci/**`, `tests/**`, the filter
  itself, and SQL/API fixtures outside the workspace trigger the entire Rust/DB
  graph. `docs/api/**` includes Markdown contracts read by Rust `include_str!`,
  so those files cannot be treated as ordinary documentation. Main-branch push
  routing covers these same inputs.
- Each aggregate always executes. Successful detection with canonical `true`
  requires every selected leaf to succeed; canonical `false` requires every
  leaf to be skipped. Failed/cancelled detection, missing or unknown output,
  unexpected execution, unexpected skips, and failed/cancelled leaves fail.
- The final `Rust CI / check` also requires shell fixtures and all internal
  aggregates to succeed. A skipped aggregate never counts as passing.
- Push, manual dispatch and reusable workflow calls keep full execution. Only
  non-PR events supply the full-scope default; empty PR detector output is
  preserved so the gates can reject it.
- All existing test commands, live database services, feature/package matrices,
  immutable action references, and required check names remain intact. Branch
  protection and the other three required contexts are unchanged.

## Verification and remaining acceptance

The new automation tests parse the real workflow YAML, evaluate its restricted
string/boolean output and leaf conditions, and execute the actual inline Bash
gate scripts with synthetic dependency results. They cover the job graph,
documentation/frontend selection, 17 Rust/DB/fixture inputs, non-PR full runs,
detector failures/cancellation/unknown outputs, and each leaf/aggregate failure
or unexpected skip. Environment injection is exercised by rendering the
workflow's actual `env` expressions rather than duplicating the gate logic.

Local verification:

- `npm test` in `.github/automation`: 217 tests passed, none skipped.
- Focused change-filter, selection and action-pinning suite: 17 tests passed.
- `git diff --check`: passed.

The expression harness models only the expressions used here; it does not
simulate GitHub's scheduler or run the full Rust suite locally. Before merge,
the PR must pass the existing required checks and independent review. After
merge, a real ordinary documentation PR must show skipped Rust/DB leaves with
successful shell fixtures and all four Rust aggregate gates. Workflow changes
on this PR intentionally select a full Rust run. Issue #216 remains open for
its broader test/build work packages.
