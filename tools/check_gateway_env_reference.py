#!/usr/bin/env python3
"""Check and (optionally) regenerate the gateway environment reference.

The gateway has both clap-backed settings and runtime-only environment reads.
Keeping the source scan here makes additions fail loudly instead of silently
drifting from the operator-facing reference.
"""
from __future__ import annotations

import argparse
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "apps" / "aether-gateway" / "src"
DOC = ROOT / "docs" / "operations" / "gateway-environment-reference.md"

NAME = r"[A-Z][A-Z0-9_]+"


def source_files() -> list[Path]:
    return [p for p in SRC.rglob("*.rs") if "/tests/" not in p.as_posix()]


def collect() -> tuple[set[str], set[str]]:
    clap: set[str] = set()
    runtime: set[str] = set()
    for path in source_files():
        text = path.read_text(encoding="utf-8")
        clap.update(re.findall(r'env\s*=\s*"(' + NAME + r')"', text))
        runtime.update(
            re.findall(r'(?:env::var|var_os|env_var_trimmed)\s*\(\s*"(' + NAME + r')"', text)
        )
        runtime.update(
            re.findall(r'const\s+\w+_ENV\w*\s*:\s*&str\s*=\s*"(' + NAME + r')"', text)
        )
    return clap, runtime - clap


def default_for(name: str) -> str:
    text = (SRC / "main.rs").read_text(encoding="utf-8")
    # Keep the attribute expression intact: expressions such as 1024 * 1024
    # are more useful to operators than an accidentally evaluated value.
    match = re.search(
        r"\#\[arg\((?:(?!\#\[arg).)*?env\s*=\s*\""
        + re.escape(name)
        + r"\"(?:(?!\#\[arg).)*?\)\]",
        text,
        re.S,
    )
    if not match:
        return "runtime (source-defined)"
    attr = match.group(0)
    value = re.search(r"default_value(?:_t)?\s*=\s*([^,\)\n]+)", attr)
    return value.group(1).strip().strip('"') if value else "unset"


def unit(name: str) -> str:
    if name.endswith(("_MS", "_MILLISECONDS")):
        return "milliseconds"
    if name.endswith(("_SECS", "_SECONDS")):
        return "seconds"
    if name.endswith("_BYTES") or name.endswith("_MB"):
        return "bytes / MiB (source-defined)"
    if any(x in name for x in ("ENABLED", "ALLOW_", "SECURE", "FAIL_OPEN", "REQUIRE_SSL", "PERSISTENCE")):
        return "boolean (true/false)"
    if name.endswith(("_URL", "_DIR", "_PATH", "_PREFIX", "_KEY", "_SECRET")):
        return "string"
    return "count or enum (see source)"


def render(clap: set[str], runtime: set[str]) -> str:
    rows = []
    for name in sorted(clap | runtime):
        kind = "clap (global; subcommands inherit)" if name in clap else "runtime-only"
        rows.append(f"| `{name}` | {kind} | `{default_for(name) if name in clap else 'unset'}` | {unit(name)} |")
    return """# Aether gateway environment reference

This file is generated from `apps/aether-gateway/src`. Run
`python3 tools/check_gateway_env_reference.py --write` after changing an
environment variable. The checker fails when a source variable is missing from
this table or when the table contains a stale variable.

Clap-backed variables are available as command-line options on the default
server invocation and on `data export|import|copy|db ...` subcommands because
the data argument group is global. Hidden maintenance flags (`--migrate` and
`--apply-backfills`) are command-line-only and have no environment variable.

`unset` means the value is optional and the runtime default or auto-sizing
logic applies. Boolean values use clap's normal `true`/`false` parsing; flags
with `default_missing_value` can also be enabled by setting the variable to an
empty value only when explicitly documented in source. Secret values are
never printed by the checker.

| Variable | Source | Default | Unit / value |
| --- | --- | --- | --- |
""" + "\n".join(rows) + "\n"


def documented_names() -> set[str]:
    if not DOC.exists():
        return set()
    return set(
        re.findall(
            r"\|\s*`(" + NAME + r")`\s*\|\s*(?:clap|runtime)",
            DOC.read_text(encoding="utf-8"),
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    clap, runtime = collect()
    expected = clap | runtime
    if args.write:
        DOC.parent.mkdir(parents=True, exist_ok=True)
        DOC.write_text(render(clap, runtime), encoding="utf-8")
        return 0
    actual = documented_names()
    missing, stale = sorted(expected - actual), sorted(actual - expected)
    if missing or stale:
        if missing:
            print("missing from gateway environment reference:", " ".join(missing))
        if stale:
            print("stale in gateway environment reference:", " ".join(stale))
        print("run: python3 tools/check_gateway_env_reference.py --write")
        return 1
    print(f"gateway environment reference is current ({len(expected)} variables)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
