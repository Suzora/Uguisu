# ADR 0041 — The desktop shell embeds the server and grants one origin

**Status:** accepted, amends [ADR 0004](0004-web-ui-and-desktop-stack.md) · **Date:** 2026-09-24

## Context

ADR 0004 said the desktop application would be a Tauri 2 shell that starts `uguisu-server` in-process and loads the same SPA. Phase 9a built it, and three facts decided the design:

- `serve_with_shutdown` bound its own socket and never said which port it got, so port 0 could not be used by anything that needed to know the port.
- The WebView loads `http://127.0.0.1:<port>/`, which Tauri treats as a *remote* origin. IPC from it needs a capability whose URL pattern matches. A port wildcard would also match every other loopback page a WebView could be sent to.
- The Tauri stack needs WebKitGTK or the WebView2 SDK to build. The root workspace's `--workspace` checks must keep working without either.

## Decision

**A separate cargo workspace.** `desktop/` is its own workspace with its own lockfile, lint table (the root's, restated) and `deny.toml`. That `deny.toml` resolves only the two shipped targets and no features the crate never enables. `check.py desktop` covers it. It builds into the root `target/` (`desktop/.cargo/config.toml`), so shared dependencies compile once.

**`bind()` before `serve()`.** `uguisu_server::bind(addr, &state)` runs the exposure gate (ADR 0037), binds, and returns a `Bound` whose address can be read. `Bound::serve` then serves it. `serve_with_shutdown` is now exactly those two calls, so there is one path and the gate still runs before the socket.

**The shell repeats `uguisu serve`'s composition rather than sharing a launcher.** The steps are the same (`Config::from_env`, `Engine::open`, `AppState`, bind, start the services, serve), but every interesting argument differs: tracing sink, shutdown source, auth options, web directory, error surface. A shared function whose two callers agree on nothing would only forward. The shell contains no business logic: no feed, download, archive, path or auth policy.

**Startup order, and what each failure does:**

| Step | On failure |
|---|---|
| Build the Tauri app | exit non-zero; nothing opened |
| Find the web UI (`UGUISU_WEB_DIR`; Tauri's resource directory; a checkout's `web/dist` in debug builds only) | exit non-zero, before the engine opens: a window with no UI is a broken installation |
| Start the termination watcher | — |
| `Engine::open`, mint the launch token, bind `127.0.0.1:0`, serve | engine closed again, exit non-zero |
| Register the capability for the exact origin, open the window, install the tray | shutdown path below |
| `run_return` | — |
| Stop serving, revoke the token, `engine.close()` | — |

`run_return`, not `run`: `run` ends the process from inside the event loop and would skip the close. The termination watcher starts before the engine opens, so a SIGTERM during startup ends in the same clean close as a later one. A missing tray library is logged, not fatal: `libappindicator-sys` panics when it cannot load one, and the shell contains that panic.

**One capability, registered at runtime for one origin.** Once the port is known, `CapabilityBuilder` grants the `main` window, at exactly `http://127.0.0.1:<port>`, exactly seven commands:

| Command | Why | Scope |
|---|---|---|
| `launch_credential` | the session bootstrap ([ADR 0042](0042-launch-credential-exchange.md)) | once per launch |
| `desktop_report` | what the Settings panel shows about this machine | read-only |
| `choose_media_root` | the folder picker ([ADR 0044](0044-archive-folder-and-flatpak.md)) | writes the desktop's own settings file |
| `set_notifications`, `notify` | native notifications | best-effort |
| `set_autostart` | start at login | refused inside a Flatpak |
| `reveal` | show an archived file in the file manager | archive-relative path, resolved with `resolve_checked` |

Nothing else is reachable from the page: no shell, filesystem, process or opener-URL permission, and no wildcard. Gate 0 verified this against the pinned Tauri 2.11.6, before any other code was written. The granted command answered from the dynamic origin, and an ungranted one was refused. Uguisu's CSP (`crates/uguisu-server/src/headers.rs`) needed no change for the IPC to work.

## Consequences

- The desktop application and `uguisu serve` are the same server, UI and data model, and can open the same data directory, though not at the same time: the engine's lock refuses a second process.
- Unpackaged runs prove the shell, not packaging: a cargo build directory has a copy of the UI beside the binary. Packaging evidence comes only from installed artifacts ([ADR 0043](0043-desktop-packaging.md)).
- Tray events are not emitted on Linux; the tray menu is. The tray is absent wherever no appindicator library exists, the Flatpak included.

## Alternatives considered

- **A root workspace member.** `cargo clippy --workspace` would then need WebKitGTK on every machine.
- **A port wildcard in a static capability.** Every other loopback page would match it.
- **Every OS action as an HTTP route.** Tauri exposes no way to write the `HttpOnly` cookie, so the bootstrap still needs IPC. And the dialog, notification and opener APIs are only reachable from the shell anyway.
- **A single-instance plugin.** The engine's lock already refuses a second launch on the same data directory, with the holder's pid.
