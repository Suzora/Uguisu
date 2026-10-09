#!/usr/bin/env python3
"""Replace one build of the desktop application with another and keep the data.

    desktop_upgrade.py --cli <uguisu> --before <cmd> --before-version 0.0.1 \\
        --replace '<shell command that installs build B>' \\
        --after <cmd> --after-version 0.1.0

`--before` and `--after` are split the way the platform's own shell splits a
command line, so a Windows path keeps its backslashes and its spaces.

The sequence is a real transition, not two runs of one binary:

1. The standalone `uguisu` CLI creates the data directory and mints a write
   token. So the desktop is first opened on a directory it did not create.
2. Build A starts on it and reports `--before-version`. Through its API it
   subscribes to a local feed, archives both episodes, stores a setting and
   indexes them for search. Then it is closed the way a user closes it.
3. The data directory is fingerprinted: the directory's and database's inodes,
   and the hash of every file under the media root. A file the user put there
   themselves is added first.
4. `--replace` runs. For a package that is the package manager upgrading it
   in place.
5. Build B starts on the same directory and must report `--after-version`,
   which differs from A's. Everything A left must still be there through B's
   API, byte for byte where there are bytes. No inode may have changed, so
   nothing was recreated or moved, and no file under the media root may be
   missing or altered.
6. `uguisu serve` opens the same directory and lists the same library, so the
   desktop's data can be reopened by the server.

With `--login-item kept|removed` (Windows), start at login is switched on in
`desktop.json` before build A first starts, and A must write the Run value
pointing at itself. The argument says what the upgrade does to that value: an
MSI major upgrade keeps it, an uninstall removes it. Either way B must point
it at itself afterwards, and after a removal B must log putting it back. It
refuses to run where a Run value, a Task Manager override or a `desktop.json`
already exists, and removes what the run created.

The token travels only in an `Authorization` header and is never printed.
Nothing leaves the machine: a local HTTP server stands in for the publisher.
"""

from __future__ import annotations

import argparse
import functools
import hashlib
import http.server
import json
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_smoke import (  # noqa: E402
    CLOSED, LAYOUTS, Streams, database_intact, request_close, sandboxed_app,
)
from e2e import OWN_GROUP, STOP, Client, Failure, Publisher, free_port, wait_for  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
RUN = r"Software\Microsoft\Windows\CurrentVersion\Run"
APPROVED = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
SETTING = ("UGUISU_APPLE_COUNTRY", "de")
USER_FILE = "a note the user left here.txt"


class Api:
    """The application's API, as the holder of a write token."""

    def __init__(self, port: int, token: str) -> None:
        self.client = Client(port)
        self.auth = {"authorization": f"Bearer {token}"}

    def call(self, method: str, path: str, body: object = None, expect: int = 200) -> dict:
        status, _headers, data = self.client.raw(method, path, body=body, headers=self.auth)
        if status != expect:
            raise Failure(f"{method} {path} answered {status}, expected {expect}\n{data[:2000]!r}")
        return json.loads(data) if data else {}

    def bytes(self, path: str) -> bytes:
        status, _headers, data = self.client.raw("GET", path, headers=self.auth)
        if status != 200:
            raise Failure(f"GET {path} answered {status}")
        return data


def split(command: str) -> list[str]:
    """A command line, split as the platform's own shell splits it."""
    if os.name != "nt":
        return shlex.split(command)
    import ctypes
    from ctypes import wintypes

    shell32, kernel32 = ctypes.windll.shell32, ctypes.windll.kernel32
    shell32.CommandLineToArgvW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_int)]
    shell32.CommandLineToArgvW.restype = ctypes.POINTER(wintypes.LPWSTR)
    kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    count = ctypes.c_int()
    argv = shell32.CommandLineToArgvW(command, ctypes.byref(count))
    if not argv:
        raise argparse.ArgumentTypeError(f"cannot split {command!r}")
    try:
        return [argv[i] for i in range(count.value)]
    finally:
        kernel32.LocalFree(ctypes.cast(argv, ctypes.c_void_p))


class LoginItem:
    """Start at login, wanted before build A first starts (Windows)."""

    def __init__(self) -> None:
        conf = json.loads((ROOT / "desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
        self.name = conf["productName"]
        self.settings = Path(os.environ["APPDATA"]) / conf["identifier"] / "desktop.json"
        self.made_dir = not self.settings.parent.exists()
        present = [f"HKCU\\{key}\\{self.name}" for key in (RUN, APPROVED) if self.value(key) is not None]
        if self.settings.exists():
            present.append(str(self.settings))
        if present:
            raise Failure(f"--login-item would overwrite what this user already has: {', '.join(present)}")
        self.settings.parent.mkdir(parents=True, exist_ok=True)
        self.settings.write_text(json.dumps({"autostart": True}), encoding="utf-8")

    def value(self, key: str) -> object:
        import winreg

        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key) as handle:
                return winreg.QueryValueEx(handle, self.name)[0]
        except FileNotFoundError:
            return None

    def expect(self, executable: str, build: str) -> None:
        # Quoted with a trailing space: what `auto-launch` writes for a path
        # with no arguments, and what Windows starts from a path with spaces.
        want = f'"{executable}" '
        wait_for(lambda: self.value(RUN) == want, what=f"build {build} to point the Run value at {want}")

    def remove(self) -> None:
        import winreg

        for key in (RUN, APPROVED):
            try:
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key, 0, winreg.KEY_SET_VALUE) as handle:
                    winreg.DeleteValue(handle, self.name)
            except FileNotFoundError:
                pass
        if self.made_dir:
            shutil.rmtree(self.settings.parent, ignore_errors=True)
        else:
            self.settings.unlink(missing_ok=True)


class App:
    """One build of the application, started and closed like a user would."""

    def __init__(self, command: list[str], env: dict[str, str], name: str, layout: str) -> None:
        self.name = name
        self.layout = layout
        self.workdir = tempfile.mkdtemp(prefix="uguisu-upgrade-cwd-")
        self.process = subprocess.Popen(
            [*command, "--print-port"],
            cwd=self.workdir,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
        )
        self.streams = Streams(self.process)
        found = wait_for(
            lambda: self.streams.port() or (self.process.poll() is not None and "exited"),
            what=f"build {name} to announce its port",
        )
        if found == "exited":
            raise Failure(f"build {name} exited before serving\n{self.streams.text()}")
        self.port = int(found)

    def close(self) -> None:
        if self.layout == "flatpak":
            # bwrap forwards no signal; a logout signals the app itself.
            os.kill(sandboxed_app(self.process.pid), signal.SIGTERM)
        else:
            request_close(self.process)
        try:
            code = self.process.wait(timeout=90)
        except subprocess.TimeoutExpired:
            self.process.kill()
            raise Failure(f"build {self.name} did not exit after a close request") from None
        # The pipes may still hold the last lines after the exit.
        deadline = time.monotonic() + 5
        while CLOSED not in self.streams.text() and time.monotonic() < deadline:
            time.sleep(0.1)
        if code != 0 or CLOSED not in self.streams.text():
            raise Failure(f"build {self.name} did not close cleanly ({code})\n{self.streams.text()}")
        shutil.rmtree(self.workdir, ignore_errors=True)

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()


def fingerprint(data_dir: Path) -> dict[str, object]:
    media = data_dir / "media"
    files = {
        str(path.relative_to(media)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(media.rglob("*"))
        if path.is_file()
    }
    return {
        "dir_inode": data_dir.stat().st_ino,
        "db_inode": (data_dir / "uguisu.db").stat().st_ino,
        "media": files,
    }


def version(api: Api) -> str:
    return api.call("GET", "/api/v1/health")["version"]


def populate(api: Api, feed_url: str) -> dict[str, object]:
    added = api.call("POST", "/api/v1/podcasts", body={"input": feed_url}, expect=201)
    podcast = added["podcast"]["id"]
    episodes = sorted(e["id"] for e in api.call("GET", f"/api/v1/podcasts/{podcast}/episodes")["episodes"])
    if len(episodes) != 2:
        raise Failure(f"expected 2 episodes, got {episodes}")
    api.call("POST", f"/api/v1/podcasts/{podcast}/downloads", body={"priority": "high"})
    wait_for(lambda: api.call("GET", "/api/v1/archive/stats").get("total") == 2, what="both episodes archived")
    api.call("PUT", f"/api/v1/settings/{SETTING[0]}", body={"value": SETTING[1]})
    api.call("POST", "/api/v1/search/reindex")
    wait_for(
        lambda: api.call("GET", "/api/v1/search?q=chapter&kind=episodes").get("episodes"),
        what="the search index to find the episodes",
    )
    return observe(api, podcast, episodes)


def observe(api: Api, podcast: str, episodes: list[str]) -> dict[str, object]:
    """Everything the upgrade must keep, as the API reports it."""
    archive = {}
    for episode in episodes:
        row = api.call("GET", f"/api/v1/archive/{episode}")
        body = api.bytes(f"/api/v1/archive/{episode}/media")
        archive[episode] = {
            "path": row["relative_path"],
            "size": row["size_bytes"],
            "served_sha256": hashlib.sha256(body).hexdigest(),
            "sidecar": bool(row.get("sidecar_written_at")),
        }
    setting = next(k for k in api.call("GET", "/api/v1/settings")["keys"] if k["key"] == SETTING[0])
    found = api.call("GET", "/api/v1/search?q=chapter&kind=episodes").get("episodes", [])
    return {
        "podcast": podcast,
        "title": api.call("GET", f"/api/v1/podcasts/{podcast}")["podcast"]["title"],
        "episodes": episodes,
        "archive": archive,
        "setting": (setting["origin"], setting["value"]),
        "search_hits": sorted(hit["episode_id"] for hit in found),
    }


def compare(label: str, before: object, after: object) -> None:
    if before != after:
        raise Failure(
            f"{label} changed across the upgrade\n  before: {json.dumps(before, sort_keys=True)}\n"
            f"  after:  {json.dumps(after, sort_keys=True)}"
        )


def upgrade(args: argparse.Namespace) -> int:
    checks = 0
    if args.data_dir is None:
        data_dir = Path(tempfile.mkdtemp(prefix="uguisu-upgrade-data-"))
    else:
        data_dir = args.data_dir
        data_dir.mkdir(parents=True)
    publisher_port = free_port()
    Publisher.base = f"http://127.0.0.1:{publisher_port}"
    publisher = http.server.ThreadingHTTPServer(("127.0.0.1", publisher_port), functools.partial(Publisher))
    threading.Thread(target=publisher.serve_forever, daemon=True).start()
    # Nothing inherited: a UGUISU_WEB_DIR would serve another UI and a
    # UGUISU_MEDIA_DIR would move the files this compares.
    env = {k: v for k, v in os.environ.items() if not k.startswith("UGUISU_")}
    env.update(
        UGUISU_DATA_DIR=str(data_dir),
        UGUISU_HTTP_ALLOW_PRIVATE_HOSTS="127.0.0.1",
        UGUISU_DISCOVERY_APPLE_ENABLED="false",
        UGUISU_DOWNLOAD_MIN_FREE_BYTES="0",
        UGUISU_ARCHIVE_ARTWORK_FETCH="false",
    )
    running: list[App] = []
    login: LoginItem | None = None
    try:
        minted = subprocess.run(
            [args.cli, "--json", "auth", "token", "create", "upgrade"],
            env=env, capture_output=True, text=True, check=False,
        )
        if minted.returncode != 0:
            raise Failure(f"uguisu auth token create exited {minted.returncode}\n{minted.stderr}")
        token = json.loads(minted.stdout)["secret"]
        checks += 1

        if args.login_item:
            login = LoginItem()
        a = App(args.before, env, "A", args.layout)
        running.append(a)
        api = Api(a.port, token)
        if version(api) != args.before_version:
            raise Failure(f"build A reports {version(api)}, expected {args.before_version}")
        if login is not None:
            login.expect(args.before[0], "A")
            checks += 1
        kept = populate(api, f"{Publisher.base}/feed.xml")
        if not all(row["sidecar"] for row in kept["archive"].values()):
            raise Failure(f"build A archived without sidecars: {kept['archive']}")
        a.close()
        running.remove(a)
        checks += 1

        (data_dir / "media" / USER_FILE).write_text("Uguisu must never remove this.\n", encoding="utf-8")
        disk = fingerprint(data_dir)

        # The same environment the builds get, as an installer would have.
        replaced = subprocess.run(args.replace, shell=True, env=env, check=False)
        if replaced.returncode != 0:
            raise Failure(f"--replace exited {replaced.returncode}: {args.replace}")
        if login is not None:
            left = login.value(RUN)
            if (left is None) != (args.login_item == "removed"):
                raise Failure(f"--login-item {args.login_item}, but after the upgrade the Run value is {left!r}")
        checks += 1

        b = App(args.after, env, "B", args.layout)
        running.append(b)
        api = Api(b.port, token)
        after_version = version(api)
        if after_version != args.after_version or after_version == args.before_version:
            raise Failure(
                f"build B reports {after_version}; expected {args.after_version}, "
                f"different from A's {args.before_version}"
            )
        compare("the library, archive, setting and search index", kept, observe(api, kept["podcast"], kept["episodes"]))
        if login is not None:
            login.expect(args.after[0], "B")
            if args.login_item == "removed" and "login item restored" not in b.streams.text():
                raise Failure("the upgrade removed the Run value and build B did not log restoring it")
            checks += 1
        b.close()
        running.remove(b)
        checks += 1

        now = fingerprint(data_dir)
        compare("the data directory's inode", disk["dir_inode"], now["dir_inode"])
        compare("the database file's inode", disk["db_inode"], now["db_inode"])
        missing_or_changed = {k: v for k, v in disk["media"].items() if now["media"].get(k) != v}
        compare("files under the media root", missing_or_changed, {})
        database_intact(data_dir)
        checks += 1

        port = free_port()
        server = subprocess.Popen(
            [args.cli, "serve", "--bind", f"127.0.0.1:{port}"],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace",
            creationflags=OWN_GROUP,
        )
        try:
            served = Api(port, token)
            wait_for(lambda: served.client.raw("GET", "/api/v1/health")[0] == 200, what="uguisu serve")
            titles = [p["podcast"]["title"] for p in served.call("GET", "/api/v1/podcasts")["podcasts"]]
            if titles != [kept["title"]]:
                raise Failure(f"uguisu serve lists {titles}, expected [{kept['title']!r}]")
            served.call("GET", "/api/v1/status")
        finally:
            # `serve` has no window to close; it is stopped the way a
            # supervisor stops it, and it must not outlive the harness.
            server.send_signal(STOP)
            try:
                out, err = server.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                server.kill()
                out, err = server.communicate()
        if server.returncode != 0:
            raise Failure(f"uguisu serve exited {server.returncode} when asked to stop\n{out}{err}")
        checks += 1
        return checks
    except Failure as failure:
        output = "\n".join(f"--- build {app.name} ---\n{app.streams.text()}" for app in running)
        raise Failure(f"{failure}\n{output}") from None
    finally:
        for app in running:
            app.kill()
        if login is not None:
            login.remove()
        publisher.shutdown()
        shutil.rmtree(data_dir, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", required=True, help="the standalone uguisu binary")
    parser.add_argument("--before", required=True, type=split, help="how to start build A")
    parser.add_argument("--before-version", required=True)
    parser.add_argument("--replace", required=True, help="shell command that puts build B in place")
    parser.add_argument("--after", required=True, type=split, help="how to start build B")
    parser.add_argument("--after-version", required=True)
    parser.add_argument("--layout", choices=LAYOUTS, default="unpackaged", help="what is installed")
    parser.add_argument(
        "--data-dir", type=Path,
        help="a directory to create for the data, where the builds can reach it; it must not exist",
    )
    parser.add_argument(
        "--login-item", choices=("kept", "removed"),
        help="switch start at login on before build A; what the upgrade does to the Run value (Windows, release builds)",
    )
    args = parser.parse_args()
    if args.data_dir is not None and args.data_dir.exists():
        parser.error(f"--data-dir {args.data_dir} exists; it is created and removed by this run")
    if args.login_item and os.name != "nt":
        parser.error("--login-item checks the Windows Run value")
    try:
        checks = upgrade(args)
    except Failure as failure:
        print(f"FAIL desktop upgrade\n\n{failure}")
        return 1
    print(f"{checks} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
