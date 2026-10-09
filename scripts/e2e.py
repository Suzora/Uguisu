#!/usr/bin/env python3
"""Drive the shipped binary and the built web UI end to end over HTTP.

`smoke.py` proves a cold start; this proves the journey a person actually
takes, in three phases:

1. No credential, bound to loopback: the SPA is served, a deep link survives a
   refresh, a podcast is discovered and added, its episodes are downloaded and
   archived, the archived bytes come back with working range requests, the
   stored artwork is reachable, local search finds the episode, and an
   environment-pinned setting is refused with a conflict.
2. A password is set, and everything above is behind it: anonymous reads are
   refused, a cookie opens them again, a mutation needs the CSRF header, a
   token works for the CLI, a read-scoped token cannot mutate, a revoked one
   stops working — and neither the password nor a token secret appears in any
   URL or on any stream the process wrote.
3. A network bind with no credential refuses to start, and says how to
   override it.

Nothing leaves the machine: a local HTTP server stands in for the publisher,
so the run is deterministic and works without a network.
"""

from __future__ import annotations

import argparse
import functools
import http.client
import http.server
import json
import math
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108000000003a7e9b55"
    "0000000a49444154789c6360000000020001"
    "0d0a2db40000000049454e44ae426082"
)


class Failure(Exception):
    """A step did not do what it promised."""


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def media(index: int) -> bytes:
    """A real WAV: a quarter second of a tone, so a browser can decode it.

    Deliberately not random bytes behind an ID3 header. The point of the
    media endpoint is that a browser plays what it serves, and only genuine
    audio proves that.
    """
    rate = 8000
    samples = rate // 4
    pcm = bytearray()
    for n in range(samples):
        value = int(12000 * math.sin(2 * math.pi * (220 * (index + 1)) * n / rate))
        pcm += (value & 0xFFFF).to_bytes(2, "little", signed=False)
    header = (
        b"RIFF"
        + (36 + len(pcm)).to_bytes(4, "little")
        + b"WAVEfmt "
        + (16).to_bytes(4, "little")
        + (1).to_bytes(2, "little")
        + (1).to_bytes(2, "little")
        + rate.to_bytes(4, "little")
        + (rate * 2).to_bytes(4, "little")
        + (2).to_bytes(2, "little")
        + (16).to_bytes(2, "little")
        + b"data"
        + len(pcm).to_bytes(4, "little")
    )
    return bytes(header) + bytes(pcm)


class Publisher(http.server.BaseHTTPRequestHandler):
    """The feed, the cover and two enclosures."""

    base = ""

    def log_message(self, *_args: object) -> None:
        pass

    def _send(self, body: bytes, kind: str) -> None:
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_HEAD(self) -> None:  # noqa: N802 - http.server's contract
        self.do_GET()

    def do_GET(self) -> None:  # noqa: N802 - http.server's contract
        if self.path == "/feed.xml":
            self._send(feed(self.base).encode(), "application/rss+xml")
        elif self.path == "/cover.png":
            self._send(PNG, "image/png")
        elif self.path in ("/ep1.wav", "/ep2.wav"):
            self._send(media(int(self.path[3])), "audio/wav")
        else:
            self.send_error(404)


def feed(base: str) -> str:
    items = "".join(
        f"<item><title>Episode {n}: chapter {n}</title>"
        f'<guid isPermaLink="false">e2e-{n}</guid>'
        f"<pubDate>0{n} Jan 2026 10:00:00 +0000</pubDate>"
        f"<description>A local fixture episode about rust and archives.</description>"
        f'<itunes:duration>0{n}:02:03</itunes:duration>'
        f'<enclosure url="{base}/ep{n}.wav" type="audio/wav"/></item>'
        for n in (1, 2)
    )
    return (
        '<?xml version="1.0"?>'
        '<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">'
        "<channel><title>Uguisu End To End</title>"
        "<link>https://e2e.example/</link>"
        "<description>A fixture feed served from this machine.</description>"
        f'<itunes:image href="{base}/cover.png"/>'
        f"{items}</channel></rss>"
    )


class Client:
    """The requests a browser and the UI make, with the assertions inline."""

    def __init__(self, port: int) -> None:
        self.port = port
        self.checks = 0

    def raw(
        self,
        method: str,
        path: str,
        *,
        body: object = None,
        headers: dict[str, str] | None = None,
        read_body: bool = True,
    ) -> tuple[int, dict[str, str], bytes]:
        """One request. `read_body=False` leaves the body unread, which is the
        only way to look at an event stream's headers without blocking on a
        response that never ends."""
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=120)
        try:
            payload = None
            sent = dict(headers or {})
            if body is not None:
                payload = json.dumps(body).encode()
                sent["content-type"] = "application/json"
            connection.request(method, path, body=payload, headers=sent)
            response = connection.getresponse()
            data = response.read() if read_body else b""
            return response.status, {k.lower(): v for k, v in response.getheaders()}, data
        finally:
            connection.close()

    def api(
        self,
        method: str,
        path: str,
        *,
        body: object = None,
        expect: int = 200,
    ) -> dict:
        status, _headers, data = self.raw(method, path, body=body)
        if status != expect:
            raise Failure(f"{method} {path} answered {status}, expected {expect}\n{data[:2000]!r}")
        self.checks += 1
        if not data:
            return {}
        try:
            return json.loads(data)
        except json.JSONDecodeError as e:
            raise Failure(f"{method} {path} did not answer JSON: {e}\n{data[:2000]!r}") from e

    def expect(self, path: str, **fields: object) -> dict:
        got = self.api("GET", path)
        for key, want in fields.items():
            if got.get(key) != want:
                raise Failure(
                    f"GET {path}: {key} is {got.get(key)!r}, expected {want!r}\n"
                    f"{json.dumps(got)[:2000]}"
                )
        return got


PASSWORD = "e2e-correct-horse"


# How `uguisu serve` is asked to stop. Windows has no SIGTERM — `send_signal`
# would be TerminateProcess, exit 1 — so there it is Ctrl-Break, which reaches
# only a process that leads its own console process group.
STOP = signal.CTRL_BREAK_EVENT if os.name == "nt" else signal.SIGTERM
OWN_GROUP = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0


def start_server(binary: str, env: dict[str, str], bind: str, dist: Path) -> subprocess.Popen:
    return subprocess.Popen(
        [binary, "serve", "--bind", bind, "--web", str(dist)],
        cwd=ROOT,
        env=env,
        creationflags=OWN_GROUP,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
    )


def run_cli(
    binary: str, env: dict[str, str], args: list[str], *, stdin: str = ""
) -> subprocess.CompletedProcess:
    """One CLI invocation.

    A password only ever arrives on standard input: an argument is in every
    process listing and every shell history.
    """
    return subprocess.run(
        [binary, *args],
        cwd=ROOT,
        env=env,
        input=stdin,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=180,
        check=False,
    )


def stop(server: subprocess.Popen) -> str:
    """Stops a server the way `uguisu serve` is stopped and returns everything
    it wrote, so a caller can prove no credential is in it."""
    if server.poll() is None:
        server.send_signal(STOP)
        try:
            server.wait(timeout=60)
        except subprocess.TimeoutExpired:
            server.kill()
    out, err = server.communicate()
    return out + err


def make_token(client: Client, cookie: dict[str, str], csrf: str, scope: str) -> tuple[str, str]:
    """Mints a token over HTTP with the session cookie.

    The CLI can mint one too, but only embedded or with a token it already
    holds, and the server owns the data directory's lock while it runs.
    """
    status, _headers, data = client.raw(
        "POST",
        "/api/v1/auth/tokens",
        body={"name": f"e2e-{scope}", "scope": scope},
        headers={**cookie, "x-uguisu-csrf": csrf},
    )
    if status != 201:
        raise Failure(f"minting a {scope} token answered {status}: {data[:400]!r}")
    body = json.loads(data)
    return body["token"]["id"], body["secret"]


def phase_two(binary: str, env: dict[str, str], dist: Path, client: Client) -> None:
    """Everything phase 1 did, now behind a password."""
    # The first password is set with the server stopped: a running server holds
    # an exclusive lock on the data directory, and `--server` asks for the
    # current password, which does not exist yet.
    setting = run_cli(binary, env, ["auth", "set-password"], stdin=f"{PASSWORD}\n")
    if setting.returncode != 0:
        raise Failure(f"auth set-password exited {setting.returncode}\n{setting.stderr}")
    if PASSWORD in setting.stdout or PASSWORD in setting.stderr:
        raise Failure("auth set-password echoed the password")
    client.checks += 1

    port = free_port()
    client.port = port
    server = start_server(binary, env, f"127.0.0.1:{port}", dist)
    secrets = [PASSWORD]
    written = ""
    try:
        wait_for(
            lambda: client.raw("GET", "/api/v1/health")[0] == 200,
            what="the authenticated server to listen",
            timeout=30,
        )

        # Public: liveness, and the question "what does this server need".
        client.api("GET", "/api/v1/health")
        opening = client.api("GET", "/api/v1/auth/session")
        if not opening.get("auth_required") or opening.get("authenticated"):
            raise Failure(f"the session route says {json.dumps(opening)}")

        # Anonymous, and therefore refused — including a path that does not
        # exist, which must not answer 404 and let a probe enumerate routes.
        for path in ("/api/v1/podcasts", "/api/v1/settings", "/api/v1/nope"):
            status, _headers, data = client.raw("GET", path)
            if status != 401 or b'"unauthenticated"' not in data:
                raise Failure(f"anonymous GET {path} answered {status}: {data[:200]!r}")
        client.api("POST", "/api/v1/scheduler/pause", body={}, expect=401)
        client.checks += 1

        status, headers, data = client.raw(
            "POST", "/api/v1/auth/login", body={"username": "uguisu", "password": PASSWORD}
        )
        if status != 200:
            raise Failure(f"login answered {status}: {data[:400]!r}")
        set_cookie = headers.get("set-cookie", "")
        if "HttpOnly" not in set_cookie or "SameSite=Lax" not in set_cookie:
            raise Failure(f"the session cookie is {set_cookie!r}")
        cookie = {"cookie": set_cookie.split(";", 1)[0]}
        csrf = json.loads(data)["csrf_token"]
        if PASSWORD in data.decode(errors="replace"):
            raise Failure("the login answer repeated the password")
        client.checks += 1

        # A wrong password says nothing an unknown user would not also say.
        status, _headers, refused = client.raw(
            "POST", "/api/v1/auth/login", body={"username": "uguisu", "password": "wrong"}
        )
        unknown = client.raw(
            "POST", "/api/v1/auth/login", body={"username": "nobody", "password": "wrong"}
        )
        if status != 401 or refused != unknown[2]:
            raise Failure(f"the two refusals differ: {refused[:200]!r} vs {unknown[2][:200]!r}")
        client.checks += 1

        # A browser has a cookie and nothing else, so everything it does must
        # work with one: reads, bytes, ranges and the event stream.
        status, _headers, listing = client.raw("GET", "/api/v1/archive", headers=cookie)
        if status != 200:
            raise Failure(f"a cookie read answered {status}")
        archived = json.loads(listing)["files"][0]
        episode, size = archived["episode_id"], archived["size_bytes"]
        status, headers, part = client.raw(
            "GET",
            f"/api/v1/archive/{episode}/media",
            headers={**cookie, "range": "bytes=0-99"},
        )
        if status != 206 or len(part) != 100:
            raise Failure(f"a cookie range answered {status} with {len(part)} bytes")
        if headers.get("content-range") != f"bytes 0-99/{size}":
            raise Failure(f"content-range was {headers.get('content-range')!r}")
        client.checks += 1

        status, headers, _ = client.raw("GET", "/api/v1/events", headers=cookie, read_body=False)
        if status != 200 or not headers.get("content-type", "").startswith("text/event-stream"):
            raise Failure(f"the stream answered {status} as {headers.get('content-type')!r}")
        status, _headers, _ = client.raw("GET", "/api/v1/events", read_body=False)
        if status != 401:
            raise Failure(f"an anonymous stream answered {status}")
        client.checks += 1

        # A cookie alone cannot mutate: another site can make a browser send
        # it, but cannot read the token that must come with it.
        status, _headers, data = client.raw(
            "POST", "/api/v1/scheduler/pause", body={}, headers=cookie
        )
        if status != 403 or b'"csrf_required"' not in data:
            raise Failure(f"a mutation without the CSRF header answered {status}: {data[:200]!r}")
        status, _headers, data = client.raw(
            "POST", "/api/v1/scheduler/pause", body={}, headers={**cookie, "x-uguisu-csrf": csrf}
        )
        if status != 200:
            raise Failure(f"a mutation with the CSRF header answered {status}: {data[:200]!r}")
        client.raw(
            "POST", "/api/v1/scheduler/resume", headers={**cookie, "x-uguisu-csrf": csrf}
        )
        client.checks += 1

        # A token is not a cookie: no browser attaches it, so no CSRF header
        # applies — and a read token stops at the first mutation.
        write_id, write_secret = make_token(client, cookie, csrf, "write")
        _read_id, read_secret = make_token(client, cookie, csrf, "read")
        secrets += [write_secret, read_secret]
        writing = {"authorization": f"Bearer {write_secret}"}
        reading = {"authorization": f"Bearer {read_secret}"}

        status, _headers, _ = client.raw("GET", "/api/v1/podcasts", headers=reading)
        if status != 200:
            raise Failure(f"a read token could not read: {status}")
        status, _headers, data = client.raw(
            "POST", "/api/v1/scheduler/pause", body={}, headers=reading
        )
        if status != 403 or b'"forbidden"' not in data:
            raise Failure(f"a read token mutated: {status} {data[:200]!r}")
        status, _headers, _ = client.raw("POST", "/api/v1/scheduler/pause", body={}, headers=writing)
        if status != 200:
            raise Failure(f"a write token could not mutate: {status}")
        client.raw("POST", "/api/v1/scheduler/resume", headers=writing)
        client.checks += 1

        # Tokens are what automation holds, so the CLI must work with one and
        # never print it back.
        listing = run_cli(
            binary,
            dict(env, UGUISU_TOKEN=write_secret),
            ["--json", "--server", f"http://127.0.0.1:{port}", "podcast", "list"],
        )
        if listing.returncode != 0:
            raise Failure(f"the CLI with a token exited {listing.returncode}\n{listing.stderr}")
        if write_secret in listing.stdout or write_secret in listing.stderr:
            raise Failure("the CLI printed the token it was given")
        anonymous = run_cli(
            binary, env, ["--json", "--server", f"http://127.0.0.1:{port}", "podcast", "list"]
        )
        if anonymous.returncode == 0:
            raise Failure("the CLI read the library with no token at all")
        client.checks += 1

        # Revoked means gone, on the next request.
        status, _headers, _ = client.raw(
            "DELETE",
            f"/api/v1/auth/tokens/{write_id}",
            headers={**cookie, "x-uguisu-csrf": csrf},
        )
        if status != 204:
            raise Failure(f"revoking answered {status}")
        status, _headers, _ = client.raw("GET", "/api/v1/podcasts", headers=writing)
        if status != 401:
            raise Failure(f"a revoked token still answered {status}")
        client.checks += 1

        # Logging out is the same for the cookie.
        client.raw("POST", "/api/v1/auth/logout", headers={**cookie, "x-uguisu-csrf": csrf})
        status, _headers, _ = client.raw("GET", "/api/v1/podcasts", headers=cookie)
        if status != 401:
            raise Failure(f"a logged-out cookie still answered {status}")
        client.checks += 1
    finally:
        written = stop(server)

    for secret in secrets:
        if secret in written:
            raise Failure("the server wrote a credential to its own output")
    client.checks += 1


def phase_three(binary: str, env: dict[str, str], dist: Path, client: Client) -> None:
    """A network bind with no credential is a startup failure, not a warning."""
    fresh = Path(tempfile.mkdtemp(prefix="uguisu-e2e-expose-"))
    exposed = dict(env, UGUISU_DATA_DIR=str(fresh))
    port = free_port()
    try:
        refused = run_cli(
            binary, exposed, ["serve", "--bind", f"0.0.0.0:{port}", "--web", str(dist)]
        )
        if refused.returncode != 12:
            raise Failure(
                f"an exposed bind exited {refused.returncode}, expected 12\n{refused.stderr}"
            )
        lines = refused.stderr.strip().splitlines()
        wanted = f"uguisu serve: refusing to bind 0.0.0.0:{port} without authentication"
        if not lines or lines[0].strip() != wanted:
            raise Failure(f"the refusal reads:\n{refused.stderr}")
        for hint in ("uguisu auth set-password", "--bind 127.0.0.1", "UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1"):
            if hint not in refused.stderr:
                raise Failure(f"the refusal does not say {hint!r}:\n{refused.stderr}")
        client.checks += 1

        # The same bind, deliberately overridden, starts.
        client.port = port
        server = start_server(
            binary,
            dict(exposed, UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE="1"),
            f"0.0.0.0:{port}",
            dist,
        )
        try:
            wait_for(
                lambda: client.raw("GET", "/api/v1/health")[0] == 200,
                what="the overridden bind to listen",
                timeout=30,
            )
        finally:
            written = stop(server)
        # The log is coloured, so the field name and its value are asserted
        # rather than the `name=value` spelling between them.
        if '"disabled"' not in written or "insecure_override" not in written:
            raise Failure(f"an exposed unauthenticated bind logged no warning:\n{written[-2000:]}")
        client.checks += 1
    finally:
        shutil.rmtree(fresh, ignore_errors=True)


def wait_for(predicate, *, what: str, timeout: float = 90.0) -> object:
    deadline = time.monotonic() + timeout
    last: object = None
    while time.monotonic() < deadline:
        try:
            last = predicate()
        except Exception as e:  # the server may not be listening yet
            last = e
        else:
            if last:
                return last
        time.sleep(0.2)
    raise Failure(f"timed out waiting for {what} (last: {last!r})")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", help="an already-built uguisu binary")
    parser.add_argument("--keep-dir", action="store_true")
    args = parser.parse_args()

    dist = ROOT / "web" / "dist"
    if not (dist / "index.html").is_file():
        raise SystemExit("web/dist/index.html is missing: run `pnpm --dir web build` first")

    binary = args.bin
    if not binary:
        subprocess.run(
            ["cargo", "build", "-q", "-p", "uguisu-cli", "--bin", "uguisu"],
            cwd=ROOT,
            check=True,
        )
        binary = str(ROOT / "target" / "debug" / "uguisu")

    publisher_port = free_port()
    api_port = free_port()
    base = f"http://127.0.0.1:{publisher_port}"
    Publisher.base = base
    publisher = http.server.ThreadingHTTPServer(
        ("127.0.0.1", publisher_port), functools.partial(Publisher)
    )
    threading.Thread(target=publisher.serve_forever, daemon=True).start()

    data_dir = Path(tempfile.mkdtemp(prefix="uguisu-e2e-"))
    env = dict(
        os.environ,
        UGUISU_DATA_DIR=str(data_dir),
        UGUISU_HTTP_ALLOW_PRIVATE_HOSTS="127.0.0.1",
        UGUISU_DISCOVERY_APPLE_ENABLED="false",
        UGUISU_DOWNLOAD_MIN_FREE_BYTES="0",
        UGUISU_ARCHIVE_ARTWORK_FETCH="false",
        # Pinned on purpose: the UI must be told a stored value would be
        # ignored rather than offered a control that fails.
        UGUISU_FEED_REFRESH_INTERVAL_SECS="1800",
    )
    server = start_server(binary, env, f"127.0.0.1:{api_port}", dist)
    client = Client(api_port)
    keep = args.keep_dir

    try:
        wait_for(
            lambda: client.raw("GET", "/api/v1/health")[0] == 200,
            what="the server to listen",
            timeout=30,
        )

        # The SPA, its assets, and a deep link that names no file.
        status, headers, page = client.raw("GET", "/")
        if status != 200 or b"<div id=\"app\">" not in page:
            raise Failure(f"GET / answered {status}: {page[:400]!r}")
        if not headers.get("content-type", "").startswith("text/html"):
            raise Failure(f"GET / served {headers.get('content-type')!r}")
        client.checks += 1

        # Every built asset is served as what it is, and the shipped scripts
        # hold one place that can send a bearer: the desktop exchange.
        bearers = 0
        for asset in sorted((dist / "assets").iterdir()):
            status, headers, data = client.raw("GET", f"/assets/{asset.name}")
            if status != 200 or data != asset.read_bytes():
                raise Failure(f"/assets/{asset.name} answered {status}")
            if "immutable" not in headers.get("cache-control", ""):
                raise Failure(f"a hashed asset is not immutable: {headers.get('cache-control')!r}")
            if asset.suffix == ".js":
                if not headers.get("content-type", "").startswith("text/javascript"):
                    raise Failure(f"/assets/{asset.name} served as {headers.get('content-type')!r}")
                bearers += data.count(b"Bearer ")
        if bearers != 1:
            raise Failure(f"the built scripts can send a bearer from {bearers} places, not one")
        client.checks += 1

        status, _headers, deep = client.raw("GET", "/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV")
        if status != 200 or b"<div id=\"app\">" not in deep:
            raise Failure(f"a deep link answered {status}, so a refresh would 404")
        client.checks += 1

        status, _headers, missing = client.raw("GET", "/assets/not-there.js")
        if status != 404 or b'"not_found"' not in missing:
            raise Failure(f"a missing asset answered {status}: {missing[:200]!r}")
        client.checks += 1

        for hostile in ("/../../etc/passwd", "/assets/../../../etc/passwd", "/%2e%2e/etc/passwd"):
            _status, _headers, leaked = client.raw("GET", hostile)
            if b"root:" in leaked:
                raise Failure(f"{hostile} read a file outside the web directory")
        client.checks += 1

        client.api("GET", "/api/v1/nope", expect=404)

        # Discovery: resolve first, then add. A search result is not a podcast.
        resolved = client.api(
            "POST", "/api/v1/discovery/resolve", body={"input": f"{base}/feed.xml"}
        )
        if resolved.get("title") != "Uguisu End To End" or resolved.get("items_with_media") != 2:
            raise Failure(f"resolve returned {json.dumps(resolved)[:800]}")

        added = client.api(
            "POST", "/api/v1/podcasts", body={"input": f"{base}/feed.xml"}, expect=201
        )
        podcast = added["podcast"]["id"]
        if not added.get("created"):
            raise Failure("the first add did not create a podcast")

        client.expect("/api/v1/podcasts", schema=1)
        detail = client.expect(f"/api/v1/podcasts/{podcast}", episodes_total=2)
        if detail["source"]["fetch"]["state"] != "fetched":
            raise Failure(f"the feed was not fetched: {detail['source']['fetch']}")

        page = client.api("GET", f"/api/v1/podcasts/{podcast}/episodes")
        episodes = [e["id"] for e in page["episodes"]]
        if len(episodes) != 2:
            raise Failure(f"expected 2 episodes, got {len(episodes)}")

        # Download both, and wait for the archive to register them.
        summary = client.api("POST", f"/api/v1/podcasts/{podcast}/downloads", body={"priority": "high"})
        if summary["created"] != 2:
            raise Failure(f"expected 2 jobs, got {json.dumps(summary)}")

        wait_for(
            lambda: client.api("GET", "/api/v1/archive/stats").get("total") == 2,
            what="both episodes to be archived",
        )
        client.expect("/api/v1/archive/stats", total=2)

        # The bytes a browser plays.
        archived = client.api("GET", f"/api/v1/archive/{episodes[0]}")
        size = archived["size_bytes"]
        status, headers, body = client.raw("GET", f"/api/v1/archive/{episodes[0]}/media")
        if status != 200 or len(body) != size:
            raise Failure(f"media answered {status} with {len(body)} of {size} bytes")
        if headers.get("content-type") != "audio/wav":
            raise Failure(f"media served {headers.get('content-type')!r}")
        if headers.get("accept-ranges") != "bytes":
            raise Failure("media does not advertise ranges, so seeking would not work")
        etag = headers.get("etag")
        client.checks += 1

        status, headers, part = client.raw(
            "GET", f"/api/v1/archive/{episodes[0]}/media", headers={"range": "bytes=100-199"}
        )
        if status != 206 or part != body[100:200]:
            raise Failure(f"a range answered {status} with {len(part)} bytes")
        if headers.get("content-range") != f"bytes 100-199/{size}":
            raise Failure(f"content-range was {headers.get('content-range')!r}")
        client.checks += 1

        status, headers, _ = client.raw(
            "GET", f"/api/v1/archive/{episodes[0]}/media", headers={"range": f"bytes={size}-"}
        )
        if status != 416 or headers.get("content-range") != f"bytes */{size}":
            raise Failure(f"a range past the end answered {status}")
        client.checks += 1

        status, _headers, empty = client.raw(
            "GET", f"/api/v1/archive/{episodes[0]}/media", headers={"if-none-match": etag or ""}
        )
        if status != 304 or empty:
            raise Failure(f"a matching validator answered {status} with {len(empty)} bytes")
        client.checks += 1

        # Artwork the browser can show without touching the publisher's CDN.
        fetched = client.api("POST", f"/api/v1/podcasts/{podcast}/artwork/fetch", body={})
        if fetched.get("state") != "fetched":
            raise Failure(f"artwork fetch returned {json.dumps(fetched)[:400]}")
        status, headers, image = client.raw("GET", f"/api/v1/podcasts/{podcast}/artwork/image")
        if status != 200 or image != PNG:
            raise Failure(f"the artwork answered {status} with {len(image)} bytes")
        if headers.get("content-type") != "image/png":
            raise Failure(f"the artwork served {headers.get('content-type')!r}")
        client.checks += 1

        # Local search over what was just added.
        client.api("POST", "/api/v1/search/reindex")
        found = wait_for(
            lambda: client.api("GET", "/api/v1/search?q=chapter&kind=episodes"),
            what="the search index to answer",
        )
        if found["outcome"] != "ok" or not found["episodes"]:
            raise Failure(f"search returned {json.dumps(found)[:800]}")
        client.expect("/api/v1/search?q=&kind=all", outcome="empty_query")

        # The setting the environment pins must be refused, not silently kept.
        settings = client.api("GET", "/api/v1/settings")
        pinned = next(
            k for k in settings["keys"] if k["key"] == "UGUISU_FEED_REFRESH_INTERVAL_SECS"
        )
        if pinned["origin"] != "env" or pinned["value"] != "1800":
            raise Failure(f"the pinned key reads {json.dumps(pinned)}")
        refused = client.api(
            "PUT",
            "/api/v1/settings/UGUISU_FEED_REFRESH_INTERVAL_SECS",
            body={"value": "900"},
            expect=409,
        )
        if refused["error"]["kind"] != "conflict":
            raise Failure(f"a pinned write was answered {json.dumps(refused)}")

        # Service controls the UI offers.
        client.expect("/api/v1/scheduler", enabled=True, running=True)
        client.api("POST", "/api/v1/scheduler/pause", body={"reason": "end-to-end run"})
        client.expect("/api/v1/scheduler", paused=True, paused_reason="end-to-end run")
        client.api("POST", "/api/v1/scheduler/resume")
        client.expect("/api/v1/scheduler", paused=False)
        client.api("POST", "/api/v1/scheduler/maintenance")

        events = client.api("GET", "/api/v1/events?limit=200")
        kinds = {e["kind"] for e in events["events"]}
        for wanted in ("podcast.added", "download.completed", "archive.registered"):
            if wanted not in kinds:
                raise Failure(f"the event log is missing {wanted}: {sorted(kinds)}")
        client.checks += 1

        # A clean shutdown, the way `uguisu serve` is stopped.
        server.send_signal(STOP)
        code = server.wait(timeout=60)
        if code != 0:
            out, err = server.communicate()
            raise Failure(f"serve exited {code}\n--- stdout ---\n{out}\n--- stderr ---\n{err}")
        client.checks += 1

        # Phase 2: the same archive, now behind a password.
        phase_two(binary, env, dist, client)

        # Phase 3: a bind that reaches the network without one.
        phase_three(binary, env, dist, client)
    except Exception as e:
        keep = True
        if server.poll() is None:
            server.kill()
        out, err = server.communicate()
        print(f"FAIL e2e after {client.checks} checks")
        print(f"  data dir: {data_dir}")
        print()
        print(e)
        if err.strip():
            print("--- serve stderr ---")
            print(err[-8000:])
        return 1
    finally:
        publisher.shutdown()
        if server.poll() is None:
            server.kill()
        if not keep:
            shutil.rmtree(data_dir, ignore_errors=True)
        else:
            print(f"data directory kept: {data_dir}", file=sys.stderr)

    print(f"{client.checks} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
