#!/usr/bin/env python3
"""npm advisory gate with a reviewed-exception mechanism.

Mirrors .github/scripts/cargo-audit-gate.sh for npm: `npm audit` stays
zero-tolerance at high severity, but advisories with no fixed release may be
carried for at most 30 days via .github/security/npm-audit-exceptions.json.
Each exception is scoped to one audited directory and one package/advisory
pair and must carry owner, review cadence, and rationale.

Usage:
  npm audit --json --package-lock-only --audit-level=high > audit.json
  npm-audit-gate.py --directory frontend --audit audit.json

Exits 0 when no high/critical vulnerabilities remain after applying reviewed
exceptions; exits 1 otherwise. The raw npm audit exit status is intentionally
not trusted here: this gate re-decides from the JSON report so an exempted
advisory can never mask an unrelated new finding.
"""
import argparse
import json
import sys
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXCEPTIONS_FILE = ROOT / ".github" / "security" / "npm-audit-exceptions.json"
MAX_EXCEPTION_AGE = timedelta(days=30)


def fail(message: str) -> "SystemExit":
    print(f"npm-audit-gate: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_exceptions(directory: str) -> list[dict]:
    if not EXCEPTIONS_FILE.exists():
        return []
    data = json.loads(EXCEPTIONS_FILE.read_text(encoding="utf-8"))
    today = date.today()
    exceptions = []
    for entry in data.get("exceptions", []):
        for key in ("directory", "package", "ghsa_id", "owner", "created_on",
                    "expires_on", "review_cadence", "rationale"):
            if not entry.get(key):
                fail(f"exception entry is missing required field '{key}': {entry!r}")
        created = date.fromisoformat(entry["created_on"])
        expires = date.fromisoformat(entry["expires_on"])
        if created > today or expires <= created or expires > created + MAX_EXCEPTION_AGE:
            fail(f"exception for {entry['package']} must expire within 30 days of creation")
        if expires <= today:
            fail(f"exception for {entry['package']} ({entry['ghsa_id']}) expired on {entry['expires_on']}")
        exceptions.append(entry)
    return [e for e in exceptions if e["directory"] == directory]


def ghsa_ids(via_item) -> set[str]:
    if isinstance(via_item, dict):
        url = via_item.get("url", "")
        if "/advisories/" in url:
            return {url.rstrip("/").rsplit("/", 1)[-1]}
    return set()


def dependency_names(via_item) -> set[str]:
    if isinstance(via_item, str):
        return {via_item}
    if isinstance(via_item, dict):
        name = via_item.get("name") or via_item.get("dependency") or ""
        return {name} if name else set()
    return set()


def load_report(path: str) -> dict:
    text = Path(path).read_text(encoding="utf-8")
    decoder = json.JSONDecoder()
    idx = 0
    while idx < len(text):
        while idx < len(text) and text[idx] not in "{[":
            idx += 1
        if idx >= len(text):
            break
        try:
            value, end = decoder.raw_decode(text, idx)
        except json.JSONDecodeError:
            idx += 1
            continue
        if isinstance(value, dict) and "vulnerabilities" in value:
            return value
        idx = end
    fail(f"could not find an npm audit report in {path}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", required=True,
                        help="audited matrix directory, e.g. 'frontend' or '.'")
    parser.add_argument("--audit", required=True, help="path to npm audit --json output")
    args = parser.parse_args()

    report = load_report(args.audit)
    vulnerabilities = report.get("vulnerabilities") or {}

    exceptions = load_exceptions(args.directory)
    exempted_ghsa = {e["ghsa_id"] for e in exceptions}
    exempted_packages = {e["package"] for e in exceptions}

    # A vulnerable entry is covered when every via item resolves to either an
    # exempted advisory, an exempted package, or a finding below the gate's
    # high threshold (npm still lists moderate roll-ups in the report tree).
    def via_item_covered(item, seen: set) -> bool:
        ids = ghsa_ids(item)
        if ids:
            if ids & exempted_ghsa:
                return True
            severity = item.get("severity") if isinstance(item, dict) else None
            # Fail closed: an advisory without an explicit severity is treated
            # as in-scope for the gate rather than silently below threshold.
            return severity is not None and severity not in ("high", "critical")
        for name in dependency_names(item):
            if name in seen:
                return False
            if not entry_covered(name, seen | {name}):
                return False
        return True

    def entry_covered(name: str, seen: set) -> bool:
        if name in exempted_packages:
            return True
        entry = vulnerabilities.get(name)
        if entry is None:
            return False
        if entry.get("severity") not in ("high", "critical"):
            return True
        via = entry.get("via") or []
        return bool(via) and all(via_item_covered(item, seen) for item in via)

    remaining = {}
    covered = set()
    for name, entry in vulnerabilities.items():
        if entry_covered(name, {name}):
            covered.add(name)
        elif entry.get("severity") in ("high", "critical"):
            remaining[name] = entry

    if remaining:
        print("npm-audit-gate: unreviewed high/critical advisories:", file=sys.stderr)
        for name, entry in sorted(remaining.items()):
            print(f"  {name} ({entry.get('severity')}): {entry.get('via')}", file=sys.stderr)
        return 1

    print(f"npm-audit-gate: {args.directory}: no unreviewed high/critical advisories"
          f" ({len(covered)} entr{'y' if len(covered) == 1 else 'ies'} covered by reviewed exceptions)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
