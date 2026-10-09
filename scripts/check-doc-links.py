#!/usr/bin/env python3
"""Static checks over the repository's documentation.

Three things, all cheap and all deterministic:

- every relative Markdown link resolves (no network, anchors ignored);
- `CLAUDE.md` stays within its line budget, because a rules file nobody
  reads to the end is a rules file that does not apply;
- every ADR on disk is listed in `docs/DECISIONS/README.md` -- a link
  checker cannot catch a *missing* link, which is how 0027-0030 came to
  exist unlisted.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
SKIP_DIRS = {"node_modules", "target", ".git", "dist"}
CLAUDE_MD_MAX_LINES = 250
ADR_DIR = ROOT / "docs" / "DECISIONS"


def markdown_files() -> list[Path]:
    return sorted(
        path
        for path in ROOT.rglob("*.md")
        if not any(part in SKIP_DIRS for part in path.parts)
    )


def check_links(files: list[Path]) -> list[str]:
    broken: list[str] = []
    for md in files:
        text = md.read_text(encoding="utf-8")
        for match in LINK.finditer(text):
            target = match.group(1)
            if re.match(r"^[a-z]+:", target) or target.startswith("#"):
                continue
            target = target.split("#", 1)[0]
            if not target:
                continue
            if not (md.parent / target).resolve().exists():
                broken.append(f"{md.relative_to(ROOT)}: {match.group(1)}")
    return broken


def check_claude_md() -> list[str]:
    path = ROOT / "CLAUDE.md"
    if not path.exists():
        return ["CLAUDE.md is missing"]
    lines = len(path.read_text(encoding="utf-8").splitlines())
    if lines > CLAUDE_MD_MAX_LINES:
        return [f"CLAUDE.md is {lines} lines, the budget is {CLAUDE_MD_MAX_LINES}"]
    return []


def check_adr_index() -> list[str]:
    index = ADR_DIR / "README.md"
    if not index.exists():
        return ["docs/DECISIONS/README.md is missing"]
    listed = index.read_text(encoding="utf-8")
    missing = [
        f"docs/DECISIONS/{adr.name} exists but docs/DECISIONS/README.md does not link it"
        for adr in sorted(ADR_DIR.glob("[0-9][0-9][0-9][0-9]-*.md"))
        if adr.name not in listed
    ]
    return missing


def main() -> int:
    files = markdown_files()
    problems = check_links(files) + check_claude_md() + check_adr_index()
    if problems:
        print("Documentation problems:")
        for item in problems:
            print(f"  {item}")
        return 1
    print(f"Checked {len(files)} markdown files: links, CLAUDE.md budget, ADR index.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
