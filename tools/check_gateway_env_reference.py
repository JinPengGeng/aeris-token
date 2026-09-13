#!/usr/bin/env python3
"""Check the complete gateway environment source reference without reading env values.

Supports literal env names on field-based clap attributes and the gateway's
env readers/constants; not a Rust parser or an inventory of dependencies.
"""
from __future__ import annotations

import argparse
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "apps" / "aether-gateway" / "src"
DOC = ROOT / "docs" / "operations" / "gateway-environment-reference.md"
NAME = r"[A-Z][A-Z0-9_]+"
ARG = re.compile(r"#\[arg\((.*?)\)\]", re.S)
FIELD = re.compile(r"(?:\s*///[^\n]*)*\s*(?:pub(?:\([^)]*\))?\s+)?\w+\s*:\s*([^,\n]+)")
READ = re.compile(
    r"(?:env::var(?:_os)?|var_os|env_[a-z0-9_]+|[a-z0-9_]+_from_env)"
    r"\s*\(\s*\"(" + NAME + r")\""
)
CONSTANT = re.compile(r'const\s+\w*_ENV\w*\s*:\s*&str\s*=\s*"(' + NAME + r')"')


def attribute_values(attribute: str) -> dict[str, str]:
    """Split supported attribute syntax, preserving nested default expressions."""
    pieces, start, depth, quoted, escaped = [], 0, 0, False, False
    for index, char in enumerate(attribute):
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        elif char == "," and depth == 0:
            pieces.append(attribute[start:index].strip())
            start = index + 1
    if depth != 0 or quoted:
        raise ValueError("unsupported or unbalanced clap attribute")
    pieces.append(attribute[start:].strip())
    return dict((key.strip(), value.strip()) for piece in pieces
                if "=" in piece for key, value in [piece.split("=", 1)])


def source_files(source: Path) -> list[Path]:
    return sorted(path for path in source.rglob("*.rs")
                  if "tests" not in path.relative_to(source).parts)


def collect(source: Path = SRC) -> tuple[dict[str, dict[str, str]], dict[str, list[str]]]:
    clap, runtime = {}, {}
    for path in source_files(source):
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(source).as_posix()
        for match in ARG.finditer(text):
            values = attribute_values(match.group(1))
            if "env" not in values:
                continue
            name_match = re.fullmatch(r'"(' + NAME + r')"', values["env"])
            field = FIELD.match(text, match.end())
            if name_match is None or field is None:
                line = text.count("\n", 0, match.start()) + 1
                raise ValueError(f"unsupported clap env declaration in {relative}:{line}; extend the checker explicitly")
            name, rust_type = name_match.group(1), field.group(1).strip()
            if name in clap:
                raise ValueError(f"duplicate clap env declaration: {name}; document binary-specific behavior explicitly")
            default = values.get("default_value", values.get("default_value_t"))
            if default is None:
                default = "unset" if rust_type.startswith("Option<") else (
                    "false (clap flag)" if rust_type == "bool" else "no declared default"
                )
            scope = (f"standalone {path.stem} CLI" if relative.startswith("bin/") else
                     "gateway global; inherited by data subcommands" if values.get("global") == "true" else
                     "gateway root/server; not inherited by subcommands")
            clap[name] = {
                "scope": scope, "default": default, "type": rust_type,
                "source": relative, "attribute": " ".join(match.group(1).split()),
            }
        for name in sorted(set(READ.findall(text)) | set(CONSTANT.findall(text))):
            runtime.setdefault(name, []).append(relative)
    return clap, {name: paths for name, paths in runtime.items() if name not in clap}


def cell(value: str) -> str:
    return value.replace("|", "&#124;").replace("`", "&#96;")


def source_link(path: str) -> str:
    return f"[{path}](../../apps/aether-gateway/src/{path})"


def render(clap: dict[str, dict[str, str]], runtime: dict[str, list[str]]) -> str:
    lines = ["""# Aether gateway environment reference

This source reference covers literal clap declarations and recognized runtime
environment readers under `apps/aether-gateway/src`; it does not enumerate
environment settings in dependencies or deployment scripts. It never reads or
prints configured environment values.

Regenerate with `python3 tools/check_gateway_env_reference.py --write`.
The check compares the complete generated document, including declared
defaults, Rust types, scope and clap validation attributes. Unsupported clap
env declarations fail instead of being silently omitted. Required Rust CI runs
the checker and its regression tests.

Only arguments explicitly marked global are inherited by `data` subcommands.
Root/server arguments are not inherited; the standalone backup-restore binary
has its own arguments. `unset` denotes an `Option` without a declared default;
it does not mean zero or disabled. Rust default expressions below are preserved
as source expressions, not evaluated by this tool. Empty environment values
are not a general way to enable boolean options.

## Clap declarations

| Variable | Scope | Declared default | Rust type | Source |
| --- | --- | --- | --- | --- |"""]
    for name, entry in sorted(clap.items()):
        lines.append(f"| `{name}` | {entry['scope']} | `{cell(entry['default'])}` | `{cell(entry['type'])}` | {source_link(entry['source'])} |")
    lines += ["", "### Declared parsing and validation", "",
              "These declarations preserve value parsers and missing-value handling for review.",
              "They contain names and source defaults, not current environment values.", ""]
    for name, entry in sorted(clap.items()):
        lines.append(f"- `{name}`: `{cell(entry['attribute'])}`")
    lines += ["", "## Runtime reader inventory", "",
              "These settings are not clap options. Defaults, parsing and supported aliases are",
              "defined by the linked readers; the inventory does not guess types or defaults",
              "from variable names. Request candidate persistence is a mode (`full`, `terminal`,",
              "`none`), not a boolean. See its reader for compatibility aliases.", "",
              "| Variable | Reader source |", "| --- | --- |"]
    for name, paths in sorted(runtime.items()):
        lines.append(f"| `{name}` | {', '.join(source_link(path) for path in sorted(set(paths)))} |")
    return "\n".join(lines) + "\n"


def check_document(document: str, source: Path = SRC) -> bool:
    return document == render(*collect(source))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    expected = render(*collect())
    if args.write:
        DOC.write_text(expected, encoding="utf-8")
        return 0
    if not DOC.exists() or DOC.read_text(encoding="utf-8") != expected:
        print("gateway environment reference has drifted; run the documented regeneration command")
        return 1
    print("gateway environment reference matches declarations and reader inventory")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
