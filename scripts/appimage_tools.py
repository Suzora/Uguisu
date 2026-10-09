#!/usr/bin/env python3
"""Put the AppImage build tools where the bundler looks, pinned by SHA-256.

The pinned tauri-bundler fetches five tools the first time it builds an
AppImage — two plugin scripts from a `master` branch and a plugin from a
`continuous` release among them — and verifies none of them. It only fetches
a tool that is not already in its tools directory, which
`bundle.useLocalToolsDir` makes `target/.tauri/`. So this writes each tool
there from a fixed source, checks its hash, and refuses a file that is already
there with the wrong one. The bundler then has nothing to download.

    appimage_tools.py            fetch what is missing, verify everything
    appimage_tools.py --verify   verify only; fetch nothing

A changed checksum is a failure, never a warning: that is how an upstream
swap is noticed. Updating a pin means changing the URL and the hash here
together, in a reviewed commit.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# name in the tools directory -> (fixed source, sha256)
TOOLS = {
    # A named release in Tauri's mirror; the tag is not a version, so the hash
    # is what fixes it.
    "AppRun-x86_64": (
        "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64",
        "f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f",
    ),
    "linuxdeploy-x86_64.AppImage": (
        "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/linuxdeploy-x86_64.AppImage",
        "e762bea85c8eb0d4b3508d46e5c1f037f717d0f9303ae3b4aafc8b04991fa1ef",
    ),
    # The bundler reads these from `master`; a commit URL cannot change.
    "linuxdeploy-plugin-gtk.sh": (
        "https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gtk/"
        "dda522bce37387f1b853d9095713bfaa924c8423/linuxdeploy-plugin-gtk.sh",
        "7804c9eef13e59bf2783aad9882ef9db8f3f3f9e8d631874b1d348d550a3693f",
    ),
    "linuxdeploy-plugin-gstreamer.sh": (
        "https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gstreamer/"
        "2a2e67491c32995a3f279ad0ecbe77abd512b42a/linuxdeploy-plugin-gstreamer.sh",
        "c107b49d84edbffc6ab226ed1007e0626a4f7aa2c3a36b7782bef62351d49e94",
    ),
    # The bundler reads this from `continuous`; this is the latest dated tag.
    "linuxdeploy-plugin-appimage.AppImage": (
        "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/"
        "1-alpha-20250213-1/linuxdeploy-plugin-appimage-x86_64.AppImage",
        "992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3",
    ),
}


# After fetching `linuxdeploy`, the bundler zeroes bytes 8..11 — the AppImage
# magic `AI\x02` — on every run, so desktop integration does not pick it up
# (`linuxdeploy.rs`, "should prevent linuxdeploy to be detected"). Those three
# bytes may be either value; every other byte is held to the pin.
MAGIC = slice(8, 11)
MAGIC_CLEARED = {"linuxdeploy-x86_64.AppImage"}


def digest(path: Path) -> str:
    data = bytearray(path.read_bytes())
    if path.name in MAGIC_CLEARED:
        if data[MAGIC] not in (b"AI\x02", b"\0\0\0"):
            return f"(bytes 8..11 are {bytes(data[MAGIC])!r}, neither the magic nor zeros)"
        data[MAGIC] = b"AI\x02"
    return hashlib.sha256(data).hexdigest()


def fetch(url: str, want: str, dest: Path) -> None:
    fd, name = tempfile.mkstemp(prefix=f".{dest.name}.", dir=dest.parent)
    temporary = Path(name)
    try:
        with os.fdopen(fd, "wb") as out, urllib.request.urlopen(url, timeout=120) as response:
            while block := response.read(1 << 20):
                out.write(block)
            out.flush()
            os.fsync(out.fileno())
        got = digest(temporary)
        if got != want:
            raise SystemExit(f"FAIL {dest.name}: {url}\n  expected sha256 {want}\n  got      sha256 {got}")
        temporary.chmod(0o755)
        temporary.replace(dest)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dir", type=Path, default=ROOT / "target" / ".tauri")
    parser.add_argument("--verify", action="store_true", help="check what is there; download nothing")
    args = parser.parse_args()
    args.dir.mkdir(parents=True, exist_ok=True)

    fetched = 0
    for name, (url, want) in TOOLS.items():
        path = args.dir / name
        if path.exists():
            got = digest(path)
            if got != want:
                print(
                    f"FAIL {path}\n  expected sha256 {want}\n  got      sha256 {got}\n"
                    "  A tool the bundler would run has changed. Delete it only after finding out why."
                )
                return 1
        elif args.verify:
            print(f"FAIL {path} is missing")
            return 1
        else:
            fetch(url, want, path)
            fetched += 1
    print(f"{len(TOOLS)} tools verified, {fetched} fetched")
    return 0


if __name__ == "__main__":
    sys.exit(main())
