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
| #367 | [34729324909 / 103649090569](https://github.com/JinPengGeng/aeris-token/actions/runs/34729324909/job/103649090569) | Two documents plus a Python test/tool, `rust=false`, `data=false` under the previous filters |
| #368 | [34729326268 / 103649093494](https://github.com/JinPengGeng/aeris-token/actions/runs/34729326268/job/103649093494) | One Markdown file, `rust=false`, `data=false` |
| #381 | [34734898659 / 103664442747](https://github.com/JinPengGeng/aeris-token/actions/runs/34734898659/job/103664442747) | Three documentation files, `rust=false`, `data=false` |

This is an accepted CI-cost and queue-latency regression fix under #216. It
restores the documented path selection policy with one conservative `rust`
scope for all Rust and database jobs. A separate data-only optimization is
deferred because callers and shared contracts cross crate boundaries.
Under the expanded input coverage below, #367's root test change would now
intentionally select Rust; the ordinary Markdown changes in #368/#381 would
still skip it.

## Implementation contract

- Ordinary documentation and unrelated frontend package changes skip all 12
  Rust/database leaf jobs.
  Shell security fixtures still run for every PR.
- Rust sources, Cargo/toolchain inputs, `tools/ci/**`, `tests/**`, the filter
  itself, and SQL/API fixtures outside the workspace trigger the entire Rust/DB
  graph. `docs/api/**` includes Markdown contracts read by Rust `include_str!`,
  so those files cannot be treated as ordinary documentation. Main-branch push
  routing covers these same inputs.
- Gateway architecture tests also read `Dockerfile.app.local`, `deploy.sh`,
  `frontend/vite.config.ts` and scan `frontend/src/**` for retired API aliases.
  Independent review caught these extra inputs; they now select Rust on PR and
  push. Frontend source changes therefore still require Rust verification.
- Each aggregate always executes. Successful detection with canonical `true`
  requires every selected leaf to succeed; canonical `false` requires every
  leaf to be skipped. Failed/cancelled detection, missing or unknown output,
  unexpected execution, unexpected skips, and failed/cancelled leaves fail.
- The final `Rust CI / check` also requires shell fixtures and all internal
  aggregates to succeed. A skipped aggregate never counts as passing.
- Push and manual dispatch keep full execution. Reusable workflows inherit
  their caller's event name, including `pull_request`; review corrected the
  initial mistaken test of an event named `workflow_call`. The reusable entry
  now defaults its boolean `force_full` input to true, independently of caller
  event or path-filter output. A caller may explicitly opt into normal path
  selection with false. Direct PR events have no such input and preserve empty
  detector output so the gates can reject it.
- All existing test commands, live database services, feature/package matrices,
  immutable action references, and required check names remain intact. Branch
  protection and the other three required contexts are unchanged.

## Verification and remaining acceptance

The new automation tests parse the real workflow YAML, evaluate its restricted
string/boolean output and leaf conditions, and execute the actual inline Bash
gate scripts with synthetic dependency results. They cover the job graph,
documentation/frontend package selection, Rust/DB/fixture and cross-module
inputs, non-PR full runs and reusable calls from a PR event,
detector failures/cancellation/unknown outputs, and each leaf/aggregate failure
or unexpected skip. Environment injection is exercised by rendering the
workflow's actual `env` expressions rather than duplicating the gate logic.

Local verification:

- `npm test` in `.github/automation`: 218 tests passed, none skipped.
- Focused change-filter and selection suite: 17 tests passed.
- `actionlint` 1.7.12 (including ShellCheck 0.11.0): passed. Installed the
  missing local validator; replaced two redundant `echo` wrappers around
  `pg_config --bindir` without changing the configured PostgreSQL path.
- `git diff --check`: passed.

The expression harness models only the expressions used here; it does not
simulate GitHub's scheduler or run the full Rust suite locally. Before merge,
the PR must pass the existing required checks and independent review. After
merge, a real ordinary documentation PR must show skipped Rust/DB leaves with
successful shell fixtures and all four Rust aggregate gates. Workflow changes
on this PR intentionally select a full Rust run. Issue #216 remains open for
its broader test/build work packages.
