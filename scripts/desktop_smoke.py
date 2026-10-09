#!/usr/bin/env python3
"""Start an installed Uguisu desktop application and prove it is the real one.

Usage:

    desktop_smoke.py --expect-spa web/dist --layout deb -- /usr/bin/uguisu-desktop
    desktop_smoke.py --self-test

The command after `--` is launched with `--print-port` appended. It may be a
wrapper such as `flatpak run <app-id>`.

`--close logoff` (Windows) ends the application the way a logoff does rather
than through its window: every top-level window is asked whether the session
may end and told that it ends, and then the process is terminated, as Windows
does once those messages are answered. The engine must have closed by then.

What a pass means, in order: the process stays up and announces a port; the
port is not reachable from outside loopback; `/api/v1/health` answers; `/`, a
deep route and every asset `index.html` references are byte-identical to the
production build given as `--expect-spa`, with the server's CSP attached; the
WebView exchanged its launch credential for a session; a close request ends
the process cleanly; nothing it started is left running; no 64-hex-digit
secret was written to either stream; and the database it leaves behind passes
SQLite's integrity check.

The SPA comparison is the reason this exists. The shell prefers
`UGUISU_WEB_DIR`, and every cargo build directory already holds a copy of the
UI beside the binary, so a package that shipped without its interface still
served one in every run that did not control both. This refuses to run with
`UGUISU_WEB_DIR` set and starts the application in an empty working
directory, so the only UI it can find is the one the package installed.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import http.client
import os
import re
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))
from e2e import Client, Failure, wait_for  # noqa: E402

LAYOUTS = ("deb", "rpm", "appimage", "flatpak", "nsis", "msi", "unpackaged")
# What the bundler writes into the binary it packages for each format. The
# Flatpak and an unpackaged build are never patched.
BUNDLE_TYPE = {
    "deb": b"__TAURI_BUNDLE_TYPE_VAR_DEB",
    "rpm": b"__TAURI_BUNDLE_TYPE_VAR_RPM",
    "appimage": b"__TAURI_BUNDLE_TYPE_VAR_APP",
    "nsis": b"__TAURI_BUNDLE_TYPE_VAR_NSS",
    "msi": b"__TAURI_BUNDLE_TYPE_VAR_MSI",
    "flatpak": b"__TAURI_BUNDLE_TYPE_VAR_UNK",
    "unpackaged": b"__TAURI_BUNDLE_TYPE_VAR_UNK",
}
ANY_BUNDLE_TYPE = re.compile(rb"__TAURI_BUNDLE_TYPE_VAR_[A-Z]{3}")

# `vite build` writes `/assets/<name>-<hash>.js`; the dev server's page loads
# `/@vite/client` instead. Either one tells the two apart.
HASHED_SCRIPT = re.compile(r'src="(/assets/[^"/]+-[A-Za-z0-9_-]{6,}\.js)"')
REFERENCE = re.compile(r'(?:src|href)="(/[^"]+)"')
DEV_MARKER = "/@vite/client"
# `crates/uguisu-server/src/headers.rs` builds a longer policy; these are the
# parts that would be missing if something other than the server answered.
CSP_REQUIRED = ("default-src 'self'", "script-src 'self'", "frame-ancestors 'none'")
DEEP_ROUTE = "/library"
SECRET = re.compile(r"\b[0-9a-f]{64}\b")
ANSI = re.compile(r"\x1b\[[0-9;]*m")
# The engine's own statement of where it opened, so the default location is
# read from the application rather than guessed per platform.
OPENED = re.compile(r"engine open data_dir=(.*?) media_dir=")
BOOTSTRAPPED = "session exchanged for a launch token"
# Logged after `engine.close()` returns, so it proves the database was closed
# rather than abandoned — which an exit code alone does not.
CLOSED = "server stopped"

Fetch = Callable[[str], "tuple[int, dict[str, str], bytes]"]


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@dataclass
class Spa:
    """A production build, as bytes the application must serve unchanged."""

    index: bytes
    assets: dict[str, str] = field(default_factory=dict)


def expected_spa(dist: Path) -> Spa:
    """Reads the build the package should contain, and refuses anything that
    is not a production build."""
    index_path = dist / "index.html"
    if not index_path.is_file():
        raise Failure(f"--expect-spa {dist}: there is no index.html")
    index = index_path.read_bytes()
    text = index.decode("utf-8", errors="replace")
    if DEV_MARKER in text:
        raise Failure(f"--expect-spa {dist}: index.html loads {DEV_MARKER}; that is the dev server")
    if not HASHED_SCRIPT.search(text):
        raise Failure(f"--expect-spa {dist}: index.html references no hashed /assets/*.js")
    assets: dict[str, str] = {}
    for reference in REFERENCE.findall(text):
        file = dist / reference.lstrip("/")
        if not file.is_file():
            raise Failure(f"--expect-spa {dist}: index.html references {reference}, which is missing")
        assets[reference] = sha(file.read_bytes())
    return Spa(index=index, assets=assets)


def check_spa(fetch: Fetch, spa: Spa, layout: str) -> int:
    """Proves the served UI is `spa`, byte for byte. Returns the checks made."""
    checks = 0

    def mismatch(what: str, path: str, want: str, got: bytes, status: int) -> Failure:
        return Failure(
            f"[{layout}] {what} GET {path}\n"
            f"  status:   {status}\n"
            f"  expected: sha256 {want}\n"
            f"  actual:   sha256 {sha(got)}\n"
            f"  begins:   {got[:160]!r}"
        )

    for path in ("/", DEEP_ROUTE):
        status, headers, body = fetch(path)
        if status != 200 or not headers.get("content-type", "").startswith("text/html"):
            raise mismatch("not the SPA:", path, sha(spa.index), body, status)
        if body != spa.index:
            raise mismatch("a different index.html:", path, sha(spa.index), body, status)
        policy = headers.get("content-security-policy", "")
        missing = [part for part in CSP_REQUIRED if part not in policy]
        if missing:
            raise Failure(f"[{layout}] GET {path}: CSP lacks {missing}; got {policy!r}")
        checks += 1
    for path, want in spa.assets.items():
        status, _headers, body = fetch(path)
        if status != 200 or sha(body) != want:
            raise mismatch("a different asset:", path, want, body, status)
        checks += 1
    return checks


def check_bundle_type(binary: bytes, layout: str) -> None:
    """The installed binary must say it came from this format, and only that."""
    found = set(ANY_BUNDLE_TYPE.findall(binary))
    if found != {BUNDLE_TYPE[layout]}:
        raise Failure(
            f"[{layout}] the installed binary records {sorted(found)}, "
            f"expected {BUNDLE_TYPE[layout].decode()}"
        )


class Streams:
    """Collects a child's stdout and stderr without ever blocking on either."""

    def __init__(self, process: subprocess.Popen) -> None:
        self.lines: list[str] = []
        self.lock = threading.Lock()
        for stream in (process.stdout, process.stderr):
            threading.Thread(target=self._pump, args=(stream,), daemon=True).start()

    def _pump(self, stream) -> None:
        for line in stream:
            with self.lock:
                self.lines.append(line)

    def text(self) -> str:
        with self.lock:
            return "".join(self.lines)

    def port(self) -> int | None:
        match = re.search(r"^port=(\d+)$", self.text(), re.MULTILINE)
        return int(match.group(1)) if match else None


def outside_address() -> str | None:
    """An address of this machine that is not loopback, if it has one."""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        try:
            probe.connect(("192.0.2.1", 9))  # TEST-NET-1: nothing is sent
        except OSError:
            return None
        address = probe.getsockname()[0]
    return None if address.startswith("127.") else address


def refused(address: str, port: int) -> bool:
    try:
        with socket.create_connection((address, port), timeout=2):
            return False
    except OSError:
        return True


WS_SYSMENU = 0x0008_0000
WS_EX_TOOLWINDOW = 0x0000_0080


def user_closable(visible: bool, style: int, ex_style: int) -> bool:
    """Whether a user could close this top-level window from its title bar.

    Only those get `WM_CLOSE`. tao keeps a visible tool window per thread to
    carry its own events; closing that one destroys it and leaves the event
    loop unable to finish shutting down, which no user can ever do.
    """
    return visible and bool(style & WS_SYSMENU) and not ex_style & WS_EX_TOOLWINDOW


def top_level_windows(pid: int) -> list[tuple[int, bool, int, int]]:
    """Each top-level window of `pid`: handle, visibility, style, extended style."""
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    user32.GetWindowLongW.argtypes = [wintypes.HWND, ctypes.c_int]
    user32.GetWindowLongW.restype = ctypes.c_long
    GWL_STYLE, GWL_EXSTYLE = -16, -20
    found: list[tuple[int, bool, int, int]] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def visit(hwnd, _lparam):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid:
            found.append((
                hwnd,
                bool(user32.IsWindowVisible(hwnd)),
                user32.GetWindowLongW(hwnd, GWL_STYLE) & 0xFFFF_FFFF,
                user32.GetWindowLongW(hwnd, GWL_EXSTYLE) & 0xFFFF_FFFF,
            ))
        return True

    user32.EnumWindows(visit, 0)
    return found


def close_windows(pid: int) -> None:
    """Asks every window of `pid` a user could close to close, as the user would.

    A release build uses the Windows GUI subsystem, so it has no console for a
    control event to reach; `WM_CLOSE` is the request it actually handles.
    """
    import ctypes

    WM_CLOSE = 0x0010
    handles = [hwnd for hwnd, *shape in top_level_windows(pid) if user_closable(*shape)]
    if not handles:
        raise Failure(f"no window of pid {pid} has a close box to press")
    for hwnd in handles:
        ctypes.windll.user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)


def end_session(pid: int) -> None:
    """Tells every top-level window of `pid` that the user is logging off.

    Every one, not only those a user could close: tao answers `WM_ENDSESSION`
    in its event-target window, and Windows sends it to all of them. Each
    message is sent and its answer awaited, as Windows does before it ends the
    process; a window gone by the time its turn comes is skipped.
    """
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    user32.SendMessageTimeoutW.argtypes = [
        wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM,
        wintypes.UINT, wintypes.UINT, ctypes.POINTER(ctypes.c_size_t),
    ]
    user32.SendMessageTimeoutW.restype = ctypes.c_size_t
    WM_QUERYENDSESSION, WM_ENDSESSION, ENDSESSION_LOGOFF, SMTO_BLOCK = 0x0011, 0x0016, 0x8000_0000, 0x0001
    handles = [hwnd for hwnd, *_shape in top_level_windows(pid)]
    if not handles:
        raise Failure(f"pid {pid} has no top-level window to tell")
    for name, message, ending in (("WM_QUERYENDSESSION", WM_QUERYENDSESSION, 0), ("WM_ENDSESSION", WM_ENDSESSION, 1)):
        for hwnd in handles:
            answer = ctypes.c_size_t()
            sent = user32.SendMessageTimeoutW(
                hwnd, message, ending, ENDSESSION_LOGOFF, SMTO_BLOCK, 90_000, ctypes.byref(answer)
            )
            if not sent and user32.IsWindow(hwnd):
                raise Failure(f"window {hwnd:#x} of pid {pid} did not answer {name} within 90 s")


def request_close(process: subprocess.Popen) -> None:
    if os.name == "nt":
        close_windows(process.pid)
    else:
        process.send_signal(signal.SIGTERM)


def sandboxed_app(root: int) -> int:
    """The `uguisu-desktop` process below `root`, by its host PID.

    `flatpak run` becomes bwrap, which runs the app in its own PID namespace
    and does not forward signals: a SIGTERM to it kills the sandbox, not the
    app. A logout signals the app itself, so the Flatpak smoke does too.
    """
    children: dict[int, list[int]] = {}
    for proc in Path("/proc").glob("[0-9]*"):
        try:
            parent = int((proc / "stat").read_text().rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, IndexError):
            continue
        children.setdefault(parent, []).append(int(proc.name))
    pending = list(children.get(root, []))
    while pending:
        pid = pending.pop()
        try:
            argv0 = (Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0", 1)[0]
        except OSError:
            continue
        if os.path.basename(argv0) == b"uguisu-desktop":
            return pid
        pending.extend(children.get(pid, []))
    raise Failure(f"no uguisu-desktop process runs below pid {root}")


def survivors(marker: str) -> list[str]:
    """Processes whose environment still carries this run's data directory.

    Every helper the application starts — WebKit's web and network processes
    included — inherits it, so this finds orphans by ancestry rather than by
    name. Linux only; elsewhere the port check stands in for it.
    """
    found = []
    for proc in Path("/proc").glob("[0-9]*"):
        try:
            environ = (proc / "environ").read_bytes()
        except OSError:
            continue
        if marker.encode() in environ.split(b"\0"):
            cmdline = (proc / "cmdline").read_bytes().replace(b"\0", b" ")
            found.append(f"{proc.name}: {cmdline!r}")
    return found


def database_intact(data_dir: Path) -> None:
    database = data_dir / "uguisu.db"
    if not database.is_file():
        raise Failure(f"{database} does not exist after a clean close")
    # `closing`, because a connection's own `with` ends a transaction and
    # leaves it open, and an open handle keeps Windows from removing the
    # temporary data directory afterwards.
    with contextlib.closing(sqlite3.connect(f"file:{database}?mode=ro", uri=True)) as db:
        verdict = db.execute("PRAGMA integrity_check").fetchone()[0]
        applied = db.execute("SELECT COUNT(*) FROM _sqlx_migrations").fetchone()[0]
    if verdict != "ok":
        raise Failure(f"{database}: integrity_check says {verdict}")
    if applied == 0:
        raise Failure(f"{database}: no migration was recorded")


def opened_data_dir(output: str) -> Path | None:
    match = OPENED.search(ANSI.sub("", output))
    return Path(match.group(1)) if match else None


def smoke(command: list[str], spa: Spa, layout: str, default_data_dir: bool, close: str) -> int:
    if "UGUISU_WEB_DIR" in os.environ:
        raise Failure(
            "UGUISU_WEB_DIR is set. It overrides the bundled UI, so a pass would not "
            "say anything about the package. Unset it."
        )
    workdir = Path(tempfile.mkdtemp(prefix="uguisu-desktop-cwd-"))
    env = {k: v for k, v in os.environ.items() if k != "UGUISU_DATA_DIR"}
    data_dir: Path | None = None
    if not default_data_dir:
        data_dir = Path(tempfile.mkdtemp(prefix="uguisu-desktop-data-"))
        env["UGUISU_DATA_DIR"] = str(data_dir)

    process = subprocess.Popen(
        [*command, "--print-port"],
        cwd=workdir,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
    )
    streams = Streams(process)
    checks = 0
    try:
        port = wait_for(
            lambda: streams.port() or (process.poll() is not None and "exited"),
            what="the application to announce its port",
        )
        if port == "exited" or process.poll() is not None:
            raise Failure(f"[{layout}] exited with {process.returncode} before serving")
        client = Client(port)
        checks += 1

        outside = outside_address()
        if outside is not None:
            if not refused(outside, port):
                raise Failure(f"[{layout}] port {port} answers on {outside}, not only on loopback")
            checks += 1

        client.expect("/api/v1/health", status="ok")
        checks += check_spa(lambda path: client.raw("GET", path), spa, layout) + 1

        what = "the WebView to bootstrap its session"
        if default_data_dir:
            # The WebView profile is per user, like the default data directory,
            # and a window that still has a valid session never exchanges.
            what += (
                "; with --default-data-dir, a WebView profile left by an earlier run may "
                "still hold a session: move the app's local data directory aside"
            )
        wait_for(lambda: BOOTSTRAPPED in streams.text(), what=what)
        checks += 1
        if process.poll() is not None:
            raise Failure(f"[{layout}] exited with {process.returncode} after starting")

        if layout == "flatpak":
            os.kill(sandboxed_app(process.pid), signal.SIGTERM)
        elif close == "logoff":
            end_session(process.pid)
            # What Windows does once the session's end is answered.
            process.kill()
        else:
            request_close(process)
        try:
            code = process.wait(timeout=90)
        except subprocess.TimeoutExpired:
            process.kill()
            raise Failure(f"[{layout}] did not exit within 90 s of a close request") from None
        if code != 0 and close != "logoff":
            raise Failure(f"[{layout}] exited with {code} after a close request")
        # The pipes may still hold the last lines after the exit.
        try:
            wait_for(lambda: CLOSED in streams.text(), what=CLOSED, timeout=5)
        except Failure:
            raise Failure(f"[{layout}] exited without logging {CLOSED!r}: the engine was not closed") from None
        checks += 1

        if not refused("127.0.0.1", port):
            raise Failure(f"[{layout}] port {port} still answers after the process exited")
        if sys.platform.startswith("linux") and not default_data_dir:
            marker = f"UGUISU_DATA_DIR={data_dir}"
            deadline = time.monotonic() + 20
            while (left := survivors(marker)) and time.monotonic() < deadline:
                time.sleep(0.5)
            if left:
                raise Failure(f"[{layout}] left processes running:\n" + "\n".join(left))
        checks += 1

        leaked = SECRET.findall(streams.text())
        if leaked:
            raise Failure(f"[{layout}] wrote {len(leaked)} 64-hex-digit value(s) to its output")
        checks += 1

        opened = opened_data_dir(streams.text())
        if opened is None:
            raise Failure(f"[{layout}] never logged where its engine opened")
        if data_dir is not None and opened != data_dir:
            raise Failure(f"[{layout}] opened {opened}, not the UGUISU_DATA_DIR it was given ({data_dir})")
        database_intact(opened)
        if default_data_dir:
            # For a caller that checks the directory survives an uninstall.
            print(f"data_dir={opened}")
        checks += 1
        return checks
    except (Failure, OSError, http.client.HTTPException) as failure:
        # A refused or reset connection usually means the application died;
        # its own output says why, and a bare traceback would hide it.
        exited = "" if process.poll() is None else f" (the application exited with {process.returncode})"
        raise Failure(
            f"[{layout}] {type(failure).__name__}: {failure}{exited}\n"
            f"--- application output ---\n{streams.text()}"
        ) from None
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        shutil.rmtree(workdir, ignore_errors=True)
        if data_dir is not None:
            shutil.rmtree(data_dir, ignore_errors=True)


def self_test() -> int:
    """Proves `expected_spa` and `check_spa` reject what they exist to reject."""
    checks = 0
    root = Path(tempfile.mkdtemp(prefix="uguisu-smoke-self-"))
    try:
        dist = root / "dist"
        (dist / "assets").mkdir(parents=True)
        index = (
            b'<!doctype html><script type="module" src="/assets/index-Ab12Cd34.js"></script>'
            b'<link rel="stylesheet" href="/assets/index-Ef56Gh78.css">'
        )
        (dist / "index.html").write_bytes(index)
        (dist / "assets" / "index-Ab12Cd34.js").write_bytes(b"console.log(1)")
        (dist / "assets" / "index-Ef56Gh78.css").write_bytes(b"body{}")
        spa = expected_spa(dist)
        csp = {"content-type": "text/html", "content-security-policy": "; ".join(CSP_REQUIRED)}

        def serving(files: dict[str, bytes], page_headers: dict[str, str] = csp) -> Fetch:
            def fetch(path: str):
                if path not in files:
                    return 404, {}, b"not found"
                if path in ("/", DEEP_ROUTE):
                    return 200, page_headers, files[path]
                return 200, {"content-type": "text/javascript"}, files[path]
            return fetch

        good = {
            "/": index,
            DEEP_ROUTE: index,
            "/assets/index-Ab12Cd34.js": b"console.log(1)",
            "/assets/index-Ef56Gh78.css": b"body{}",
        }
        check_spa(serving(good), spa, "self-test")
        checks += 1

        rejected = {
            "a different index.html": {**good, "/": b"<!doctype html><p>other</p>"},
            "no history fallback": {k: v for k, v in good.items() if k != DEEP_ROUTE},
            "a missing asset": {k: v for k, v in good.items() if not k.endswith(".css")},
            "a changed asset": {**good, "/assets/index-Ab12Cd34.js": b"console.log(2)"},
        }
        for name, files in rejected.items():
            try:
                check_spa(serving(files), spa, "self-test")
            except Failure:
                checks += 1
            else:
                raise Failure(f"self-test: {name} was accepted")
        try:
            check_spa(serving(good, {"content-type": "text/html"}), spa, "self-test")
        except Failure:
            checks += 1
        else:
            raise Failure("self-test: a response without the CSP was accepted")

        check_bundle_type(b"..__TAURI_BUNDLE_TYPE_VAR_DEB..", "deb")
        checks += 1
        for name, binary, layout in (
            ("another format's", b"__TAURI_BUNDLE_TYPE_VAR_DEB", "rpm"),
            ("an unpatched", b"__TAURI_BUNDLE_TYPE_VAR_UNK", "msi"),
            ("a doubly patched", b"__TAURI_BUNDLE_TYPE_VAR_NSS__TAURI_BUNDLE_TYPE_VAR_MSI", "msi"),
            ("an unmarked", b"no marker", "deb"),
        ):
            try:
                check_bundle_type(binary, layout)
            except Failure:
                checks += 1
            else:
                raise Failure(f"self-test: {name} binary was accepted as {layout}")

        bad_builds = {
            "no index.html": {},
            "a dev-server page": {"index.html": b'<script type="module" src="/@vite/client"></script>'},
            "no hashed script": {"index.html": b'<script src="/main.js"></script>'},
            "a missing referenced file": {
                "index.html": b'<script type="module" src="/assets/index-Zz99Yy88.js"></script>'
            },
        }
        for name, files in bad_builds.items():
            build = root / name.replace(" ", "-")
            build.mkdir()
            for relative, data in files.items():
                (build / relative).write_bytes(data)
            try:
                expected_spa(build)
            except Failure:
                checks += 1
            else:
                raise Failure(f"self-test: --expect-spa accepted {name}")

        # The main window's style, then tao's event-target window (tao 0.35
        # `create_event_target_window`) and a hidden main window.
        if not user_closable(True, 0x10CF_0000, 0x0000_0100):
            raise Failure("self-test: the application window was not closable")
        checks += 1
        for name, visible, style, ex_style in (
            ("tao's event target", True, 0x9000_0000, 0x0808_00A0),
            ("a hidden window", False, 0x10CF_0000, 0x0000_0100),
        ):
            if user_closable(visible, style, ex_style):
                raise Failure(f"self-test: {name} would be sent WM_CLOSE")
            checks += 1

        if sys.platform.startswith("linux"):
            app = root / "uguisu-desktop"
            app.symlink_to(shutil.which("sleep"))
            tree = subprocess.Popen(["sh", "-c", f"sh -c '\"{app}\" 30' & wait"])
            pid = wait_for(lambda: sandboxed_app(tree.pid), what="the self-test's process tree")
            try:
                argv0 = (Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0", 1)[0]
                if argv0 != str(app).encode():
                    raise Failure(f"self-test: found {argv0!r} instead of {app}")
                checks += 1
            finally:
                os.kill(pid, signal.SIGKILL)
                tree.wait()
            try:
                sandboxed_app(os.getpid())
            except Failure:
                checks += 1
            else:
                raise Failure("self-test: found an uguisu-desktop below a tree without one")
    finally:
        shutil.rmtree(root, ignore_errors=True)
    return checks


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--self-test", action="store_true", help="check the checks, then exit")
    parser.add_argument("--expect-spa", type=Path, help="the web/dist the package was built from")
    parser.add_argument("--layout", choices=LAYOUTS, help="what was installed, for the messages")
    parser.add_argument(
        "--default-data-dir",
        action="store_true",
        help="leave UGUISU_DATA_DIR unset, check wherever the app says it opened, and keep it",
    )
    parser.add_argument(
        "--installed-binary",
        type=Path,
        help="the executable the package installed, to check the format recorded in it",
    )
    parser.add_argument(
        "--close",
        choices=("window", "logoff"),
        default="window",
        help="how the application is ended: its close request, or a Windows logoff",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER, help="-- <launcher and arguments>")
    args = parser.parse_args()
    if args.close == "logoff" and os.name != "nt":
        parser.error("--close logoff simulates a Windows logoff")

    try:
        if args.self_test:
            print(f"{self_test()} checks")
            return 0
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command or args.expect_spa is None or args.layout is None:
            parser.error("--expect-spa, --layout and a command after -- are required")
        checks = 0
        if args.installed_binary is not None:
            check_bundle_type(args.installed_binary.read_bytes(), args.layout)
            checks += 1
        checks += smoke(command, expected_spa(args.expect_spa), args.layout, args.default_data_dir, args.close)
    except Failure as failure:
        print(f"FAIL desktop smoke\n\n{failure}")
        return 1
    print(f"{checks} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
