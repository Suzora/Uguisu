# ADR 0043 — Packaging: six formats, one version, and a gate that installs them

**Status:** accepted, amended by [0046](0046-ci-within-a-minutes-budget.md) (packaging runs on tags and by hand; the gate includes the upgrade) and [0064](0064-linux-floor-in-a-container-and-every-update-tested.md) (the floor is an `ubuntu:22.04` container; every format but the AppImage has an upgrade job) · **Date:** 2026-09-24

## Context

A desktop package can build cleanly and still be broken in ways no build step notices. Phase 9a hit each of these:

- The first `.deb` contained no web UI. The shell looked beside its binary, and Linux installs resources elsewhere.
- `deb` and `rpm` dependencies default to empty in the pinned bundler, so a package can install and then fail to start.
- A binary linked on Ubuntu 24.04 needs `GLIBC_2.39` and does not start on Debian 12 or Ubuntu 22.04.
- The AppImage bundler downloads five tools unverified, two from a `master` branch and one from a `continuous` release.
- The bundler records the package format by patching the release binary in place. A bundle run that fails part-way leaves it patched, and the next run labels every package with the wrong format, with only a warning.
- Every unpackaged smoke run found a UI somewhere: in `UGUISU_WEB_DIR`, beside a cargo build's binary, or in the working directory. So a passing smoke said nothing about the package.

## Decision

**Formats and where the UI must be.** Tauri resolves resources at `<exe>/../lib/<productName>`, or at the exe's own directory on Windows (`tauri-utils` `resource_dir_from`). `bundle.resources` maps `web/dist` to `web`:

| Format | Installed binary | UI |
|---|---|---|
| deb, rpm | `/usr/bin/uguisu-desktop` | `/usr/lib/Uguisu/web` |
| AppImage | `$APPDIR/usr/bin/uguisu-desktop` | `$APPDIR/usr/lib/Uguisu/web` |
| Flatpak | `/app/bin/uguisu-desktop` | `/app/lib/Uguisu/web` (the manifest puts it there) |
| NSIS, MSI | `<install dir>\uguisu-desktop.exe` | `<install dir>\web` |

A release build no longer falls back to a checkout's `web/dist`, and a missing UI is a startup failure. `check-desktop-layout.py` fails if the Flatpak path, `productName` or `bundle.resources` drift apart.

**Linux support floor: amd64, glibc 2.35, WebKitGTK 4.1.** Supported: Ubuntu 22.04 LTS and newer, Debian 12 and newer, and Fedora releases whose runtime satisfies the tested `.rpm`. Anything else is unsupported until verified. Linux packages are built on `ubuntu-22.04`, the oldest supported system, and `desktop_bundle.py` refuses a binary that needs a newer glibc symbol. The AppImage is built there too, and is never described as universal.

**Dependencies are read from the binary.** The `deb` list is the binary's `NEEDED` libraries mapped to packages, minus the three tauri-cli appends itself. The `t64` names carry alternatives for distributions without Ubuntu's time_t transition. The `rpm` list is the Fedora equivalent, plus the tray library, which tauri-cli appends for `deb` only. Both lists add `ca-certificates`, which no `NEEDED` entry shows: without the system's root certificates the HTTP client refuses to build, so the engine does not open.

**Windows.** WebView2 uses `embedBootstrapper`: about 1.8 MB, it needs the network at install time when WebView2 is missing, and it leaves patching to Windows Update. The other options were rejected: `offlineInstaller` adds about 127 MB to every download, `fixedRuntime` adds about 180 MB and makes us responsible for WebView2 security updates, and `downloadBootstrapper` makes the installer fetch the bootstrapper too. NSIS installs per user without elevation; Tauri's MSI template is per machine. Neither uninstaller touches the data directory, which is outside every install prefix.

**AppImage tools are pinned by SHA-256** (`scripts/appimage_tools.py`). With `bundle.useLocalToolsDir`, the bundler looks in `target/.tauri/` before downloading, so the script fills it first:

| Tool | Source |
|---|---|
| the two plugin scripts | commit URLs instead of `master` |
| linuxdeploy-plugin-appimage | tag `1-alpha-20250213-1` instead of `continuous` |
| AppRun, linuxdeploy | Tauri's named mirror releases |

A different hash is a failure. The bundler zeroes linuxdeploy's AppImage magic on every run, so those three bytes may be either value. A bundler download is also a failure.

**One version.** The root `[workspace.package] version` is authoritative. `desktop/Cargo.toml`, `web/package.json` and the newest metainfo `<release>` are mirrors, and `tauri.conf.json` has no version, so every installer takes the crate's. `check.py version` (in the default set) fails on any difference. It also fails on a version MSI would reject: not plain `major.minor.patch`, or above `255.255.65535`.

**Bundling relinks first.** `desktop_bundle.py` cleans the one crate so the binary is freshly linked. It refuses a binary without the format placeholder, fails on the bundler's "Failed to add bundler type" warning, and checks the binary was restored afterwards.

**The gate installs every artifact.** In `.github/workflows/desktop.yml`, after the bundle jobs:

- One `package-*` job per format downloads that run's artifact (exactly one file), installs or extracts it, checks the UI path and the installed binary's recorded format, and runs `desktop_smoke.py` against the exact `web/dist` that was bundled.
- The smoke refuses `UGUISU_WEB_DIR`, starts in an empty directory, and requires `/`, a deep route and every asset to match that build byte for byte.
- `desktop-packaging` needs all six jobs, runs `if: always()`, and passes only if all six succeeded when bundling was required, or all were skipped when it was not. `check_needs.py --workflow` fails if the gate's shape drifts.
- `upgrade-deb` installs a 0.0.1 build of the same source, populates it, `apt-get install`s the real package over it, and requires every byte of user data to survive (`desktop_upgrade.py`).

The bundle tier runs on tags, manual runs and pull requests, and whenever a path that ships changes: `desktop/`, `crates/`, `web/`, the root manifest and lockfile, the build and smoke scripts, and the workflow itself.

## Consequences

- Packaging counts as verified only for a workflow run whose `desktop-packaging` job passed. A green job on its own is not packaging evidence.
- A Linux build on a newer runner fails outright, instead of shipping packages that do not start on the floor.
- Changing an AppImage tool, the runtime floor or the version is a reviewed edit in one place.
- `ubuntu-22.04` runners will retire. Moving the floor is a new decision, not a runner bump.

## Alternatives considered

- **Build on the newest Ubuntu.** It excludes Debian 12 and Ubuntu 22.04, both still supported by their distributions.
- **Trust the bundler's dependency defaults.** They are empty.
- **Accept the AppImage tools unpinned.** A silent upstream swap would then produce a green build.
- **Unpackaged smoke only.** It passes for a package with no UI.
