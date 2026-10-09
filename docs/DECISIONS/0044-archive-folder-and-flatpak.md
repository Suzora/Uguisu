# ADR 0044 — The archive folder is the desktop's setting, and the Flatpak stays narrow

**Status:** accepted · **Date:** 2026-09-24

## Context

A desktop user expects to choose where the archive goes. Uguisu had nowhere to put that choice:

- `UGUISU_MEDIA_DIR` is one of the keys that cannot be stored (ADR 0028), and `Engine::set_setting` refuses it.
- `PUT /api/v1/settings/{key}` goes through `Engine::set_setting` too.
- The engine resolves the media directory before it loads stored settings.
- Uguisu never moves a media file, so re-pointing the directory under an existing archive would strand every row.

Inside a Flatpak, the sandbox shows the app only its own files. The GTK file chooser would show only the sandbox, and autostart writes a file the host session never reads.

## Decision

**The folder is a desktop setting, applied at the next launch.** The picker writes `media_root` to the shell's own `desktop.json` in the app's configuration directory. The next `Engine::open` layers it into `Config`, and only when the environment set no `UGUISU_MEDIA_DIR`. An environment value always wins, and the panel says the folder is pinned instead of pretending to save. The chosen folder is validated before it is stored: it must exist, be a directory, accept a write probe, and canonicalise. A picker returning something is not a reason to trust it. The panel explains that Uguisu does not move existing media. This is one desktop-local key, not a second settings system. Uguisu's own settings and the server's semantics do not change.

**The Flatpak** (`desktop/flatpak/`):

- **Runtime:** GNOME 50. Its platform ships WebKitGTK 4.1 (gnome-build-meta `sdk-platform.bst`), a matching `gnome-50` CI image exists, and its freedesktop 25.08 base has rust-stable 1.98.1. GNOME 48's 24.08 base ships Rust 1.89, below the MSRV.
- **Build:** offline from declared sources: the checkout with `web/dist` built, plus `cargo vendor` output. It uses `--no-default-features --features flatpak,tauri/custom-protocol`, which swaps the GTK chooser for the xdg-desktop-portal one. rfd refuses both at once.
- **Permissions:** Wayland with an X11 fallback, IPC, DRI, network, PulseAudio, and the notification service. There is **no `--filesystem` entry**. The archive defaults to the app's own data directory, `~/.var/app/io.github.suzora.Uguisu/data/uguisu`. A folder elsewhere is reached only through the portal.
- **Tray:** none. The runtime has no appindicator library, and the shell logs its absence.
- **Autostart:** reported as unavailable in the sandbox, instead of offering a switch that does nothing.

## Consequences

- A new folder applies from the next launch. An existing archive stays where it is: the panel says so before the change, and the Archive lists its episodes as missing.
- Whether a folder chosen through the portal stays writable across restarts and upgrades has **not been verified**. It has to hold for the whole of Uguisu's write sequence: fsync, atomic rename and directory fsync through the document portal's FUSE layer. Until it is verified on a real Flatpak desktop, a chosen folder that is no longer reachable must fail clearly and ask the user to choose again. It must never fall back to widening the sandbox.
- A normal `flatpak uninstall` keeps `~/.var/app/<id>`. `--delete-data` removes it, and with it the database, and the archive too if it lives there. `INSTALL.md` says this plainly.

## Alternatives considered

- **Make `UGUISU_MEDIA_DIR` storable.** A stored value would be read after the engine had already chosen the directory, and would reopen a decision ADR 0028 made deliberately.
- **Move media when the folder changes.** Uguisu never moves or deletes media; a move that fails halfway is the data loss this project exists to prevent.
- **`--filesystem=home` or `host`.** It makes the portal pointless and hands the whole home directory to a network-facing application.
- **Ship the appindicator modules in the Flatpak.** That means four more source-built modules for a Show/Quit menu. It is left for a later phase if the tray turns out to matter there.
