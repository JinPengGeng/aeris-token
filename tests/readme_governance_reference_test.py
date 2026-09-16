#!/usr/bin/env python3
"""Check README local navigation and exact CODEOWNERS paths."""

from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_readme_primary_navigation_resolves() -> None:
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    navigation = {
        "简介": "简介",
        "部署": "部署",
        "api-文档": "API 文档",
        "环境变量": "环境变量",
        "qa": "Q&A",
    }
    for anchor, heading in navigation.items():
        assert f'href="#{anchor}"' in readme, f"README navigation is missing #{anchor}"
        assert f"\n## {heading}\n" in readme, f"README navigation target is missing: {heading}"


def test_codeowners_exact_paths_exist() -> None:
    codeowners = ROOT / ".github" / "CODEOWNERS"
    missing: list[str] = []
    for line in codeowners.read_text(encoding="utf-8").splitlines():
        stripped = line.split("#", 1)[0].strip()
        if not stripped:
            continue
        pattern = stripped.split()[0]
        if pattern.startswith("/") and not any(char in pattern for char in "*?["):
            target = ROOT / pattern.lstrip("/")
            if not target.exists():
                missing.append(pattern)
    assert not missing, f"CODEOWNERS contains missing exact paths: {missing}"


if __name__ == "__main__":
    test_readme_primary_navigation_resolves()
    test_codeowners_exact_paths_exist()
    print("README and CODEOWNERS references are valid")
