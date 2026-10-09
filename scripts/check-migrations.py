#!/usr/bin/env python3
"""Refuse any change to a migration that has already shipped.

`crates/uguisu-storage/src/lib.rs` applies migrations with `sqlx::migrate!`,
which records a checksum per file in `_sqlx_migrations` and refuses to open a
database whose migration files no longer match. Editing a shipped migration --
including reformatting a comment inside one -- therefore breaks every existing
installation, and no test notices, because tests always start from an empty
directory.

Adding a new migration file is fine. Changing or deleting an old one is not.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MIGRATIONS = "crates/uguisu-storage/migrations"


def git(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        cwd=ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def base_ref() -> str | None:
    for ref in ("origin/main", "main"):
        if git("rev-parse", "--verify", "--quiet", ref).returncode == 0:
            return ref
    return None


def main() -> int:
    base = base_ref()
    if base is None:
        print("SKIP: no base ref (origin/main or main) to compare against")
        return 0

    # Two questions, because a commit range says nothing about a file that has
    # been edited but not committed yet -- which is exactly when an agent can
    # still undo it.
    diffs = [
        ("since " + base, git("diff", "--name-status", f"{base}...HEAD", "--", MIGRATIONS)),
        ("in the working tree", git("diff", "--name-status", "HEAD", "--", MIGRATIONS)),
    ]
    touched = []
    for where, diff in diffs:
        if diff.returncode != 0:
            print(f"SKIP: cannot diff {where}: {diff.stderr.strip()}")
            return 0
        for line in diff.stdout.splitlines():
            parts = line.split("\t")
            status, path = parts[0], parts[-1]
            # Adding a file is the supported way to change the schema.
            if status.startswith("A") or status.startswith("?"):
                continue
            touched.append((f"{status} {where}", path))

    if touched:
        print("FAIL: a shipped migration changed")
        for status, path in touched:
            print(f"  {status}  {path}")
        print()
        print("sqlx checksums every migration. A changed file makes every existing")
        print("database refuse to open. Revert it and add a new numbered migration.")
        return 1

    print(f"migrations unchanged since {base} and in the working tree")
    return 0


if __name__ == "__main__":
    sys.exit(main())
