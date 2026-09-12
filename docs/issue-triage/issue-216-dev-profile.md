# Issue #216: Development build fan-out

## Decision

The first low-risk implementation slice for Issue #216 is a Cargo development
profile setting for workspace dependencies:

```toml
[profile.dev.package."*"]
opt-level = 1
```

This keeps the existing `line-tables-only` debug policy for local workspace
crates, while compiling third-party and workspace dependencies with the small
optimization level recommended by Cargo for faster edit/compile/test loops.
It does not change release artifacts, runtime behavior, or the pinned toolchain.

## Why this slice

The issue identifies a large gateway test/build graph and dependency fan-out as
a local iteration cost. The profile change is bounded to development builds,
does not require a build-script redesign, and has no deployment or migration
impact. Measuring a before/after wall-clock baseline remains a follow-up because
the result depends on the local compiler cache and machine; this PR records the
configuration guard so the intended policy cannot silently regress.

## Verification and acceptance

- `cargo test -p aether-gateway workspace_dev_profile_optimizes_dependencies --lib`
  parses the workspace manifest section and requires `opt-level = 1`.
- `cargo fmt --all -- --check`
- `git diff --check`

This is a child slice only. Live database test wiring, VSCodex required-check
policy, build-script invalidation measurement, and the shared backend contract
suite remain separate Issue #216 work packages.
