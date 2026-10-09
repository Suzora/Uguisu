#!/usr/bin/env python3
"""Run a command with the checkout at another version, then put it back.

    python3 scripts/at_version.py 0.0.1 -- python3 scripts/desktop_bundle.py deb

An upgrade test needs build A: this source, identifying itself as an older
version. This writes `<version>` into every place `check-version.py` reads
and into both lockfiles, runs the command, and restores every file it touched
byte for byte afterwards — also when the command fails or the run is
interrupted. It exits with the command's status.
"""

from __future__ import annotations

import argparse
import re
import signal
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Each place and the one value in it that carries the version: the first
# `version` of `[workspace.package]`, the top-level `version`, and the newest
# AppStream release, which comes first.
WORKSPACE = re.compile(r'(\[workspace\.package\][^\[]*?^version = ")([^"]*)(")', re.MULTILINE)
PLACES = {
    "Cargo.toml": WORKSPACE,
    "desktop/Cargo.toml": WORKSPACE,
    "web/package.json": re.compile(r'(\A\{[^{}]*?\n  "version": ")([^"]*)(")'),
    "desktop/flatpak/io.github.suzora.Uguisu.metainfo.xml": re.compile(r'(<releases>\s*<release version=")([^"]*)(")'),
}
LOCKS = ("Cargo.lock", "desktop/Cargo.lock")
# A lockfile's packages without a `source` are path packages, and every one of
# them takes the workspace version; these are the lines cargo itself rewrites.
# Rewriting them here needs no registry, which an offline build has not got.
LOCAL = r'(\[\[package\]\]\nname = "[^"]+"\nversion = "){old}("\n)(?!source = )'
# MSI's rule, as in check-version.py: a build A that MSI refuses is no build A.
PLAIN = re.compile(r"(\d+)\.(\d+)\.(\d+)", re.ASCII)
MSI_LIMITS = (255, 255, 65535)


def interrupted(_signum: int, _frame: object) -> None:
    raise KeyboardInterrupt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version", help="plain major.minor.patch")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="-- <command and arguments>")
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a command after -- is required")
    plain = PLAIN.fullmatch(args.version)
    if not plain or any(int(part) > most for part, most in zip(plain.groups(), MSI_LIMITS)):
        parser.error(f"{args.version!r} is not plain major.minor.patch within MSI's 255.255.65535")

    saved = {path: (ROOT / path).read_bytes() for path in (*PLACES, *LOCKS)}
    signal.signal(signal.SIGTERM, interrupted)
    try:
        current = WORKSPACE.search(saved["Cargo.toml"].decode("utf-8"))
        if current is None:
            print("FAIL at_version: Cargo.toml has no [workspace.package] version")
            return 1
        for path, pattern in PLACES.items():
            changed, count = pattern.subn(rf"\g<1>{args.version}\g<3>", saved[path].decode("utf-8"), count=1)
            if count != 1:
                print(f"FAIL at_version: no version found in {path}")
                return 1
            (ROOT / path).write_bytes(changed.encode("utf-8"))
        local = re.compile(LOCAL.format(old=re.escape(current.group(2))))
        for path in LOCKS:
            changed, count = local.subn(rf"\g<1>{args.version}\g<2>", saved[path].decode("utf-8"))
            if count == 0:
                print(f"FAIL at_version: no package of this workspace in {path}")
                return 1
            (ROOT / path).write_bytes(changed.encode("utf-8"))
        return subprocess.run(command, cwd=ROOT, check=False).returncode
    finally:
        for path, data in saved.items():
            (ROOT / path).write_bytes(data)


if __name__ == "__main__":
    sys.exit(main())
