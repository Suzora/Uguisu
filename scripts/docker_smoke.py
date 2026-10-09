#!/usr/bin/env python3
"""Build the Docker image and play the release checklist's container scenarios.

What `check-docker.py` reads, this runs (ADR 0061): the image is built from
the checkout, started on throwaway volumes and a throwaway network, and fed
by a publisher container that serves a feed and a paced, range-capable
enclosure. Checked, in order:

- the image is under 60 MB compressed;
- with no password, `serve` refuses to start (exit 12);
- the first password goes in on stdin and a token comes out as JSON;
- the server becomes healthy, serves the web UI and refuses an anonymous call;
- a stop during start-up ends it promptly under `--init`;
- SIGTERM in the middle of a download exits 0 and parks the job as
  `queued(shutdown)`; a restart resumes it with a Range request and the
  archived bytes are the publisher's;
- after a login with the password and calls with the token, neither appears
  in `docker logs` of a server logging at debug, nor the password in the
  one-shots' output;
- every file the server wrote belongs to uid 65532.

Everything it creates is named `uguisu-smoke-<pid>` and removed at the end,
also after a failure; nothing else is touched. Needs Docker and the network
the build needs, so it runs on request, not in CI.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PYTHON = "python:3.13-slim-trixie"
LIMIT = 60 * 1024 * 1024
MEDIA_BYTES = 4 * 1024 * 1024
PACE = 256 * 1024
PASSWORD = "smoke-password-" + os.urandom(6).hex()

# The publisher: a feed and one enclosure, sent at PACE bytes a second so a
# stop lands mid-transfer, with Range, a strong ETag and Last-Modified so a
# resume is allowed. `/stats` says what was sent and which ranges were asked.
PUBLISHER = r'''
import hashlib, json, os, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SIZE = int(os.environ["MEDIA_BYTES"])
PACE = int(os.environ["PACE"])
BODY = (b"ID3\x04" + bytes(range(256)) * (SIZE // 256 + 1))[:SIZE]
ETAG = '"' + hashlib.sha256(BODY).hexdigest()[:16] + '"'
STATS = {"sent": 0, "ranges": []}
LOCK = threading.Lock()
FEED = """<?xml version="1.0"?>
<rss version="2.0"><channel><title>Smoke Show</title><link>http://publisher.smoke.test:8000/</link>
<description>A feed for the container smoke.</description>
<item><title>Paced Episode</title><guid isPermaLink="false">paced-1</guid>
<pubDate>Tue, 02 Sep 2025 10:00:00 +0000</pubDate>
<enclosure url="http://publisher.smoke.test:8000/media.mp3" length="%d" type="audio/mpeg"/></item>
</channel></rss>""" % SIZE


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path == "/feed.xml":
            body = FEED.encode()
            self.send_response(200)
            self.send_header("content-type", "application/rss+xml")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/stats":
            with LOCK:
                body = json.dumps(STATS).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/media.mp3":
            start = 0
            ranged = self.headers.get("range")
            if ranged and ranged.startswith("bytes="):
                start = int(ranged[6:].split("-")[0])
                with LOCK:
                    STATS["ranges"].append(start)
                self.send_response(206)
                self.send_header("content-range", "bytes %d-%d/%d" % (start, SIZE - 1, SIZE))
            else:
                self.send_response(200)
            self.send_header("content-type", "audio/mpeg")
            self.send_header("content-length", str(SIZE - start))
            self.send_header("accept-ranges", "bytes")
            self.send_header("etag", ETAG)
            self.send_header("last-modified", "Tue, 02 Sep 2025 10:00:00 GMT")
            self.end_headers()
            at = start
            try:
                while at < SIZE:
                    chunk = BODY[at:at + PACE // 8]
                    self.wfile.write(chunk)
                    self.wfile.flush()
                    at += len(chunk)
                    with LOCK:
                        STATS["sent"] += len(chunk)
                    time.sleep(1 / 8)
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.send_response(404)
            self.end_headers()


ThreadingHTTPServer(("0.0.0.0", 8000), Handler).serve_forever()
'''


class Failure(Exception):
    """A step did not do what it promised."""


def docker(*argv: str, stdin: str | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    done = subprocess.run(
        ["docker", *argv],
        input=stdin,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if check and done.returncode != 0:
        raise Failure(f"docker {' '.join(argv)} exited {done.returncode}:\n{done.stderr}{done.stdout}")
    return done


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def until(what: str, predicate, timeout: float = 90.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.5)
    raise Failure(f"timed out waiting for {what}")


class Api:
    def __init__(self, port: int) -> None:
        self.base = f"http://127.0.0.1:{port}"
        self.token: str | None = None

    def call(self, method: str, path: str, body: object = None) -> tuple[int, bytes]:
        headers = {"content-type": "application/json"}
        if self.token:
            headers["authorization"] = f"Bearer {self.token}"
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.base + path, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read()
        except (urllib.error.URLError, ConnectionError, TimeoutError):
            return 0, b""

    def json(self, method: str, path: str, body: object = None, expect: tuple[int, ...] = (200, 201)) -> dict:
        status, data = self.call(method, path, body)
        if status not in expect:
            raise Failure(f"{method} {path} answered {status}: {data[:400]!r}")
        return json.loads(data)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--image", help="test this image instead of building one")
    args = parser.parse_args()

    tag = f"uguisu-smoke-{os.getpid()}"
    image = args.image or f"{tag}:latest"
    data, media, net = f"{tag}-data", f"{tag}-media", f"{tag}-net"
    server, publisher = f"{tag}-server", f"{tag}-publisher"
    scratch = Path(tempfile.mkdtemp(prefix="uguisu-docker-smoke-"))
    port = free_port()
    api = Api(port)
    checks = 0
    volumes = ["-v", f"{data}:/data", "-v", f"{media}:/media/podcasts"]
    serve = [
        "run", "-d", "--init", "--name", server, "--network", net,
        "-p", f"127.0.0.1:{port}:8484", "--stop-timeout", "60",
        "-e", "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS=publisher.smoke.test",
        "-e", "UGUISU_DOWNLOAD_MIN_FREE_BYTES=0",
        "-e", "UGUISU_LOG=debug",
        *volumes, image,
    ]
    try:
        if not args.image:
            docker("build", "-t", image, str(ROOT))
        saved = subprocess.run(["docker", "save", image], capture_output=True, check=True).stdout
        size = len(gzip.compress(saved, compresslevel=6))
        if size >= LIMIT:
            raise Failure(f"the image is {size} bytes compressed, over {LIMIT}")
        checks += 1

        for volume in (data, media):
            docker("volume", "create", volume)
        docker("network", "create", net)

        refused = docker("run", "--rm", *volumes, image, check=False)
        if refused.returncode != 12 or "refusing to bind" not in refused.stderr:
            raise Failure(f"with no password serve exited {refused.returncode}: {refused.stderr}")
        checks += 1

        set_password = docker("run", "--rm", "-i", *volumes, image, "auth", "set-password",
                              "--username", "admin", stdin=PASSWORD + "\n")
        token_create = docker("run", "--rm", *volumes, image, "--json", "auth", "token", "create", "smoke")
        one_shots = set_password.stdout + set_password.stderr + token_create.stdout + token_create.stderr
        created = json.loads(token_create.stdout)
        api.token = created["secret"]
        if not api.token or created["token"]["scope"] != "write":
            raise Failure(f"no write token: {created}")
        checks += 1

        # A stop while the server is still starting: under --init the signal
        # reaches it even before it handles SIGTERM itself.
        docker(*serve)
        started = time.monotonic()
        docker("stop", "-t", "10", server)
        if time.monotonic() - started > 8:
            raise Failure("a stop during start-up waited for the kill")
        docker("rm", server)
        checks += 1

        (scratch / "publisher.py").write_text(PUBLISHER, encoding="utf-8")
        docker("run", "-d", "--name", publisher, "--network", net, "--network-alias", "publisher.smoke.test",
               "-e", f"MEDIA_BYTES={MEDIA_BYTES}", "-e", f"PACE={PACE}",
               "-v", f"{scratch}:/srv:ro", PYTHON, "python", "/srv/publisher.py")
        docker(*serve)
        until("the server to be healthy", lambda: docker(
            "inspect", "--format", "{{.State.Health.Status}}", server).stdout.strip() == "healthy")
        status, page = api.call("GET", "/")
        if status != 200 or b'<div id="app">' not in page:
            raise Failure(f"GET / answered {status}")
        token, api.token = api.token, None
        status, _ = api.call("GET", "/api/v1/podcasts")
        api.token = token
        if status != 401:
            raise Failure(f"an anonymous call answered {status}")
        api.token = None
        api.json("POST", "/api/v1/auth/login", {"username": "admin", "password": PASSWORD})
        api.token = token
        checks += 1

        added = api.json("POST", "/api/v1/podcasts", {"input": "http://publisher.smoke.test:8000/feed.xml"})
        podcast = added["podcast"]["id"]
        api.json("POST", f"/api/v1/podcasts/{podcast}/downloads", {"priority": "normal"})
        jobs = api.json("GET", f"/api/v1/downloads?podcast={podcast}")["jobs"]
        job, episode = jobs[0]["id"], jobs[0]["episode_id"]
        def stats() -> dict:
            asked = "import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:8000/stats').read().decode())"
            return json.loads(docker("exec", publisher, "python", "-c", asked).stdout)

        until("a quarter of the enclosure", lambda: stats()["sent"] >= MEDIA_BYTES // 4)
        stopping = time.monotonic()
        docker("stop", server)
        stopped = docker("inspect", "--format", "{{.State.ExitCode}}", server).stdout.strip()
        if stopped != "0" or time.monotonic() - stopping > 30:
            raise Failure(f"SIGTERM mid-download exited {stopped}:\n{docker('logs', server).stderr}")
        shown = json.loads(docker("run", "--rm", *volumes, image, "--json", "download", "show", job).stdout)
        parked = shown.get("job", shown)
        if parked["state"] != "queued" or parked["state_reason"] != "shutdown":
            raise Failure(f"the stopped download is {parked['state']}({parked['state_reason']})")
        checks += 1

        docker("start", server)
        until("the server to be healthy again", lambda: docker(
            "inspect", "--format", "{{.State.Health.Status}}", server).stdout.strip() == "healthy")
        until("the download to complete", lambda: api.json(
            "GET", f"/api/v1/downloads/{job}")["job"]["state"] == "completed", timeout=120)
        if not any(start > 0 for start in stats()["ranges"]):
            raise Failure(f"the restart did not resume with a Range request: {stats()}")
        until("the archived file", lambda: api.call("GET", f"/api/v1/archive/{episode}/media")[0] == 200)
        _, archived = api.call("GET", f"/api/v1/archive/{episode}/media")
        expected = (b"ID3\x04" + bytes(range(256)) * (MEDIA_BYTES // 256 + 1))[:MEDIA_BYTES]
        if hashlib.sha256(archived).digest() != hashlib.sha256(expected).digest():
            raise Failure(f"the archived file is {len(archived)} bytes and not the publisher's")
        checks += 1

        logs = docker("logs", server)
        logged = logs.stdout + logs.stderr
        if "DEBUG" not in logged:
            raise Failure("the server did not log at debug, so its logs prove nothing")
        if PASSWORD in logged or token in logged:
            raise Failure("docker logs show the password or the token")
        if PASSWORD in one_shots:
            raise Failure("a one-shot printed the password")
        checks += 1

        owners = docker(
            "run", "--rm", "-v", f"{data}:/d:ro", "-v", f"{media}:/m:ro", PYTHON, "python", "-c",
            "import os\nfor root in ('/d', '/m'):\n for d, _, fs in os.walk(root):\n"
            "  for p in [d, *(os.path.join(d, f) for f in fs)]: print(os.stat(p).st_uid, p)",
        ).stdout.split("\n")
        foreign = [line for line in owners if line and not line.startswith("65532 ")]
        if foreign:
            raise Failure(f"files not owned by 65532: {foreign[:5]}")
        checks += 1
    except Failure as failure:
        print(f"FAIL docker smoke after {checks} checks\n{failure}")
        return 1
    finally:
        for container in (server, publisher):
            docker("rm", "-f", container, check=False)
        for volume in (data, media):
            docker("volume", "rm", volume, check=False)
        docker("network", "rm", net, check=False)
        if not args.image:
            docker("image", "rm", image, check=False)
        for leftover in scratch.iterdir():
            leftover.unlink()
        scratch.rmdir()
    print(f"{checks} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
