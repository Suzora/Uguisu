# ADR 0064 — Packaging: the Linux floor is a container, and every update is tested

**Status:** accepted, amends [0043](0043-desktop-packaging.md), [0046](0046-ci-within-a-minutes-budget.md) · **Date:** 2026-10-09

## Context

[ADR 0043](0043-desktop-packaging.md) builds the Linux packages on the `ubuntu-22.04` runner image, because that is the oldest supported system. It says moving the floor when that image retires is a new decision. GitHub retires runner images on its own schedule, while Ubuntu 22.04 and Debian 12 are still supported by their distributions.

Only the `.deb` upgrade ran in a packaging run ([ADR 0046](0046-ci-within-a-minutes-budget.md)). Of the others:
- NSIS over NSIS, an MSI major upgrade and `dnf upgrade` were checked once, by hand, on 2026-09-26;
- a Flatpak update was never checked.

Building the `.deb` job's build A rewrote the version in the two `Cargo.toml` files with `sed`, and left the mirrors that `check-version.py` reads at the old value.

Two Windows behaviours were read from source and never observed:
- autostart restored after an NSIS upgrade (F12);
- a logoff closing the engine (F11).

## Decision

**The floor is a container, not a runner image.**
- `bundle-linux` runs in `ubuntu:22.04` on an `ubuntu-24.04` runner. `desktop_bundle.py` keeps refusing a binary that needs a glibc newer than 2.35, so a toolchain or image change that breaks the floor fails the build, as before.
- No job runs on an `ubuntu-22.04` runner any more:
  - `package-appimage`'s FUSE step runs on `ubuntu-24.04`;
  - the floor is still checked in that job's `ubuntu:22.04` and `debian:12` containers, and in `package-deb`'s.
- The floor stays where ADR 0043 put it. Moving it is still its own decision, now tied to the end of support of the distributions it names rather than to GitHub's schedule.
- Container images come from ECR Public's copy of Docker Hub's official images. Docker Hub limits anonymous pulls per address, and runners share addresses.

**Every format that updates in place has an upgrade job.** Each bundle job also builds this source as version 0.0.1. That is build A. The jobs below install it, fill it through its API, and replace it the way a user updates that format. Then `desktop_upgrade.py` checks that build B keeps everything:
- the library, the archive's bytes, a stored setting and the search index;
- the inodes of the data directory and the database;
- a file the user left under the media root;
- and that `uguisu serve` reopens the directory.

| Job | Build A installed | Replaced by | Also checked |
|---|---|---|---|
| `upgrade-deb` | in `ubuntu:22.04` | `apt-get install` | — |
| `upgrade-rpm` | in `fedora:43` | `dnf upgrade` | — |
| `upgrade-flatpak` | from A's bundle | B's bundle, `flatpak install`, as [`INSTALL.md`](../../INSTALL.md) tells a user | — |
| `upgrade-nsis` | the default per-user folder | A's uninstaller, then B's installer, as B's installer does for a user who clicks through | the login item, removed and restored |
| `upgrade-msi` | `C:\Program Files\Uguisu`, a path with a space | `msiexec` major upgrade | the login item, kept; installing A over B is refused |

- **Build A:**
  - `scripts/at_version.py` writes the version into every place `check-version.py` reads, and into the lockfiles' path packages.
  - It runs the bundler and restores every file byte for byte.
  - The CLI the upgrades create their data directory with is built in the same jobs: the Linux one in the floor container.
  - The Flatpak's A is committed before B, so B is the newer commit of the ref, as in a release; flatpak refuses an update to an older one.
- **The login item:**
  - `desktop.json` asks for autostart before A first starts.
  - A must write the Run value pointing at itself.
  - The job states what the upgrade does to the value. An MSI major upgrade keeps it: `login-item.wxs` skips its removal then. The NSIS reinstall page runs A's uninstaller, which removes it.
  - B must point the value at itself, and after a removal must log restoring it (F12).
- **The AppImage** has no upgrade job: a user replaces the file, and no data lives in it.
- **The gate:** `desktop-packaging` needs the six package jobs and the five upgrade jobs. `check_needs.py --workflow` fails when one goes missing.

**The MSI smoke ends the application by a simulated logoff** (`desktop_smoke.py --close logoff`).
- Every top-level window is sent `WM_QUERYENDSESSION`, then `WM_ENDSESSION` with `ENDSESSION_LOGOFF`. Each one is answered before the next is sent.
- The process is then terminated, as Windows terminates it.
- The engine must have closed by then.
- The NSIS smoke closes its window, so both Windows paths run in every packaging run.

## Consequences

- A packaging run builds every format but the AppImage twice, and takes longer.
- The floor's container is pinned by tag, not digest. An `ubuntu:22.04` image update can change the build, and the glibc check still guards the floor.
- A silent NSIS upgrade (`/S` over an installation) skips the reinstall page, so the old uninstaller does not run and the Run value stays. That path is not checked; `upgrade-nsis` checks the one a person takes.
- A silent NSIS downgrade is still not refused ([`DESKTOP.md`](../DESKTOP.md) § Known limits). Only the MSI's refusal is tested.
- The logoff is simulated against a running application. A real logoff, a shutdown and a restart are still a person's check.

## Alternatives considered

- **Move the floor to Ubuntu 24.04 with the runner.** A binary linked there needs glibc 2.39 and does not start on Ubuntu 22.04 or Debian 12, which are both still supported.
- **A job container for the upgrade jobs too.** An upgrade needs a clean system for build A, and a job container offers no `docker run` for one.
- **Build A in each upgrade job.** That means another toolchain, tauri-cli and UI build per job. The bundle jobs already have all three.
- **The Flatpak through a local repository and `flatpak update`.** Uguisu publishes bundles, not a repository, and `INSTALL.md` tells a user to install the newer bundle.
