#!/usr/bin/env python3
"""The two gates that turn many jobs into one verdict, and the check that
keeps them gates.

    check_needs.py '<toJSON(needs)>'        run by desktop.yml's `desktop-packaging` job
    check_needs.py --ci '<toJSON(needs)>'   run by ci.yml's `ci` job
    check_needs.py --workflow               run by `check.py desktop-meta`

In the workflow, `desktop-packaging` needs the six `package-*` jobs and the
five `upgrade-*` jobs, and runs even when they did not (`if: always()`), so
its verdict is an explicit pass or fail and never a skip. The workflow runs
only on a tag or by hand, where bundling is always required, so the gate
passes only when all eleven succeeded. Anything else — a failure, a cancellation, a
skip, a job missing from `needs` — fails. Packaging is reported done only
from a run where this job passed.

`ci` is the one check a pull request needs before it merges (ADR 0063). It
needs every job in ci.yml except `win-test-full`, which never runs on a pull
request, and likewise passes only when every one of them succeeded.

`--workflow` fails when `.github/workflows/desktop.yml` stops matching this:
a format or an upgrade dropped from `needs`, a `package-*` or `upgrade-*` job
added, renamed or removed without being listed here, a trigger
on pull requests or branches, `continue-on-error` anywhere, the gate losing
`if: always()`, or a `docker run` without `--init`. In ci.yml it fails when
`ci` stops needing every other job but `win-test-full`, loses `if: always()`
or stops running this script, when `win-test-full` could run on a pull
request, and on `continue-on-error`. It also fails on a job without
`timeout-minutes` in either workflow. It is line-based so that
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
# The only proof that an update keeps the data; the AppImage has none, since
# a user replaces its file.
UPGRADE_JOBS = ("upgrade-deb", "upgrade-rpm", "upgrade-flatpak", "upgrade-nsis", "upgrade-msi")
GATED = (*PACKAGE_JOBS, *UPGRADE_JOBS)
CI_GATE = "ci"
# The whole suite on Windows: too slow for a pull request, so it runs after
# the merge and turns main red rather than holding up the gate.
POST_MERGE = "win-test-full"


def gate(name: str, needs: dict, expected: tuple[str, ...], failure: str, passed: str) -> int:
    if set(needs) != set(expected):
        print(f"FAIL {name} needs {sorted(needs)}, expected {sorted(expected)}")
        return 1
    failed = False
    for job in expected:
        result = needs[job].get("result")
        print(f"{job}: {result}")
        failed |= result != "success"
    if failed:
        print(f"FAIL {name}: {failure}")
        return 1
    print(f"{name}: {passed}")
    return 0


def jobs(text: str) -> dict[str, str]:
    """Each job's name and body, from a workflow's `jobs:` section."""
    section = text[text.index("\njobs:\n"):]
    return dict(re.findall(r"^  ([a-z0-9-]+):\s*\n((?:    .*\n|\s*\n)*)", section, re.MULTILINE))


def ci_gated(text: str) -> tuple[str, ...]:
    return tuple(job for job in jobs(text) if job not in (CI_GATE, POST_MERGE))


def untimed(text: str) -> list[str]:
    """The jobs without `timeout-minutes`: a hung one holds its runner for six hours."""
    return [
        job
        for job, body in jobs(text).items()
        if not re.search(r"^    timeout-minutes:\s*\d+\s*$", body, re.MULTILINE)
    ]


def ci_problems(text: str) -> list[str]:
    problems = []
    body = jobs(text).get(CI_GATE)
    if body is None:
        problems.append(f"ci.yml has no {CI_GATE} job")
    else:
        needs = re.search(r"^    needs:\s*\[([^\]]*)\]", body, re.MULTILINE)
        listed = {n.strip() for n in needs.group(1).split(",")} if needs else set()
        if listed != set(ci_gated(text)):
            problems.append(
                f"{CI_GATE} needs {sorted(listed)}, expected every job but {POST_MERGE}: "
                f"{sorted(ci_gated(text))}"
            )
        if not re.search(r"^    if:\s*always\(\)\s*$", body, re.MULTILINE):
            problems.append(f"{CI_GATE} must run with `if: always()`, or a skip could pass for success")
        if "scripts/check_needs.py --ci" not in body:
            problems.append(f"{CI_GATE} does not run scripts/check_needs.py --ci")
    post = jobs(text).get(POST_MERGE, "")
    if not re.search(r"^    if:\s*github\.event_name != 'pull_request'\s*$", post, re.MULTILINE):
        problems.append(f"{POST_MERGE} must exist and never run on a pull request")
    if "continue-on-error" in text:
        problems.append("continue-on-error appears in ci.yml")
    return problems


def workflow() -> int:
    text = WORKFLOW.read_text(encoding="utf-8")
    problems = []
    jobs = re.findall(r"^  ([a-z0-9-]+):\s*$", text, re.MULTILINE)
    packages = {job for job in jobs if job.startswith("package-")}
    if packages != set(PACKAGE_JOBS):
        problems.append(f"package jobs are {sorted(packages)}, expected {sorted(PACKAGE_JOBS)}")
    upgrades = {job for job in jobs if job.startswith("upgrade-")}
    if upgrades != set(UPGRADE_JOBS):
        problems.append(f"upgrade jobs are {sorted(upgrades)}, expected {sorted(UPGRADE_JOBS)}")
    block = re.search(rf"^  {GATE}:\s*\n((?:    .*\n|\s*\n)*)", text, re.MULTILINE)
    if block is None:
        problems.append(f"there is no {GATE} job")
    else:
        body = block.group(1)
        needs = re.search(r"^    needs:\s*\[([^\]]*)\]", body, re.MULTILINE)
        listed = {n.strip() for n in needs.group(1).split(",")} if needs else set()
        if listed != set(GATED):
            problems.append(f"{GATE} needs {sorted(listed)}, expected every package and upgrade job")
        if not re.search(r"^    if:\s*always\(\)\s*$", body, re.MULTILINE):
            problems.append(f"{GATE} must run with `if: always()`, or a skip could pass for success")
        if "scripts/check_needs.py" not in body:
            problems.append(f"{GATE} does not run scripts/check_needs.py")
    # Packaging takes far longer than a pull request's five minutes; it runs
    # before a release, not on every change (ADR 0046, ADR 0063).
    on = re.search(r"^on:\s*\n((?:[ #].*\n|\s*\n)*)", text, re.MULTILINE)
    triggers = on.group(1) if on else ""
    if not on or re.search(r"pull_request|branches", triggers):
        problems.append("the workflow must run only on tags and workflow_dispatch")
    if "continue-on-error" in text:
        problems.append("continue-on-error appears in the workflow")
    for path in (WORKFLOW, CI):
        for job in untimed(path.read_text(encoding="utf-8")):
            problems.append(f"the {job} job in {path.name} has no timeout-minutes")
    problems += ci_problems(CI.read_text(encoding="utf-8"))
    # bash execs the last command of `-c`, so without an init the container's
    # xvfb-run is PID 1, which Xvfb never signals ready: the job hangs.
    if re.search(r"docker run(?![^\n]*--init)", text):
        problems.append("a docker run without --init")
    if problems:
        print("FAIL workflows")
        for problem in problems:
            print(f"  {problem}")
        return 1
    print(f"{GATE} gates {len(PACKAGE_JOBS)} package jobs and {len(UPGRADE_JOBS)} upgrade jobs")
    print(f"{CI_GATE} gates {len(ci_gated(CI.read_text(encoding='utf-8')))} jobs")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--workflow"]:
        return workflow()
    if len(sys.argv) == 3 and sys.argv[1] == "--ci":
        expected = ci_gated(CI.read_text(encoding="utf-8"))
        return gate(
            CI_GATE, json.loads(sys.argv[2]), expected,
            failure="every job must succeed",
            passed=f"all {len(expected)} jobs passed",
        )
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    return gate(
        GATE, json.loads(sys.argv[1]), GATED,
        failure="every package and upgrade job must succeed",
        passed="all six formats installed and passed, and every upgrade kept the data",
    )


if __name__ == "__main__":
    sys.exit(main())
