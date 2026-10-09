#!/usr/bin/env python3
"""Refuse a version that differs between the places a release carries it.

The root `Cargo.toml`'s `[workspace.package] version` is the version. The
desktop shell is a separate cargo workspace and cannot inherit it, the web UI
has its own `package.json`, and AppStream keeps the version in the Flatpak
metainfo's `<releases>`, so each of those holds a mirror. This reads every
mirror and fails on any difference. It never rewrites a file.

`tauri.conf.json` must not have a `version` of its own: without one, Tauri
takes the crate's, which is how NSIS, MSI, deb, rpm and the AppImage name
all follow the mirror checked here.

MSI is the strictest consumer: a plain `major.minor.patch` with major and
minor at most 255 and patch at most 65535, and no pre-release suffix. A
version it rejects fails here rather than at the end of a Windows build.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METAINFO = ROOT / "desktop/flatpak/io.github.suzora.Uguisu.metainfo.xml"


def toml(path: str) -> dict:
    return tomllib.loads((ROOT / path).read_text(encoding="utf-8"))


def main() -> int:
    authority = toml("Cargo.toml")["workspace"]["package"]["version"]
    problems: list[str] = []

    mirrors = {
        "desktop/Cargo.toml [workspace.package]": toml("desktop/Cargo.toml")["workspace"]["package"][
            "version"
        ],
        "web/package.json": json.loads((ROOT / "web/package.json").read_text(encoding="utf-8"))[
            "version"
        ],
    }
    releases = ET.parse(METAINFO).getroot().findall("./releases/release")
    if releases:
        # AppStream lists releases newest first.
        mirrors[f"{METAINFO.relative_to(ROOT)} <release>"] = releases[0].get("version")
    else:
        problems.append(f"{METAINFO.relative_to(ROOT)} lists no <release>")

    for place, version in mirrors.items():
        if version != authority:
            problems.append(f"{place} says {version!r}; Cargo.toml says {authority!r}")

    if toml("desktop/src-tauri/Cargo.toml")["package"].get("version") != {"workspace": True}:
        problems.append("desktop/src-tauri/Cargo.toml must take `version.workspace = true`")
    conf = json.loads((ROOT / "desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    if "version" in conf:
        problems.append("desktop/src-tauri/tauri.conf.json has a `version`; remove it so the crate's is used")

    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", authority)
    if not match:
        problems.append(f"{authority!r} is not plain major.minor.patch, which MSI requires")
    else:
        major, minor, patch = map(int, match.groups())
        if major > 255 or minor > 255 or patch > 65535:
            problems.append(f"{authority!r} exceeds MSI's limits (255.255.65535)")

    if problems:
        print("FAIL version")
        for problem in problems:
            print(f"  {problem}")
        return 1
    print(f"version {authority} in {len(mirrors) + 1} places")
    return 0


if __name__ == "__main__":
    sys.exit(main())
