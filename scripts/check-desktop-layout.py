#!/usr/bin/env python3
"""Refuse desktop packaging files that disagree with each other.

Each of these would build fine and fail on a user's machine:

- The Flatpak installs the UI somewhere other than `lib/<productName>/web`.
  Tauri looks only at `<exe>/../lib/<productName>`, and the shell only for a
  `web` directory there, so the app would start with no interface.
- `bundle.resources` stops mapping `web/dist` to `web`, and every native
  package loses its interface.
- The app id differs between tauri.conf.json, the Flatpak manifest, the
  AppStream metadata and the desktop entry, so the desktop cannot tie the
  window, the launcher and the store listing together; or the shell's own
  copy of it, which places the log file, drifts from tauri.conf.json.
- The Flatpak is built without `--no-default-features --features flatpak`,
  and the folder picker is the GTK chooser, which inside the sandbox can only
  show the sandbox (or, with both backends on, rfd refuses to build).
- The Flatpak asks for a `--filesystem` permission (ADR 0044: the archive is
  reached through the portal, never by widening the sandbox).

Line-based on purpose: no YAML parser is needed on a machine that runs this.
"""

from __future__ import annotations

import json
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / "desktop/src-tauri/tauri.conf.json"
FLATPAK = ROOT / "desktop/flatpak"


def main() -> int:
    conf = json.loads(TAURI.read_text(encoding="utf-8"))
    app_id = conf["identifier"]
    product = conf["productName"]
    manifest_path = FLATPAK / f"{app_id}.yml"
    metainfo_path = FLATPAK / f"{app_id}.metainfo.xml"
    desktop_path = FLATPAK / f"{app_id}.desktop"
    problems: list[str] = []

    for path in (manifest_path, metainfo_path, desktop_path):
        if not path.is_file():
            problems.append(f"{path.relative_to(ROOT)} is missing (named after the identifier {app_id})")
    if problems:
        return report(problems)

    resources = conf.get("bundle", {}).get("resources", {})
    if resources.get("../../web/dist") != "web":
        problems.append(f"bundle.resources must map ../../web/dist to web; it is {resources!r}")

    manifest = manifest_path.read_text(encoding="utf-8")
    declared = re.search(r"^id:\s*(\S+)\s*$", manifest, re.MULTILINE)
    if not declared or declared.group(1) != app_id:
        problems.append(f"the Flatpak manifest's id is {declared and declared.group(1)!r}, not {app_id!r}")
    if f"/app/lib/{product}/web" not in manifest:
        problems.append(f"the Flatpak manifest does not install the UI to /app/lib/{product}/web")
    # rfd refuses both backends at once, so the GTK default must be off too.
    if not re.search(r"--no-default-features\s+--features\s+\S*\bflatpak\b", manifest):
        problems.append(
            "the Flatpak build must pass `--no-default-features --features flatpak` (portal file chooser)"
        )
    if re.search(r"^\s*-\s*--filesystem", manifest, re.MULTILINE):
        problems.append("the Flatpak manifest grants --filesystem; the archive goes through the portal")

    metainfo = ET.parse(metainfo_path).getroot()
    if metainfo.findtext("id") != app_id:
        problems.append(f"the metainfo <id> is {metainfo.findtext('id')!r}, not {app_id!r}")
    launchable = metainfo.findtext("launchable")
    if launchable != desktop_path.name:
        problems.append(f"the metainfo <launchable> is {launchable!r}, not {desktop_path.name!r}")

    shell = (ROOT / "desktop/src-tauri/src/main.rs").read_text(encoding="utf-8")
    if f'const IDENTIFIER: &str = "{app_id}";' not in shell:
        problems.append(f"desktop/src-tauri/src/main.rs's IDENTIFIER is not {app_id!r}")

    entry = desktop_path.read_text(encoding="utf-8")
    if f"Icon={app_id}\n" not in entry:
        problems.append(f"the desktop entry's Icon is not {app_id}")

    if problems:
        return report(problems)
    print(f"desktop layout consistent for {app_id}")
    return 0


def report(problems: list[str]) -> int:
    print("FAIL desktop layout")
    for problem in problems:
        print(f"  {problem}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
