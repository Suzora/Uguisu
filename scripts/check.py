#!/usr/bin/env python3
"""Run Uguisu's checks and say `PASS` or `FAIL <name>` plus everything relevant.

Every check command in the repository is defined here once. `justfile` and
`.github/workflows/ci.yml` call this script rather than re-spelling cargo
invocations, which is how four copies of `cargo clippy --workspace
--all-targets -- -D warnings` (three of them already divergent) became one.

Output on success is one line per check, because a passing five-minute test
run has nothing to say and an agent pays for every line of it. Nothing is
printed while a check runs; use `--stream` to watch one. Output on failure is
the command, the log path and the captured log in full: nothing is filtered,
truncated or summarised. CI always passes `--stream`, so its logs stay
exactly as debuggable as before.

Python rather than shell: the Rust CI job runs on Windows too, and
`scripts/check-doc-links.py` already makes python3 a repository dependency.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOG_DIR = ROOT / "target" / "check"
PY = sys.executable or "python3"


class Step:
    """One command, and the directory and tool it needs."""

    def __init__(
        self, argv: list[str], cwd: str | None = None, tool: str | None = None
    ) -> None:
        self.argv = argv
        self.cwd = ROOT / cwd if cwd else ROOT
        self.tool = tool or argv[0]


def count_tests(log: str) -> str | None:
    """Sum libtest's per-binary result lines into one summary."""
    passed = ignored = 0
    for m in re.finditer(r"(\d+) passed; (\d+) failed; (\d+) ignored", log):
        passed += int(m.group(1))
        ignored += int(m.group(3))
    if not passed and not ignored:
        return None
    return f"{passed} passed, {ignored} ignored"


def count_files(log: str) -> str | None:
    m = re.search(r"Checked (\d+) markdown files", log)
    return f"{m.group(1)} files" if m else None


def count_smoke(log: str) -> str | None:
    m = re.search(r"(\d+) checks", log)
    return f"{m.group(1)} checks" if m else None


class Check:
    """A named group of steps, all of which must succeed."""

    def __init__(self, *steps: Step, summary=None) -> None:
        self.steps = steps
        self.summary = summary


# `-D warnings` after `--` denies rustc lints too, so `missing_docs` fails here
# exactly as it does under CI's RUSTFLAGS. CI sets RUSTFLAGS and this does not,
# so dropping the flag would make the local check weaker than the remote one.
CHECKS: dict[str, Check] = {
    "fmt": Check(Step(["cargo", "fmt", "--all", "--check"])),
    # `--locked` on the commands that resolve dependencies: a Cargo.lock that
    # needs updating fails here instead of being rewritten and passing.
    "clippy": Check(
        Step(
            ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]
        )
    ),
    "build": Check(Step(["cargo", "build", "--workspace", "--all-targets"])),
    # Every test binary runs even after one fails, so a red run shows every
    # failure at once rather than the first binary's.
    "test": Check(
        Step(["cargo", "test", "--workspace", "--no-fail-fast", "--locked"]), summary=count_tests
    ),
    "bench": Check(Step(["cargo", "bench", "--workspace", "--no-run"])),
    # Both workspaces: needs no compiler, so CI runs it in its first job.
    "deny": Check(
        Step(
            ["cargo", "deny", "check", "advisories", "bans", "licenses", "sources"],
            tool="cargo-deny",
        ),
        Step(
            [
                "cargo", "deny",
                "--manifest-path", "src-tauri/Cargo.toml",
                "check", "advisories", "bans", "licenses", "sources",
            ],
            cwd="desktop",
            tool="cargo-deny",
        ),
    ),
    "web": Check(
        Step(["pnpm", "check"], cwd="web", tool="pnpm"),
        Step(["pnpm", "test"], cwd="web", tool="pnpm"),
    ),
    "web-build": Check(Step(["pnpm", "build"], cwd="web", tool="pnpm")),
    # The web dependencies, build tooling included, against the npm advisory
    # database. It asks the registry, so it is not in DEFAULT and CI does not
    # run it; `deny` is the Rust side's equivalent and needs no network.
    "audit": Check(Step(["pnpm", "audit"], cwd="web", tool="pnpm")),
    "docs": Check(Step([PY, "scripts/check-doc-links.py"]), summary=count_files),
    "smoke": Check(Step([PY, "scripts/smoke.py"]), summary=count_smoke),
    # `e2e` needs the built UI, so it builds it first rather than failing on a
    # tree where nobody ran pnpm.
    "e2e": Check(
        Step(["pnpm", "build"], cwd="web", tool="pnpm"),
        Step([PY, "scripts/e2e.py"]),
        summary=count_smoke,
    ),
    "migrations": Check(Step([PY, "scripts/check-migrations.py"])),
    "openapi": Check(Step(["cargo", "run", "-q", "-p", "openapi-doc", "--", "--check"])),
    "api-types": Check(
        Step(["pnpm", "api:types:check"], cwd="web", tool="pnpm"),
    ),
    # One application version, checked in every place a release carries it.
    "version": Check(Step([PY, "scripts/check-version.py"])),
    # The Dockerfile's rules (ADR 0061), read as text: no Docker needed.
    "docker": Check(Step([PY, "scripts/check-docker.py"])),
    # Builds the image and runs it (ADR 0061). Needs Docker and the network
    # the build needs, so only on request; skipped where Docker is missing.
    "docker-smoke": Check(Step([PY, "scripts/docker_smoke.py"], tool="docker"), summary=count_smoke),
    # The desktop harnesses and the workflows' shape: Python only, so CI runs
    # it in its first job, before anything is compiled.
    "desktop-meta": Check(
        # First, so a smoke harness that could not fail never gets to pass.
        Step([PY, "scripts/desktop_smoke.py", "--self-test"]),
        Step([PY, "scripts/check-desktop-layout.py"]),
        Step([PY, "scripts/check_needs.py", "--workflow"]),
    ),
    # The desktop shell is a separate workspace because building it needs
    # WebKitGTK or the WebView2 SDK, which `--workspace` above must not start
    # requiring. Deliberately outside DEFAULT: on a machine without those
    # libraries this fails with a linker error naming them, which is the right
    # answer for someone who asked for it and the wrong one for someone who
    # just ran `check.py`.
    "desktop": Check(
        Step(["cargo", "fmt", "--all", "--check"], cwd="desktop"),
        Step(
            ["cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"],
            cwd="desktop",
        ),
        Step(["cargo", "test", "--locked"], cwd="desktop", tool="cargo"),
        summary=count_tests,
    ),
}

DEFAULT = ["fmt", "clippy", "test", "web", "docs", "openapi", "api-types", "version", "docker"]


def missing_tool(check: Check) -> str | None:
    for step in check.steps:
        if shutil.which(step.tool) is None:
            return step.tool
    return None


def run(name: str, check: Check, *, stream: bool) -> tuple[bool, str]:
    """Runs every step of one check. Returns (passed, captured log)."""
    log = ""
    for step in check.steps:
        shown = " ".join(step.argv)
        # On Windows `pnpm` may be a `pnpm.cmd` shim, which CreateProcess runs
        # only when it is given the full path.
        argv = [shutil.which(step.argv[0]) or step.argv[0], *step.argv[1:]]
        if stream:
            print(f"$ {shown}", flush=True)
            done = subprocess.run(argv, cwd=step.cwd, check=False)
        else:
            done = subprocess.run(
                argv,
                cwd=step.cwd,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                errors="replace",
            )
            log += done.stdout or ""
        if done.returncode != 0:
            path = LOG_DIR / f"{name}.log"
            print(f"FAIL {name}")
            print(f"  command: {shown}  (in {step.cwd})")
            print(f"  exit:    {done.returncode}")
            if not stream:
                LOG_DIR.mkdir(parents=True, exist_ok=True)
                path.write_text(log, encoding="utf-8", errors="replace")
                print(f"  log:     {path}")
                print()
                print(log, end="" if log.endswith("\n") else "\n")
            return False, log
    return True, log


def main() -> int:
    # A failing tool's log may hold characters the console's code page lacks
    # (cp1252 on Windows); printing it must not end the run.
    sys.stdout.reconfigure(errors="replace")
    parser = argparse.ArgumentParser(
        description="Run Uguisu's checks with compact output.",
        epilog="checks: " + " ".join(CHECKS) + "; 'all' runs every one",
    )
    parser.add_argument("checks", nargs="*", metavar="CHECK")
    parser.add_argument(
        "--stream", action="store_true", help="inherit stdio instead of capturing"
    )
    parser.add_argument(
        "--require-tools",
        action="store_true",
        help="a missing tool fails instead of skipping (CI uses this)",
    )
    parser.add_argument("--fail-fast", action="store_true", help="stop at the first failure")
    parser.add_argument("--list", action="store_true", help="list the checks and exit")
    args = parser.parse_args()

    if args.list:
        for name, check in CHECKS.items():
            print(f"{name:12} {'; '.join(' '.join(s.argv) for s in check.steps)}")
        print(f"\ndefault: {' '.join(DEFAULT)}")
        return 0

    selected = args.checks or DEFAULT
    if selected == ["all"]:
        selected = list(CHECKS)
    unknown = [c for c in selected if c not in CHECKS]
    if unknown:
        print(f"unknown check(s): {' '.join(unknown)}", file=sys.stderr)
        print(f"available: {' '.join(CHECKS)} all", file=sys.stderr)
        return 2

    # One selected check answers with a bare PASS; a set answers per name.
    bare = len(selected) == 1
    failed: list[str] = []
    # Every step's temporary files go under target/ rather than the system's
    # temporary directory, which nothing empties: the Rust tests leave
    # databases behind, and on Windows a file SQLite still has open cannot be
    # deleted when the test drops it. A run that passed removes the directory;
    # a failed one keeps it, with any data directory e2e or smoke kept there.
    scratch = LOG_DIR / f"tmp-{os.getpid()}"
    scratch.mkdir(parents=True, exist_ok=True)
    for key in ("TMPDIR", "TEMP", "TMP"):
        os.environ[key] = str(scratch)
    try:
        for name in selected:
            check = CHECKS[name]
            tool = missing_tool(check)
            if tool and not args.require_tools:
                print(f"SKIP {name} ({tool} not installed)")
                continue
            if tool:
                print(f"FAIL {name} ({tool} not installed)")
                failed.append(name)
                if args.fail_fast:
                    break
                continue
            # Nothing is printed until the check finishes: a passing check is one
            # line and that line is the answer. `--stream` is the way to watch a
            # long run, and a failure prints its log path itself.
            started = time.monotonic()
            ok, log = run(name, check, stream=args.stream)
            if not ok:
                failed.append(name)
                if args.fail_fast:
                    break
                continue
            extra = check.summary(log) if check.summary else None
            took = f"{time.monotonic() - started:.0f}s"
            if bare:
                print(f"PASS{f' ({extra})' if extra else ''}")
            else:
                print(f"PASS {name}{f' ({extra})' if extra else ''}  {took}")
    finally:
        if not failed:
            shutil.rmtree(scratch, ignore_errors=True)

    if failed:
        print(f"FAILED {len(failed)} of {len(selected)}: {' '.join(failed)}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
