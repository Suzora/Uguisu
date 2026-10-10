#!/usr/bin/env python3
"""Measure the release binary at the scale ADR 0002 and DATA_MODEL.md §6 promise.

    python3 scripts/bench_scale.py            # 100, 1 000 and 10 000 feeds; 100 000 archived files
    python3 scripts/bench_scale.py --quick    # 100 feeds and 1 000 files

`uguisu` and `uguisu serve` are driven from outside, as a user drives them,
against a publisher on this machine; nothing leaves it.

Each feed tier imports N feeds of 50 episodes from OPML, refreshes them cold,
again with nothing changed, and again after every feed gained an episode,
then checks, backs up and vacuums the database, prunes it and rebuilds the
search index. With the scheduler paused, `serve` answers the pages the web UI
asks for, and their latencies are taken against the < 50 ms target. The
archive tier downloads one small file per episode through `serve`, then
verifies the archive light and full (after a byte is flipped and the mtime
put back), looks for orphans, verifies the manifests, reconciles deep and
dry-runs a rebuild from the sidecars, and pages the archive list.

Every step records its wall time, the process's CPU time and peak memory, and
the database and WAL size after it, into `target/bench-scale/<mode>.json` and
a table on stdout. What every run must show anyway is asserted: the episode
counts, a warm refresh that fetched nothing, the added episodes, the flipped
byte found by a full verification only, a clean database and no orphans.

It deletes only the working directory it created under `target/bench-scale`.
"""

from __future__ import annotations

import argparse
import http.client
import http.server
import json
import os
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

from e2e import OWN_GROUP, STOP, Client, Failure, free_port, wait_for

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / os.environ.get("CARGO_TARGET_DIR", "target")
OUT = TARGET / "bench-scale"
EPISODES = 50
SAMPLES = 25
# DATA_MODEL.md §6: list and filter queries under 50 ms on a laptop SSD.
TARGET_MS = 50


def wav(feed: int, episode: int) -> bytes:
    """A valid WAV of four samples that differ per episode, so no two files hash alike."""
    pcm = struct.pack("<II", feed, episode)
    header = b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt "
    header += struct.pack("<IHHIIHH", 16, 1, 1, 8000, 16000, 2, 16)
    return header + b"data" + struct.pack("<I", len(pcm)) + pcm


def feed_xml(base: str, feed: int, episodes: int) -> bytes:
    items = "".join(
        f"<item><title>Show {feed} episode {n}</title>"
        f'<guid isPermaLink="false">s{feed}-e{n}</guid>'
        f"<pubDate>{1 + n % 28:02d} Jan {2000 + n // 28} 10:00:00 +0000</pubDate>"
        f"<description>Episode {n} of show {feed}, about archives and feeds.</description>"
        f'<enclosure url="{base}/m/{feed}/{n}.wav" length="{len(wav(feed, n))}" type="audio/wav"/></item>'
        for n in range(episodes)
    )
    return (
        '<?xml version="1.0"?><rss version="2.0"><channel>'
        f"<title>Show {feed}</title><link>https://bench.example/{feed}</link>"
        f"<description>A synthetic show.</description>{items}</channel></rss>"
    ).encode()


class Publisher(http.server.BaseHTTPRequestHandler):
    """Every feed and enclosure. `generation` 1 gives each feed one more episode.

    A feed closes its connection, since every show is on a host of its own;
    enclosures keep theirs, as one CDN serves a show's episodes.
    """

    base = ""
    generation = 0
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args: object) -> None:
        pass

    def do_GET(self) -> None:  # noqa: N802 - http.server's contract
        parts = self.path.strip("/").split("/")
        if len(parts) == 2 and parts[0] == "f" and parts[1].endswith(".xml"):
            feed = int(parts[1][:-4])
            tag = f'"g{self.generation}-{feed}"'
            self.close_connection = True
            if self.headers.get("if-none-match") == tag:
                self.send_response(304)
                self.send_header("etag", tag)
                self.send_header("connection", "close")
                self.end_headers()
                return
            self._send(feed_xml(self.base, feed, EPISODES + self.generation), "application/rss+xml", tag)
        elif len(parts) == 3 and parts[0] == "m" and parts[2].endswith(".wav"):
            self._send(wav(int(parts[1]), int(parts[2][:-4])), "audio/wav")
        else:
            self.send_error(404)

    def _send(self, body: bytes, kind: str, tag: str | None = None) -> None:
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(body)))
        if tag:
            self.send_header("etag", tag)
        if self.close_connection:
            self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(body)


def opml(base: str, feeds: int) -> bytes:
    outlines = "".join(f'<outline type="rss" text="S{i}" xmlUrl="{base}/f/{i}.xml"/>' for i in range(feeds))
    return f'<?xml version="1.0"?><opml version="2.0"><head/><body>{outlines}</body></opml>'.encode()


@dataclass
class Step:
    tier: str
    name: str
    seconds: float
    cpu_seconds: float | None = None
    peak_mib: float | None = None
    db_mib: float | None = None
    wal_mib: float | None = None
    detail: dict = field(default_factory=dict)


def usage(process: subprocess.Popen) -> tuple[float | None, float | None, int]:
    """Waits for the process; returns its CPU seconds, peak resident MiB and exit code."""
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        process.wait()

        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
                (name, ctypes.c_size_t)
                for name in ("peak", "ws", "qpp", "qp", "qpnp", "qnp", "pf", "peakpf")
            ]

        handle = wintypes.HANDLE(int(process._handle))  # noqa: SLF001 - the only handle there is
        counters = Counters(cb=ctypes.sizeof(Counters))
        ctypes.windll.kernel32.K32GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb)
        times = [wintypes.FILETIME() for _ in range(4)]
        ctypes.windll.kernel32.GetProcessTimes(handle, *map(ctypes.byref, times))

        def seconds(t: wintypes.FILETIME) -> float:
            return ((t.dwHighDateTime << 32) | t.dwLowDateTime) / 1e7

        return seconds(times[2]) + seconds(times[3]), counters.peak / 2**20, process.returncode
    _pid, status, rusage = os.wait4(process.pid, 0)
    process.returncode = os.waitstatus_to_exitcode(status)
    return rusage.ru_utime + rusage.ru_stime, rusage.ru_maxrss / 1024, process.returncode


class Bench:
    def __init__(self, binary: Path, work: Path) -> None:
        self.binary = binary
        self.work = work
        self.steps: list[Step] = []

    def env(self, data: Path) -> dict[str, str]:
        env = {k: v for k, v in os.environ.items() if not k.startswith("UGUISU_")}
        env.update(
            UGUISU_DATA_DIR=str(data),
            UGUISU_HTTP_ALLOW_PRIVATE_HOSTS="127.0.0.1",
            UGUISU_DISCOVERY_APPLE_ENABLED="false",
            UGUISU_DOWNLOAD_MIN_FREE_BYTES="0",
            UGUISU_ARCHIVE_ARTWORK_FETCH="false",
            UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY="16",
            UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY="16",
        )
        return env

    def sizes(self, step: Step, data: Path) -> None:
        for attr, name in (("db_mib", "uguisu.db"), ("wal_mib", "uguisu.db-wal")):
            path = data / name
            setattr(step, attr, round(path.stat().st_size / 2**20, 2) if path.exists() else 0.0)

    def cli(self, tier: str, name: str, data: Path, *args: str, ok: tuple[int, ...] = (0,)) -> object:
        """Runs one `--json` command and records it; returns what it printed."""
        out = self.work / "out.json"
        err = self.work / "err.txt"
        with out.open("wb") as stdout, err.open("wb") as stderr:
            started = time.perf_counter()
            process = subprocess.Popen(
                [str(self.binary), "--json", *args], cwd=self.work, env=self.env(data),
                stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
            )
            cpu, peak, code = usage(process)
            elapsed = time.perf_counter() - started
        if code not in ok:
            raise Failure(
                f"uguisu {' '.join(args)} exited {code}\n"
                f"{err.read_text(errors='replace')[-4000:]}{out.read_text(errors='replace')[-4000:]}"
            )
        step = Step(tier, name, round(elapsed, 3), round(cpu, 3) if cpu is not None else None,
                    round(peak, 1) if peak is not None else None)
        self.sizes(step, data)
        self.steps.append(step)
        print(f"  {name}: {elapsed:.2f} s", flush=True)
        text = out.read_text(encoding="utf-8").strip()
        return json.loads(text) if text else None

    def serve(self, data: Path) -> tuple[subprocess.Popen, Client, float, float]:
        """Starts `serve`; returns it, a client, when it started and how long it took to answer."""
        port = free_port()
        log = (self.work / "serve.log").open("ab")
        started = time.perf_counter()
        # A --web that does not exist serves the API alone.
        process = subprocess.Popen(
            [str(self.binary), "serve", "--bind", f"127.0.0.1:{port}", "--web", str(self.work / "no-ui")],
            cwd=self.work, env=self.env(data), stdin=subprocess.DEVNULL, stdout=log, stderr=log,
            creationflags=OWN_GROUP,
        )
        log.close()
        client = Client(port)
        wait_for(lambda: client.raw("GET", "/api/v1/health")[0] == 200, what="uguisu serve", timeout=600)
        return process, client, started, time.perf_counter() - started

    def stop(self, tier: str, name: str, process: subprocess.Popen, data: Path, started: float) -> None:
        process.send_signal(STOP)
        cpu, peak, code = usage(process)
        if code != 0:
            raise Failure(f"uguisu serve exited {code}\n{(self.work / 'serve.log').read_text(errors='replace')[-4000:]}")
        step = Step(tier, name, round(time.perf_counter() - started, 3), round(cpu, 3) if cpu else None,
                    round(peak, 1) if peak else None)
        self.sizes(step, data)
        self.steps.append(step)

    def record(self, tier: str, name: str, timings: list[float]) -> None:
        timings = sorted(timings)
        p95 = timings[min(len(timings) - 1, round(0.95 * len(timings)) - 1)]
        self.steps.append(Step(tier, f"GET {name}", round(sum(timings) / 1000, 3), detail={
            "requests": len(timings), "median_ms": round(statistics.median(timings), 2),
            "p95_ms": round(p95, 2), "max_ms": round(timings[-1], 2), "target_ms": TARGET_MS,
        }))
        print(f"  GET {name}: median {statistics.median(timings):.1f} ms, p95 {p95:.1f} ms", flush=True)

    def latency(self, tier: str, client: Client, name: str, paths: list[str]) -> None:
        connection = KeptAlive(client.port)
        self.record(tier, name, [connection.get(path)[0] for path in paths])

    def pages(self, tier: str, client: Client, name: str, path: str, key: str) -> list[dict]:
        """Every page of a paged list at the largest page size, timed; returns the rows."""
        connection = KeptAlive(client.port)
        rows: list[dict] = []
        timings, after = [], None
        while True:
            query = f"{path}{'&' if '?' in path else '?'}limit=500" + (f"&after={after}" if after else "")
            elapsed, page = connection.get(query)
            timings.append(elapsed)
            rows += page[key]
            after = page.get("next_after")
            if not after:
                break
        self.record(tier, f"{name} (every page of 500)", timings)
        return rows


class KeptAlive:
    """One connection for many requests, as a browser holds it.

    A new connection per request would measure the client: on Windows,
    Python's socket timeouts cost a timer tick, about 15 ms, now and then.
    """

    def __init__(self, port: int) -> None:
        self.connection = http.client.HTTPConnection("127.0.0.1", port, timeout=600)

    def get(self, path: str) -> tuple[float, dict]:
        """The milliseconds a GET took, and its JSON."""
        return self.call("GET", path, None, 200)

    def call(self, method: str, path: str, body: object, expect: int) -> tuple[float, dict]:
        payload = None if body is None else json.dumps(body).encode()
        headers = {} if body is None else {"content-type": "application/json"}
        started = time.perf_counter()
        self.connection.request(method, path, body=payload, headers=headers)
        response = self.connection.getresponse()
        data = response.read()
        elapsed = (time.perf_counter() - started) * 1000
        if response.status != expect:
            raise Failure(f"{method} {path} answered {response.status}: {data[:2000]!r}")
        return elapsed, json.loads(data) if data else {}


def refresh_counts(report: dict) -> dict[str, int]:
    """Outcomes and added episodes over a `podcast refresh --all --json` report."""
    counts: dict[str, int] = {"added": 0}
    for entry in report["entries"]:
        if "report" not in entry:
            raise Failure(f"refreshing {entry.get('podcast_id')} failed: {json.dumps(entry)[:1000]}")
        outcome = entry["report"]["outcome"]
        kind = outcome if isinstance(outcome, str) else outcome.get("kind") or outcome.get("status")
        counts[kind] = counts.get(kind, 0) + 1
        counts["added"] += entry["report"].get("episodes", {}).get("added", 0)
    return counts


def expect(what: str, got: object, want: object) -> None:
    if got != want:
        raise Failure(f"{what}: got {got!r}, expected {want!r}")


def feeds_tier(bench: Bench, base: str, feeds: int) -> None:
    tier = f"{feeds} feeds"
    print(tier, flush=True)
    data = bench.work / f"feeds-{feeds}"
    subscriptions = bench.work / f"feeds-{feeds}.opml"
    subscriptions.write_bytes(opml(base, feeds))
    if subscriptions.stat().st_size > 1 << 20:
        raise Failure(f"{subscriptions} is over the 1 MiB an import reads")
    Publisher.generation = 0

    imported = bench.cli(tier, "podcast import --apply", data, "podcast", "import", str(subscriptions), "--apply")
    expect("feeds added by the import", imported["counts"]["add"], feeds)
    cold = refresh_counts(bench.cli(tier, "podcast refresh --all (cold)", data, "podcast", "refresh", "--all"))
    expect("episodes added by the cold refresh", cold["added"], feeds * EPISODES)
    warm = refresh_counts(bench.cli(tier, "podcast refresh --all (unchanged)", data, "podcast", "refresh", "--all"))
    expect("episodes added by a refresh of unchanged feeds", warm["added"], 0)
    if warm.get("fetched"):
        raise Failure(f"a refresh of unchanged feeds fetched {warm['fetched']} of them: {warm}")
    Publisher.generation = 1
    more = refresh_counts(bench.cli(tier, "podcast refresh --all (+1 each)", data, "podcast", "refresh", "--all"))
    expect("episodes added when each feed gained one", more["added"], feeds)

    expect("db check", bench.cli(tier, "db check", data, "db", "check")["ok"], True)
    bench.cli(tier, "db backup", data, "db", "backup", str(bench.work / f"backup-{feeds}.db"))
    bench.cli(tier, "db vacuum", data, "db", "vacuum")
    bench.cli(tier, "scheduler maintenance", data, "scheduler", "maintenance")
    bench.cli(tier, "search reindex", data, "search", "reindex")
    bench.cli(tier, "scheduler pause", data, "scheduler", "pause")

    process, client, started, healthy = bench.serve(data)
    bench.steps.append(Step(tier, "serve until healthy", round(healthy, 3)))
    try:
        podcasts = bench.pages(tier, client, "/podcasts", "/api/v1/podcasts", "podcasts")
        expect("podcasts listed", len(podcasts), feeds)
        ids = [row["podcast"]["id"] for row in podcasts]
        picks = [ids[(i * 7919) % len(ids)] for i in range(SAMPLES)]
        bench.latency(tier, client, "/podcasts (first page)", ["/api/v1/podcasts"] * SAMPLES)
        for sort in ("added", "refreshed", "episodes"):
            bench.latency(tier, client, f"/podcasts?sort={sort}", [f"/api/v1/podcasts?sort={sort}"] * SAMPLES)
        bench.latency(tier, client, "/podcasts?q=", [f"/api/v1/podcasts?q=Show%20{i * 37 % feeds}" for i in range(SAMPLES)])
        bench.latency(tier, client, "/podcasts/{id}", [f"/api/v1/podcasts/{p}" for p in picks])
        bench.latency(tier, client, "/podcasts/{id}/episodes", [f"/api/v1/podcasts/{p}/episodes" for p in picks])
        episodes = [client.api("GET", f"/api/v1/podcasts/{p}/episodes")["episodes"][0]["id"] for p in picks]
        bench.latency(tier, client, "/episodes/{id}", [f"/api/v1/episodes/{e}" for e in episodes])
        # The worst case: one word every episode contains, so every row is ranked.
        words = ("episode", "show", "about", "archives", "feeds")
        bench.latency(tier, client, "/search?kind=episodes (a word in every one)", [
            f"/api/v1/search?q={words[i % len(words)]}&kind=episodes" for i in range(SAMPLES)
        ])
        # A show's number, above every episode number: one show's episodes match.
        bench.latency(tier, client, "/search?kind=episodes (one show)", [
            f"/api/v1/search?q={51 + i * 37 % (feeds - 51)}&kind=episodes" for i in range(SAMPLES)
        ])
        for path in ("/status", "/events?limit=50", "/archive/stats", "/downloads/stats", "/archive?limit=50"):
            bench.latency(tier, client, path, [f"/api/v1{path}"] * SAMPLES)
    finally:
        bench.stop(tier, "serve (whole session)", process, data, started)


def archive_tier(bench: Bench, base: str, files: int) -> None:
    tier = f"{files} files"
    print(tier, flush=True)
    feeds = files // EPISODES
    data = bench.work / f"archive-{files}"
    subscriptions = bench.work / f"archive-{files}.opml"
    subscriptions.write_bytes(opml(base, feeds))
    Publisher.generation = 0
    bench.cli(tier, "podcast import --apply", data, "podcast", "import", str(subscriptions), "--apply")
    bench.cli(tier, "podcast refresh --all (cold)", data, "podcast", "refresh", "--all")
    bench.cli(tier, "scheduler pause", data, "scheduler", "pause")

    process, client, started, _healthy = bench.serve(data)
    try:
        queued = time.perf_counter()
        podcasts = [row["podcast"]["id"] for row in bench.pages(tier, client, "/podcasts", "/api/v1/podcasts", "podcasts")]
        # One connection for the lot: 2 000 new ones next to 100 000
        # downloads would run Windows out of ephemeral ports.
        keep = KeptAlive(client.port)
        for podcast in podcasts:
            keep.call("POST", f"/api/v1/podcasts/{podcast}/downloads", {}, 200)
        wait_for(lambda: keep.get("/api/v1/archive/stats")[1].get("total") == files,
                 what=f"{files} archived files", timeout=6 * 3600)
        bench.steps.append(Step(tier, f"download and archive {files} files", round(time.perf_counter() - queued, 3)))
        print(f"  download and archive: {time.perf_counter() - queued:.1f} s", flush=True)
    finally:
        bench.stop(tier, "serve (downloads)", process, data, started)

    light = bench.cli(tier, "archive verify --all", data, "archive", "verify", "--all")
    expect("files a light verification found intact", light["verified"], files)
    victim = next(p for p in sorted((data / "media").rglob("*.wav")))
    stat = victim.stat()
    body = bytearray(victim.read_bytes())
    body[-1] ^= 0xFF
    victim.write_bytes(bytes(body))
    os.utime(victim, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    after_flip = bench.cli(tier, "archive verify --all (after a byte flip)", data, "archive", "verify", "--all")
    expect("files a light verification found wrong after the flip", after_flip["invalid"], 0)
    full = bench.cli(tier, "archive verify --all --full", data, "archive", "verify", "--all", "--full", ok=(1,))
    expect("files a full verification found wrong", full["invalid"], 1)
    bench.cli(tier, "archive orphans", data, "archive", "orphans")
    bench.cli(tier, "archive manifest status", data, "archive", "manifest", "status")
    bench.cli(tier, "archive manifest verify --full (one podcast)", data,
              "archive", "manifest", "verify", "--podcast", podcasts[0], "--full", ok=(0, 1))
    bench.cli(tier, "archive reconcile --deep", data, "archive", "reconcile", "--deep")
    bench.cli(tier, "archive reconcile --rebuild (dry run)", data, "archive", "reconcile", "--rebuild")
    expect("db check", bench.cli(tier, "db check", data, "db", "check")["ok"], True)

    process, client, started, healthy = bench.serve(data)
    bench.steps.append(Step(tier, "serve until healthy", round(healthy, 3)))
    try:
        rows = bench.pages(tier, client, "/archive", "/api/v1/archive", "files")
        expect("archived files listed", len(rows), files)
        # The light passes since the full one must not have cleared its finding.
        expect("invalid files listed", len(KeptAlive(client.port).get("/api/v1/archive/invalid")[1]["files"]), 1)
        bench.latency(tier, client, "/archive?state=invalid", ["/api/v1/archive?state=invalid"] * SAMPLES)
        bench.latency(tier, client, "/archive/invalid", ["/api/v1/archive/invalid"] * SAMPLES)
        bench.latency(tier, client, "/archive/stats", ["/api/v1/archive/stats"] * SAMPLES)
    finally:
        bench.stop(tier, "serve (archive pages)", process, data, started)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--quick", action="store_true", help="100 feeds and 1 000 files")
    args = parser.parse_args()
    tiers, files = ([100], 1000) if args.quick else ([100, 1000, 10000], 100_000)

    subprocess.run(["cargo", "build", "--release", "--locked", "-q", "-p", "uguisu-cli", "--bin", "uguisu"],
                   cwd=ROOT, check=True)
    binary = TARGET / "release" / ("uguisu.exe" if os.name == "nt" else "uguisu")
    OUT.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="work-", dir=OUT))
    port = free_port()
    Publisher.base = f"http://127.0.0.1:{port}"
    publisher = http.server.ThreadingHTTPServer(("127.0.0.1", port), Publisher)
    threading.Thread(target=publisher.serve_forever, daemon=True).start()
    bench = Bench(binary, work)
    try:
        for feeds in tiers:
            feeds_tier(bench, Publisher.base, feeds)
        archive_tier(bench, Publisher.base, files)
    except Failure as failure:
        print(f"FAIL bench-scale\n\n{failure}")
        return 1
    finally:
        publisher.shutdown()
        report = OUT / f"{'quick' if args.quick else 'full'}.json"
        report.write_text(json.dumps([asdict(s) for s in bench.steps], indent=2), encoding="utf-8")
        shutil.rmtree(work, ignore_errors=True)
    for step in bench.steps:
        if "p95_ms" in step.detail:
            measured = f"median {step.detail['median_ms']} ms, p95 {step.detail['p95_ms']} ms"
        else:
            parts = [f"{step.seconds:.2f} s"] + [
                f"{label} {value}{unit}"
                for label, value, unit in (("cpu", step.cpu_seconds, " s"), ("peak", step.peak_mib, " MiB"),
                                           ("db", step.db_mib, " MiB"), ("wal", step.wal_mib, " MiB"))
                if value is not None
            ]
            measured = ", ".join(parts)
        print(f"{step.tier:>12} | {step.name:<46} | {measured}")
    print(f"{len(bench.steps)} steps, report {report}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
