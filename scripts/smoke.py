#!/usr/bin/env python3
"""Exercise the `uguisu` binary against a cold, empty data directory.

This is deliberately not a `cargo test`: `crates/uguisu-cli/tests/*` already
assert these commands against mock servers. What this adds is the shipped
binary, a data directory that has never existed before, no mocks and no test
harness -- the state a first run is actually in. Turning it into a test would
delete exactly that property.

Assertions read the `--json` body with `json.loads` rather than grepping it,
which is both stricter and immune to how the JSON happens to be formatted.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


class Failure(Exception):
    """A step did not do what it promised."""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", help="an already-built uguisu binary (default: build it)")
    parser.add_argument("--keep-dir", action="store_true", help="keep the data directory")
    args = parser.parse_args()

    binary = args.bin
    if not binary:
        subprocess.run(
            ["cargo", "build", "-q", "-p", "uguisu-cli", "--bin", "uguisu"],
            cwd=ROOT,
            check=True,
        )
        binary = str(ROOT / "target" / "debug" / "uguisu")
    base = [binary]
    data_dir = Path(tempfile.mkdtemp(prefix="uguisu-smoke-"))
    env = dict(os.environ, UGUISU_DATA_DIR=str(data_dir))
    empty = Path(tempfile.mkdtemp(prefix="uguisu-smoke-empty-"))
    keep = args.keep_dir

    def run(argv: list[str], *, expect: int = 0) -> subprocess.CompletedProcess[str]:
        done = subprocess.run(
            base + argv,
            cwd=ROOT,
            env=env,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
        )
        if done.returncode != expect:
            raise Failure(
                f"`uguisu {' '.join(argv)}` exited {done.returncode}, expected {expect}\n"
                f"--- stdout ---\n{done.stdout}\n--- stderr ---\n{done.stderr}"
            )
        return done

    def body(argv: list[str]) -> dict:
        done = run(argv)
        try:
            return json.loads(done.stdout)
        except json.JSONDecodeError as e:
            raise Failure(
                f"`uguisu {' '.join(argv)}` did not print JSON: {e}\n{done.stdout}"
            ) from e

    def expect(argv: list[str], **fields) -> None:
        got = body(argv)
        for key, want in fields.items():
            if got.get(key) != want:
                raise Failure(
                    f"`uguisu {' '.join(argv)}`: {key} is {got.get(key)!r}, expected {want!r}\n"
                    f"{json.dumps(got, indent=2)[:2000]}"
                )

    checks = 0
    try:
        run(["--help"])
        checks += 1

        # An empty archive checks nothing and succeeds, with a tagged body.
        expect(["--json", "archive", "verify", "--all"], schema=1, checked=0)
        checks += 1

        # An import is a dry run by default, so an empty tree is a plan with
        # nothing in it -- not an error, and nothing written.
        expect(["--json", "archive", "import", str(empty)], schema=1, applied=False, scanned=0)
        checks += 1

        expect(["--json", "archive", "manifest", "status"], manifests=[])
        checks += 1

        expect(["--json", "archive", "reconcile", "--rebuild"], applied=False, rebuilt=0)
        checks += 1

        # An archive can never be imported into itself.
        (data_dir / "media").mkdir(parents=True, exist_ok=True)
        done = run(["--json", "archive", "import", str(data_dir / "media")], expect=1)
        if "imported into itself" not in done.stderr:
            raise Failure(f"expected a self-import refusal, got:\n{done.stderr}")
        checks += 1

        # The service commands answer on an empty library.
        expect(["--json", "scheduler", "status"], enabled=True, running=False, due_now=0)
        checks += 1

        # Nothing is stored, so nothing is being ignored.
        run(["config", "validate"])
        checks += 1

        expect(["--json", "config", "set", "UGUISU_ARCHIVE_MAX_BACKLOG", "5"], origin="settings")
        checks += 1

        run(["--json", "config", "unset", "UGUISU_ARCHIVE_MAX_BACKLOG"])
        checks += 1

        # An empty library finds nothing and exits 3 saying which kind of
        # nothing it was, rather than printing an empty list.
        run(["search", "library", "anything"], expect=3)
        checks += 1
    except Failure as e:
        keep = True
        print(f"FAIL smoke after {checks} checks")
        print(f"  data dir: {data_dir}")
        print()
        print(e)
        return 1
    finally:
        shutil.rmtree(empty, ignore_errors=True)
        if not keep:
            shutil.rmtree(data_dir, ignore_errors=True)
        else:
            print(f"data directory kept: {data_dir}", file=sys.stderr)

    print(f"{checks} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
