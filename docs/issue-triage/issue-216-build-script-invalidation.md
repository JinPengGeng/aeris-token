# Issue 216: linked-worktree build-script invalidation

## Bounded slice

`apps/aether-gateway/build.rs` used the fixed path `../../.git/HEAD` as its
Cargo invalidation input. That path exists in a normal checkout, but a linked
worktree stores its administrative files under the common repository's
`.git/worktrees/<name>/` directory and exposes `.git` as a file. Cargo could
therefore miss the real HEAD file and repeatedly rerun the gateway build
script in linked worktrees.

The build script now asks Git for `git rev-parse --git-path HEAD`, resolves a
relative result from `CARGO_MANIFEST_DIR`, canonicalizes the existing file, and
emits that absolute path as `cargo:rerun-if-changed`. If Git metadata is not
available, it emits no invalid path and keeps the existing environment
variable listeners. Version precedence and the `git describe --tags
--match v[0-9]* --always --dirty` invocation are unchanged.

## Acceptance

`tests/aether_gateway_build_script_invalidation_test.sh` creates a small Git
repository and a linked worktree, then verifies:

* the second unchanged Cargo build is `Fresh` and does not rerun
  `build-script-build`;
* an empty commit followed by `checkout --detach HEAD` changes the watched
  HEAD file, reruns the build script, and reports the new Git version; unchanged
  builds before and after that operation stay `Fresh`;
* explicit build-version, `AETHER_VERSION`, GitHub ref, and tunnel-tag
  fallback precedence remains intact;
* ordinary and linked checkouts report the same tagged version;
* paths containing spaces work and a source archive without Git metadata
  falls back to the package version without emitting a nonexistent watcher.

The fixture runs in the Rust CI shell-security job and is asserted by the
Rust-CI automation contract. The job explicitly installs Rust 1.95.0; this
fixture requires real Cargo and Git, uses one build job, and disables the Rust
compiler wrapper. All baseline and HEAD-change Cargo calls use identical
explicit build-version and build-type variables, so an environment change
cannot satisfy the HEAD-invalidation assertion. It uses a temporary target
directory and removes the repository and build artifacts on exit.

## Remaining boundary

This change intentionally does not watch branch refs, tag refs, `packed-refs`,
or the whole worktree. A same-branch commit, tag mutation, or dirty-tree
transition may therefore retain the existing Cargo cache semantics. Making
the version always equal to the current dirty `git describe` result requires a
separate version-freshness decision because those watches can recompile the
large gateway graph.
