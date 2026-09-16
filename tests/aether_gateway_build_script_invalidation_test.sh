#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_root="${AGENT_TMP_DIR:-${TMPDIR:-/tmp}}/aether-build-script-invalidation.$RANDOM"
mkdir -p "$tmp_root"
trap 'rm -rf "$tmp_root"' EXIT

source_repo="$tmp_root/source"
linked_repo="$tmp_root/linked"
mkdir -p "$source_repo/src"
cp "$repo_root/apps/aether-gateway/build.rs" "$source_repo/build.rs"
cat > "$source_repo/Cargo.toml" <<'EOF'
[package]
name = "aether-build-script-fixture"
version = "0.1.0"
edition = "2021"
build = "build.rs"
EOF
cat > "$source_repo/src/main.rs" <<'EOF'
fn main() {
    println!(
        "{}|{}",
        option_env!("AETHER_BUILD_VERSION").unwrap_or("missing"),
        option_env!("AETHER_BUILD_TYPE").unwrap_or("missing")
    );
}
EOF

git -C "$source_repo" init -q
git -C "$source_repo" config user.email fixture@example.invalid
git -C "$source_repo" config user.name fixture
git -C "$source_repo" add .
git -C "$source_repo" commit -qm initial
git -C "$source_repo" tag v1.2.3
git -C "$source_repo" worktree add -qb linked "$linked_repo" HEAD >/dev/null

linked_head_path="$(git -C "$linked_repo" rev-parse --git-path HEAD)"
[[ -f "$linked_head_path" ]] || {
    printf 'linked worktree HEAD path is missing: %s\n' "$linked_head_path" >&2
    exit 1
}

linked_target="$tmp_root/target-linked"
first_log="$tmp_root/linked-first.log"
second_log="$tmp_root/linked-second.log"
third_log="$tmp_root/linked-third.log"
(
    cd "$linked_repo"
    CARGO_TARGET_DIR="$linked_target" cargo build -vv
) >"$first_log" 2>&1
(
    cd "$linked_repo"
    CARGO_TARGET_DIR="$linked_target" cargo build -vv
) >"$second_log" 2>&1
grep -q 'Fresh aether-build-script-fixture v0.1.0' "$second_log"
! grep -q 'Running .*build-script-build' "$second_log"

run_value() {
    local checkout="$1"
    local target="$2"
    shift 2
    (
        cd "$checkout"
        env "$@" CARGO_TARGET_DIR="$target" cargo run -q
    )
}

linked_version="$(run_value "$linked_repo" "$linked_target" AETHER_BUILD_VERSION= AETHER_VERSION= GITHUB_REF_NAME= AETHER_BUILD_TYPE=source)"
[[ "$linked_version" == '1.2.3|source' ]] || {
    printf 'unexpected linked git version: %s\n' "$linked_version" >&2
    exit 1
}

git -C "$linked_repo" commit --allow-empty -qm change
git -C "$linked_repo" checkout --detach -q HEAD
(
    cd "$linked_repo"
    CARGO_TARGET_DIR="$linked_target" cargo build -vv
) >"$third_log" 2>&1
grep -q 'Running .*build-script-build' "$third_log"

explicit_version="$(run_value "$linked_repo" "$linked_target" AETHER_BUILD_VERSION=v9.8.7 AETHER_VERSION=v2.0.0 GITHUB_REF_NAME=v1.0.0 AETHER_BUILD_TYPE=release)"
[[ "$explicit_version" == '9.8.7|release' ]] || {
    printf 'explicit build version precedence failed: %s\n' "$explicit_version" >&2
    exit 1
}

fallback_version="$(run_value "$linked_repo" "$linked_target" AETHER_BUILD_VERSION=tunnel-v99 AETHER_VERSION=v2.0.0 GITHUB_REF_NAME=v1.0.0 AETHER_BUILD_TYPE=source)"
[[ "$fallback_version" == '2.0.0|source' ]] || {
    printf 'tunnel tag fallback failed: %s\n' "$fallback_version" >&2
    exit 1
}

ref_version="$(run_value "$linked_repo" "$linked_target" AETHER_BUILD_VERSION=tunnel-v99 AETHER_VERSION= GITHUB_REF_NAME=v8.0.0 AETHER_BUILD_TYPE=source)"
[[ "$ref_version" == '8.0.0|source' ]] || {
    printf 'GitHub ref fallback failed: %s\n' "$ref_version" >&2
    exit 1
}

regular_target="$tmp_root/target-regular"
regular_version="$(run_value "$source_repo" "$regular_target" AETHER_BUILD_VERSION= AETHER_VERSION= GITHUB_REF_NAME= AETHER_BUILD_TYPE=source)"
[[ "$regular_version" == "$linked_version" ]] || {
    printf 'ordinary checkout version differs: %s\n' "$regular_version" >&2
    exit 1
}

printf 'aether gateway build script invalidation fixture passed\n'
