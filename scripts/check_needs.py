#!/usr/bin/env python3
"""The desktop packaging gate, and the check that keeps it a gate.

    check_needs.py '<toJSON(needs)>'   run by the `desktop-packaging` job
    check_needs.py --workflow          run by `check.py desktop-meta`

In the workflow, `desktop-packaging` needs the six `package-*` jobs and
`upgrade-deb`, and runs even when they did not (`if: always()`), so its
verdict is an explicit pass or fail and never a skip. The workflow runs only
on a tag or by hand, where bundling is always required, so the gate passes
only when all seven succeeded. Anything else — a failure, a cancellation, a
skip, a job missing from `needs` — fails. Packaging is reported done only
from a run where this job passed.

`--workflow` fails when `.github/workflows/desktop.yml` stops matching this:
a format or the upgrade dropped from `needs`, a `package-*` job added or
renamed without being listed here, the `upgrade-deb` job removed, a trigger
on pull requests or branches, `continue-on-error` anywhere, the gate losing
`if: always()`, or a `docker run` without `--init`. It also fails on a job
without `timeout-minutes` in desktop.yml or ci.yml. It is line-based so that
no YAML parser is needed.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/desktop.yml"
CI = ROOT / ".github/workflows/ci.yml"
FORMATS = ("deb", "rpm", "appimage", "flatpak", "nsis", "msi")
PACKAGE_JOBS = tuple(f"package-{f}" for f in FORMATS)
GATE = "desktop-packaging"
UPGRADE = "upgrade-deb"
GATED = (*PACKAGE_JOBS, UPGRADE)


def gate(needs: dict) -> int:
    if set(needs) != set(GATED):
        print(f"FAIL {GATE} needs {sorted(needs)}, expected {sorted(GATED)}")
        return 1
    failed = False
    for job in GATED:
        result = needs[job].get("result")
        print(f"{job}: {result}")
        failed |= result != "success"
    if failed:
        print(f"FAIL {GATE}: every package job and {UPGRADE} must succeed")
        return 1
    print(f"{GATE}: all six formats installed and passed, and the upgrade kept the data")
    return 0


def untimed(text: str) -> list[str]:
    """The jobs without `timeout-minutes`: a hung one holds its runner for six hours."""
    section = text[text.index("\njobs:\n"):]
    return [
        job
        for job, body in re.findall(r"^  ([a-z0-9-]+):\s*\n((?:    .*\n|\s*\n)*)", section, re.MULTILINE)
        if not re.search(r"^    timeout-minutes:\s*\d+\s*$", body, re.MULTILINE)
    ]


def workflow() -> int:
    text = WORKFLOW.read_text(encoding="utf-8")
    problems = []
    jobs = re.findall(r"^  ([a-z0-9-]+):\s*$", text, re.MULTILINE)
    packages = {job for job in jobs if job.startswith("package-")}
    if packages != set(PACKAGE_JOBS):
        problems.append(f"package jobs are {sorted(packages)}, expected {sorted(PACKAGE_JOBS)}")
    block = re.search(rf"^  {GATE}:\s*\n((?:    .*\n|\s*\n)*)", text, re.MULTILINE)
    if block is None:
        problems.append(f"there is no {GATE} job")
    else:
        body = block.group(1)
        needs = re.search(r"^    needs:\s*\[([^\]]*)\]", body, re.MULTILINE)
        listed = {n.strip() for n in needs.group(1).split(",")} if needs else set()
        if listed != set(GATED):
            problems.append(f"{GATE} needs {sorted(listed)}, expected the six package jobs and {UPGRADE}")
        if not re.search(r"^    if:\s*always\(\)\s*$", body, re.MULTILINE):
            problems.append(f"{GATE} must run with `if: always()`, or a skip could pass for success")
        if "scripts/check_needs.py" not in body:
            problems.append(f"{GATE} does not run scripts/check_needs.py")
    # Not one of the six formats, but the only proof that an upgrade keeps the
    # data, so it must not disappear either.
    if UPGRADE not in jobs:
        problems.append(f"the {UPGRADE} job is missing")
    # Packaging costs about 170 billed minutes a run; it runs before a release,
    # not on every change (ADR 0046).
    on = re.search(r"^on:\s*\n((?:[ #].*\n|\s*\n)*)", text, re.MULTILINE)
    triggers = on.group(1) if on else ""
    if not on or re.search(r"pull_request|branches", triggers):
        problems.append("the workflow must run only on tags and workflow_dispatch")
    if "continue-on-error" in text:
        problems.append("continue-on-error appears in the workflow")
    for path in (WORKFLOW, CI):
        for job in untimed(path.read_text(encoding="utf-8")):
            problems.append(f"the {job} job in {path.name} has no timeout-minutes")
    # bash execs the last command of `-c`, so without an init the container's
    # xvfb-run is PID 1, which Xvfb never signals ready: the job hangs.
    if re.search(r"docker run(?![^\n]*--init)", text):
        problems.append("a docker run without --init")
    if problems:
        print("FAIL desktop workflow")
        for problem in problems:
            print(f"  {problem}")
        return 1
    print(f"{GATE} gates {len(PACKAGE_JOBS)} package jobs and {UPGRADE}")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--workflow"]:
        return workflow()
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    return gate(json.loads(sys.argv[1]))


if __name__ == "__main__":
    sys.exit(main())
