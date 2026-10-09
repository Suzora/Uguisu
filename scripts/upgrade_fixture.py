#!/usr/bin/env python3
"""Write an upgrade fixture: a data directory as a given `uguisu` binary leaves
it, and what that binary said about it.

    python3 scripts/upgrade_fixture.py --bin <uguisu> --name v0.2.0-rc.1

Run it with the binary of a tagged pre-release and commit the result: every
later build must open that directory, keep what was in it, and say the same
things about it (`crates/uguisu-cli/tests/upgrade.rs`, ADR 0054).

The directory is made the way a person makes theirs, through the CLI: a
podcast is added from a local publisher, both episodes are downloaded and
archived with their sidecars and manifest, the artwork is fetched, a policy
and a setting are stored, a later refresh leaves a candidate duplicate and an
unverified feed announcement, a password is set and a read token issued.
Then the same binary's `--json` output for a dozen read commands is recorded
next to it. Nothing leaves the machine.
"""

from __future__ import annotations

import argparse
import functools
import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

from e2e import PNG, free_port, media

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests" / "fixtures" / "upgrade"

# Test data, committed with the fixture: nothing real is protected by them.
USERNAME = "fixture"
PASSWORD = "upgrade-fixture-password"


class Publisher(http.server.BaseHTTPRequestHandler):
    """The feed, the cover and two enclosures; the feed can be swapped."""

    base = ""
    feed = ""

    def log_message(self, *_args: object) -> None:
        pass

    def _send(self, body: bytes, kind: str) -> None:
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 - http.server's contract
        if self.path == "/feed.xml":
            self._send(Publisher.feed.encode(), "application/rss+xml")
        elif self.path == "/cover.png":
            self._send(PNG, "image/png")
        elif self.path in ("/ep1.wav", "/ep2.wav"):
            self._send(media(int(self.path[3])), "audio/wav")
        else:
            self.send_error(404)


def feed(base: str, *, later: bool) -> str:
    """The first feed, or the one a later refresh sees: episode 2 under a
    rewritten GUID, and a new home announced that does not answer."""
    items = "".join(
        f"<item><title>Episode {n}</title>"
        f'<guid isPermaLink="false">upgrade-{n}{"-rewritten" if later and n == 2 else ""}</guid>'
        f"<pubDate>0{n} Jan 2026 10:00:00 +0000</pubDate>"
        f"<description>Episode {n} of the upgrade fixture.</description>"
        f"<itunes:duration>0:0{n}</itunes:duration>"
        f'<enclosure url="{base}/ep{n}.wav" length="{len(media(n))}" type="audio/wav"/></item>'
        for n in (1, 2)
    )
    moved = f"<itunes:new-feed-url>{base}/moved.xml</itunes:new-feed-url>" if later else ""
    return (
        '<?xml version="1.0"?>'
        '<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">'
        "<channel><title>Upgrade Fixture</title>"
        "<link>https://upgrade.example/</link>"
        "<description>A feed served from this machine.</description>"
        f'<itunes:image href="{base}/cover.png"/>{moved}'
        f"{items}</channel></rss>"
    )


def run(binary: str, env: dict[str, str], *args: str, stdin: str = "") -> str:
    done = subprocess.run(
        [binary, *args],
        env=env,
        input=stdin,
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    if done.returncode != 0:
        raise SystemExit(f"uguisu {' '.join(args)} exited {done.returncode}:\n{done.stderr}")
    return done.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin", required=True, help="the uguisu binary whose data directory to record")
    parser.add_argument("--name", required=True, help="the fixture's name: the pre-release's tag")
    args = parser.parse_args()
    binary = str(Path(args.bin).resolve())
    out = FIXTURES / args.name
    if out.exists():
        raise SystemExit(f"{out} exists; a fixture is never rewritten")

    port = free_port()
    base = f"http://127.0.0.1:{port}"
    Publisher.base = base
    Publisher.feed = feed(base, later=False)
    publisher = http.server.ThreadingHTTPServer(("127.0.0.1", port), functools.partial(Publisher))
    threading.Thread(target=publisher.serve_forever, daemon=True).start()

    work = Path(tempfile.mkdtemp(prefix="uguisu-upgrade-"))
    data = work / "data"
    env = {
        **{k: v for k, v in os.environ.items() if not k.startswith("UGUISU_")},
        "UGUISU_DATA_DIR": str(data),
        "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS": "127.0.0.1",
        "UGUISU_DISCOVERY_APPLE_ENABLED": "false",
        "UGUISU_DOWNLOAD_MIN_FREE_BYTES": "0",
        "UGUISU_LOG": "error",
    }
    try:
        podcast = json.loads(run(binary, env, "--json", "podcast", "add", f"{base}/feed.xml"))["podcast"]["id"]
        run(binary, env, "download", "podcast", podcast)
        run(binary, env, "download", "run", "--until-idle")
        run(binary, env, "archive", "manifest", "write")
        run(binary, env, "archive", "artwork", "fetch", podcast)
        run(binary, env, "archive", "policy", "set", podcast, "--mode", "manual", "--max-backlog", "2")
        run(binary, env, "config", "set", "UGUISU_ARCHIVE_MAX_AGE_DAYS", "30")
        Publisher.feed = feed(base, later=True)
        run(binary, env, "podcast", "refresh", podcast)
        run(binary, env, "auth", "set-password", "--username", USERNAME, stdin=PASSWORD + "\n")
        token = json.loads(run(binary, env, "--json", "auth", "token", "create", "upgrade-fixture", "--scope", "read"))
        episodes = json.loads(run(binary, env, "--json", "archive", "list"))["files"]

        reads = [
            ["podcast", "list"],
            ["podcast", "show", podcast],
            ["archive", "list"],
            ["archive", "policy", "list"],
            ["archive", "manifest", "status"],
            ["archive", "artwork", "show", podcast],
            ["download", "list"],
            ["episode", "duplicates"],
            ["config", "get", "UGUISU_ARCHIVE_MAX_AGE_DAYS"],
            ["auth", "token", "list"],
            *[["archive", "sidecar", "show", f["episode_id"]] for f in episodes],
        ]
        commands = [{"args": r, "output": json.loads(run(binary, env, "--json", *r))} for r in reads]
        version = run(binary, env, "--version").strip()
    finally:
        publisher.shutdown()

    for leftover in ("uguisu.lock", "uguisu.pid", "uguisu.db-wal", "uguisu.db-shm"):
        path = data / leftover
        if path.exists() and (leftover in ("uguisu.lock", "uguisu.pid") or path.stat().st_size == 0):
            path.unlink()
        elif path.exists():
            raise SystemExit(f"{path} is not empty: the binary did not close its database")

    out.mkdir(parents=True)
    shutil.copytree(data, out / "data")
    shutil.rmtree(work)
    commit = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=False
    ).stdout.strip()
    fixture = {
        "name": args.name,
        "version": version,
        "commit": commit,
        "username": USERNAME,
        "password": PASSWORD,
        "token": token["secret"],
        "commands": commands,
    }
    (out / "fixture.json").write_text(json.dumps(fixture, indent=2, sort_keys=True) + "\n")
    print(f"wrote {out.relative_to(ROOT)}: {len(commands)} commands recorded")
    return 0


if __name__ == "__main__":
    sys.exit(main())
