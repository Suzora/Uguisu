#!/usr/bin/env python3
"""Build the desktop binary from a fresh link and bundle the requested formats.

    desktop_bundle.py deb rpm appimage      # on Linux
    desktop_bundle.py nsis msi              # on Windows

The pinned tauri-bundler records each package's format by overwriting a
placeholder (`__TAURI_BUNDLE_TYPE_VAR_UNK`) in the release binary in place,
and restores the binary after each format. A bundle run that fails part-way
skips the restore. The patched binary is then still in `target/release`,
cargo considers it fresh, and the next bundle run can only warn that the
placeholder is gone and package a binary that names the wrong format. So
this always relinks the binary, refuses one without its placeholder, turns
that warning into a failure, and checks that the restore happened.

The web UI must already be built into `web/dist`; the bundle takes it from
there (`bundle.resources`). The AppImage tools are put in place from pinned
sources first, and a download by the bundler is a failure.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DESKTOP = ROOT / "desktop"
EXE = ROOT / "target" / "release" / ("uguisu-desktop.exe" if os.name == "nt" else "uguisu-desktop")
PLACEHOLDER = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
FORMATS = ("deb", "rpm", "appimage", "nsis", "msi")
TOOLS = [sys.executable, str(ROOT / "scripts" / "appimage_tools.py")]
# The Linux support floor: Ubuntu 22.04 and Debian 12, glibc 2.35, WebKitGTK
# 4.1 (ADR 0043). A binary linked on a newer system asks for newer glibc
# symbols and does not start on the floor, so it is refused here rather than
# discovered by a user.
GLIBC_FLOOR = (2, 35)


def run(*command: str) -> str:
    """Runs a step, echoing its output, and returns that output."""
    completed = subprocess.run(
        command, cwd=DESKTOP, capture_output=True, text=True, errors="replace", check=False
    )
    output = completed.stdout + completed.stderr
    sys.stdout.write(output)
    if completed.returncode != 0:
        raise SystemExit(f"FAIL {' '.join(command)} exited with {completed.returncode}")
    return output


def newest_glibc() -> tuple[int, ...]:
    symbols = subprocess.run(
        ["objdump", "-T", str(EXE)], capture_output=True, text=True, check=True
    ).stdout
    versions = {tuple(map(int, v.split("."))) for v in re.findall(r"GLIBC_(\d+(?:\.\d+)+)", symbols)}
    return max(versions)


def placeholders() -> int:
    return EXE.read_bytes().count(PLACEHOLDER)


def main() -> int:
    formats = sys.argv[1:]
    unknown = [f for f in formats if f not in FORMATS]
    if not formats or unknown:
        raise SystemExit(f"usage: desktop_bundle.py {{{','.join(FORMATS)}}}...  (unknown: {unknown})")
    if not (ROOT / "web" / "dist" / "index.html").is_file():
        raise SystemExit("FAIL web/dist/index.html is missing; run `pnpm --dir web build` first")

    # `target/release/uguisu-desktop` is a hard link to cargo's copy under
    # `deps/`, so an unrestored patch lives in both. Cleaning the one crate
    # forces a new link without rebuilding its dependencies.
    run("cargo", "clean", "--release", "-p", "uguisu-desktop")
    run("cargo", "tauri", "build", "--no-bundle")
    if placeholders() != 1:
        raise SystemExit(f"FAIL {EXE} does not carry exactly one {PLACEHOLDER.decode()}")
    if sys.platform.startswith("linux"):
        newest = newest_glibc()
        if newest > GLIBC_FLOOR:
            floor = ".".join(map(str, GLIBC_FLOOR))
            raise SystemExit(
                f"FAIL {EXE} needs glibc {'.'.join(map(str, newest))}; the floor is {floor}. "
                "Build Linux packages on Ubuntu 22.04."
            )

    if "appimage" in formats:
        subprocess.run(TOOLS, check=True)
    log = run("cargo", "tauri", "bundle", "--bundles", ",".join(formats))
    if "Failed to add bundler type" in log:
        raise SystemExit("FAIL a package was built without its format recorded in the binary")
    if placeholders() != 1:
        raise SystemExit(f"FAIL the bundler did not restore {EXE}; the next bundle run would mislabel")
    if "appimage" in formats:
        # Every tool was put there and verified first; a download now would
        # mean the bundler reached for something unpinned.
        if "Downloading" in log:
            raise SystemExit("FAIL the AppImage bundler downloaded a tool that was not pinned")
        subprocess.run([*TOOLS, "--verify"], check=True)
    print(f"BUNDLED {' '.join(formats)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
