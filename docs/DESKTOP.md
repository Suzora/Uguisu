# The desktop application

The Tauri 2 shell in `desktop/` runs the same `uguisu-server`, the same engine and the same SPA as `uguisu serve`, on a private loopback port, inside a native window. Installing it is covered in [`INSTALL.md`](../INSTALL.md). The decisions behind it are ADRs [0041](DECISIONS/0041-desktop-shell-and-embedded-server.md) (the shell), [0042](DECISIONS/0042-launch-credential-exchange.md) (the credential), [0043](DECISIONS/0043-desktop-packaging.md) (packaging) and [0044](DECISIONS/0044-archive-folder-and-flatpak.md) (the archive folder and the Flatpak).

## Lifecycle

| Step | What happens | On failure |
|---|---|---|
| 1 | Probe for a window system; build the Tauri app | exit non-zero, nothing opened |
| 2 | Read `desktop.json`; find the web UI | error dialog, exit non-zero before the engine opens |
| 3 | Watch for SIGTERM / Ctrl-C | — |
| 4 | `Engine::open`, mint the launch token, bind `127.0.0.1:0`, serve | engine closed, error dialog, exit non-zero |
| 5 | Grant the capability to that exact origin, open the window, install the tray | shutdown path, then an error dialog |
| 6 | Event loop (`run_return`) until the window closes, Quit, a signal or a Windows logoff | — |
| 7 | On the loop's `Exit`: hide the window, revoke the token, stop serving (open requests get 2 s), `engine.close()`, release the lock | — |

A second launch on the same data directory is refused by the engine's lock; its dialog names the lock file and the holding pid. Every error dialog waits to be dismissed, and the log carries the reason too. Without a window system (step 1) there is no dialog, only the log. In the Flatpak the dialog library shows messages only through `zenity`; whether the runtime provides it is not verified, so there the log may be all there is.

## Where things are

| | Linux | Flatpak | Windows |
|---|---|---|---|
| Web UI (bundled) | `/usr/lib/Uguisu/web` (AppImage: `$APPDIR/usr/lib/Uguisu/web`) | `/app/lib/Uguisu/web` | `<install dir>\web` |
| Data directory (database, lock) | `~/.local/share/uguisu` | `~/.var/app/io.github.suzora.Uguisu/data/uguisu` | `%APPDATA%\uguisu\data` |
| Archive (default) | `<data directory>/media` | `<data directory>/media` | `<data directory>\media` |
| Desktop settings (`desktop.json`) | `~/.config/io.github.suzora.Uguisu/` | `~/.var/app/io.github.suzora.Uguisu/config/io.github.suzora.Uguisu/` | `%APPDATA%\io.github.suzora.Uguisu\` |
| Log file (the running launch; the previous one as `.1`) | `~/.local/share/io.github.suzora.Uguisu/logs/uguisu-desktop.log` | `~/.var/app/io.github.suzora.Uguisu/data/io.github.suzora.Uguisu/logs/uguisu-desktop.log` | `%LOCALAPPDATA%\io.github.suzora.Uguisu\logs\uguisu-desktop.log` |

`UGUISU_DATA_DIR` and `UGUISU_MEDIA_DIR` behave exactly as they do for the server ([`CONFIGURATION.md`](CONFIGURATION.md)). A folder chosen in **Settings → Desktop** is stored in `desktop.json` and used from the next launch, unless `UGUISU_MEDIA_DIR` is set.

**Start at login** is stored in `desktop.json` too, and an installed Uguisu checks it against the OS at every start. A Windows Run value that an installer deleted (an NSIS upgrade or any uninstall does) is written again; any other difference, such as an item switched off in Task Manager or a deleted autostart file, is recorded, so the switch in Settings shows what the OS will do.

## The capability surface

Granted at runtime to the `main` window, at exactly `http://127.0.0.1:<port>`, and to nothing else:

| Command | Why | Where it is used | Platform scope |
|---|---|---|---|
| `launch_credential` | one-time session bootstrap | `web/src/lib/api/desktop.ts` `bootstrapSession` | all |
| `desktop_report` | the Desktop settings panel | Settings view | all |
| `choose_media_root` | archive folder picker | Settings view | all; the xdg portal in the Flatpak |
| `set_notifications`, `notify` | download finished / failed, feed refresh failed | `web/src/lib/desktop-notifications.ts` | all; best-effort |
| `set_autostart` | start at login | Settings view | refused inside the Flatpak |
| `reveal` | show an archived file in the file manager | Archive view | all; archive-relative paths only |

No shell, filesystem, process or opener-URL permission is granted, and no origin wildcard exists. The tray has Show and Quit only; it is absent where no appindicator library exists (the Flatpak included).

## Support statement

| Platform | Floor | Built on | Verified by |
|---|---|---|---|
| Linux (deb, rpm, AppImage) | amd64, glibc 2.35, WebKitGTK 4.1: Ubuntu 22.04+, Debian 12+, Fedora releases the `.rpm` installs on | `ubuntu-22.04` | `package-deb` (ubuntu:22.04, ubuntu:24.04, debian:12), `package-rpm` (fedora:43), `package-appimage` (ubuntu:22.04, debian:12, FUSE on the runner) |
| Flatpak | GNOME 50 runtime | GNOME 50 SDK | `package-flatpak` |
| Windows (NSIS, MSI) | Windows 10/11 x64 with WebView2 | `windows-2022` | `package-nsis`, `package-msi` |

Other Linux distributions are unsupported until verified. The AppImage is not "universal".

## Verification

| Check | What it proves | Where |
|---|---|---|
| `python3 scripts/check.py desktop-meta` | harness self-tests, layout and workflow consistency | local; `ci.yml` `quick` |
| `python3 scripts/check.py desktop` | fmt, clippy and tests of the desktop crate | local; `ci.yml` `linux` and `windows` |
| `python3 scripts/check.py deny` | cargo-deny for both workspaces | local; `ci.yml` `quick` |
| `desktop_smoke.py … --layout unpackaged` | the shell, bootstrap and shutdown; **not** packaging | `ci.yml` `linux` and `windows` |
| `desktop_smoke.py … --layout <format>` against an installed artifact | the installed UI is byte-identical to the bundled build; loopback only; clean close; intact database | `package-*` |
| `desktop_upgrade.py` | installing build B over build A keeps every byte of user data, and `uguisu serve` reopens it | `upgrade-deb` |
| `desktop-packaging` | all six `package-*` jobs and `upgrade-deb` passed in the same run | `desktop.yml`, on a `v*` tag or by hand ([ADR 0046](DECISIONS/0046-ci-within-a-minutes-budget.md)) |

Run the smoke by hand against an installed package, from a directory with no `web/dist`:

```
cd / && python3 <repo>/scripts/desktop_smoke.py --expect-spa <repo>/web/dist --layout deb \
    --installed-binary /usr/bin/uguisu-desktop -- /usr/bin/uguisu-desktop
```

Under Xvfb, set `WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 LIBGL_ALWAYS_SOFTWARE=1` for that command only.

`--default-data-dir` expects a user on whom Uguisu has not run, as on a CI runner. The WebView profile lives in the app's local data directory, beside `logs/`; one left by an earlier run can still hold a valid session, and then the window never exchanges its launch token and the smoke times out waiting for it. Move that directory aside first.

## Known limits

Commit hashes and CI run numbers that predate publication refer to the pre-publication history, which this repository does not contain.

- Whether a folder chosen through the Flatpak portal stays writable across restarts and upgrades is not verified ([ADR 0044](DECISIONS/0044-archive-folder-and-flatpak.md)).
- A remembered archive folder that is missing, not a folder or not writable stops the start before the engine opens, with a dialog that offers another folder or quits; in the Flatpak the portal's picker opens directly. An empty, writable mount point left behind by a disconnected drive passes that check. The dialog has not been observed on a desktop yet.
- Packaging runs verify upgrades for the `.deb` only. NSIS over NSIS, an MSI major upgrade and `dnf upgrade` were verified by hand on 2026-09-26, on Windows 11 and in a Fedora 43 container; Flatpak updates are not yet exercised.
- The fresh-install journey on a Windows 11 desktop was observed by a person on 2026-09-26, against `392e1f9`, before the fixes it led to; it has not been repeated since. The journey on a graphical Flatpak desktop is not claimed.
- A silent NSIS install of an older version over a newer one is not refused, although `allowDowngrades` is off: the template's check compares versions only on the reinstall page, which `/S` skips. The MSI refuses a downgrade. This is tauri-bundler 2.9.4's template, which Uguisu does not override.
- After an NSIS installation is removed, the MSI suggests the NSIS per-user folder (`%LOCALAPPDATA%\Uguisu`) instead of `C:\Program Files\Uguisu`. Both templates use the key `HKCU\Software\Suzora\Uguisu`, and the NSIS uninstaller keeps it unless *Delete the application data* is ticked.
- A Windows logoff runs step 7 because tao reports `WM_ENDSESSION` as the loop's `Exit`, and Windows ends the process once that returns. This is read from tao's source, not yet observed at a logoff; shutdown and restart are not checked.
- An uninstall removes the login item of the user who uninstalls; the MSI does it through Uguisu's own WiX fragment (`desktop/src-tauri/windows/login-item.wxs`), since the template does not know the Run value. The MSI installs for every user of the machine, so another user's login item stays and names the removed executable until Uguisu is installed again at the same path or the entry is turned off in Task Manager.
- The AppImage closes cleanly on SIGTERM: its runtime execs the application in the process that was launched and serves the mount from a separate one. This was checked locally through FUSE, and the `package-appimage` FUSE step passed in Desktop run 36845457461.
