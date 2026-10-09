# Installing Uguisu

Uguisu comes in two forms that share one engine, one data format and one web interface:

- **The desktop application**, which this file covers. It runs Uguisu on a private loopback port inside a native window.
- **The server**, `uguisu serve`, for a headless machine or a container. See [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).

Both can open the same data directory, one at a time: the second is refused while the first is running. What the desktop application does, where it keeps things, and how it is tested is in [`docs/DESKTOP.md`](docs/DESKTOP.md).

## Windows

Two installers, both x64:

| | `Uguisu_<version>_x64-setup.exe` (NSIS) | `Uguisu_<version>_x64_en-US.msi` |
|---|---|---|
| Installs for | the current user, no administrator rights | every user; needs administrator rights |
| Default location | `%LOCALAPPDATA%\Uguisu` | `%ProgramFiles%\Uguisu` |
| Silent install | `setup.exe /S` (`/D=<dir>` last, to choose the folder) | `msiexec /i Uguisu.msi /qn` (`INSTALLDIR=<dir>` to choose the folder) |

**WebView2.** Uguisu draws its window with Microsoft Edge WebView2, which Windows 11 already includes. Both installers embed Microsoft's small WebView2 bootstrapper. On a machine that lacks WebView2, installing therefore needs an internet connection to fetch it, and Windows Update keeps it patched afterwards. An offline machine without WebView2 needs Microsoft's standalone WebView2 installer run first.

**Upgrading.** Run the newer installer over the old one. Your data is not inside the install folder, so replacing the program does not touch it.

**Uninstalling.** Use *Settings → Apps*, or `uninstall.exe /S` in the install folder, or `msiexec /x Uguisu.msi /qn`. None of them deletes your data.

**Your data** is in `%APPDATA%\uguisu\data`, and the archive in `%APPDATA%\uguisu\data\media` unless you chose another folder. The log of the latest run is `%LOCALAPPDATA%\io.github.suzora.Uguisu\logs\uguisu-desktop.log`.

## Linux

Supported: **amd64, glibc 2.35 or newer, WebKitGTK 4.1**. That covers Ubuntu 22.04 LTS and newer, Debian 12 and newer, and the Fedora releases the `.rpm` installs on. Other distributions may work but are not verified. The packages are built on Ubuntu 22.04, the oldest supported system.

| Package | Install | Upgrade | Remove |
|---|---|---|---|
| `.deb` (Ubuntu, Debian) | `sudo apt install ./Uguisu_<version>_amd64.deb` | install the newer `.deb` the same way | `sudo apt remove uguisu` |
| `.rpm` (Fedora) | `sudo dnf install ./Uguisu-<version>-1.x86_64.rpm` | `sudo dnf upgrade ./Uguisu-<newer>.rpm` | `sudo dnf remove uguisu` |
| AppImage | `chmod +x Uguisu_<version>_amd64.AppImage` and run it | replace the file | delete the file |
| Flatpak | `flatpak install --user ./uguisu.flatpak` | install the newer bundle the same way | `flatpak uninstall io.github.suzora.Uguisu` |

The package manager pulls in WebKitGTK and the other libraries. An AppImage carries its own, and still needs glibc 2.35, a normal desktop's graphics stack and font libraries (fontconfig, HarfBuzz, FriBidi), and the system's root certificates (`ca-certificates`). It needs FUSE 2 (`libfuse2`) to start directly. Without FUSE, `--appimage-extract-and-run` works instead.

**Your data** is in `~/.local/share/uguisu`, and the archive in `~/.local/share/uguisu/media` unless you chose another folder. The log of the latest run is `~/.local/share/io.github.suzora.Uguisu/logs/uguisu-desktop.log`. Removing a package never deletes either.

### Flatpak

The Flatpak runs in a sandbox that sees only its own files:

- **Data** is in `~/.var/app/io.github.suzora.Uguisu/data/uguisu`, and the archive in `media` inside it, unless you choose a folder.
- **Choosing a folder** (*Settings → Desktop → Choose folder*) goes through the desktop's file-chooser portal, which grants Uguisu that one folder. Whether that grant survives restarts and updates on every desktop has not been verified yet. If Uguisu cannot reach a folder it was given, choose it again.
- **Start at login** is not available inside the sandbox; use your desktop's own startup settings.
- **There is no tray icon**, because the runtime has no tray library.
- **Uninstalling:** `flatpak uninstall io.github.suzora.Uguisu` keeps `~/.var/app/io.github.suzora.Uguisu`, and reinstalling finds your library and archive where they were. `flatpak uninstall --delete-data io.github.suzora.Uguisu` deletes that directory: the database, and the archive too if it lives there. Copy the archive out first if you want to keep it.

## Choosing where the archive goes

*Settings → Desktop → Choose folder* sets the folder, and it is used from the **next launch**. Uguisu never moves media, so what is already archived stays where it is, and the Archive lists those episodes as missing. If `UGUISU_MEDIA_DIR` is set in the environment, it wins, and the setting says so.

## Opening the same library with the server

The `uguisu` command is not part of the desktop packages: build it from a checkout ([`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md), "Installing the server") or use the Docker image ([`DOCKER.md`](DOCKER.md)). The desktop application and `uguisu serve` read the same data format. Close the application first: only one process may have a data directory open. Then point the server at the same directory:

```
UGUISU_DATA_DIR=~/.local/share/uguisu uguisu serve
```

If you chose an archive folder in the application, that choice lives in the application's own settings, which the server does not read. Tell the server as well:

```
UGUISU_DATA_DIR=~/.local/share/uguisu UGUISU_MEDIA_DIR=/path/you/chose uguisu serve
```

On Windows the data directory is `%APPDATA%\uguisu\data`. In PowerShell:

```
$env:UGUISU_DATA_DIR = "$env:APPDATA\uguisu\data"
$env:UGUISU_MEDIA_DIR = "D:\path\you\chose"   # only if you chose a folder
uguisu serve
```

In a Flatpak it is the path given above.

A library created by `uguisu serve` opens in the application the same way. Start the application with `UGUISU_DATA_DIR` set to that directory. An archive folder chosen in the application applies to whichever library it opens, this one included, so if you ever chose one, set `UGUISU_MEDIA_DIR` to this library's media directory as well (by default `media` inside its data directory).
