# The Uguisu desktop shell

A Tauri 2 application that starts the **existing** `uguisu-server` in-process on
a loopback port and shows the **existing** Svelte SPA in the system WebView.
One UI, one API, one data model — see [ADR 0004](../docs/DECISIONS/0004-web-ui-and-desktop-stack.md)
and [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) §3.

## Why this is a separate cargo workspace

Building it needs WebKitGTK on Linux and the WebView2 SDK on Windows. The root
workspace's `cargo build/clippy/test --workspace` must keep working on a machine
with neither, so `desktop/` has its own `[workspace]`, its own `Cargo.lock` and
its own `deny.toml`. `python3 scripts/check.py desktop` covers it, and
`check.py deny` checks its `deny.toml` beside the root's.

## Building

```
sudo apt-get install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
pnpm --dir ../web build          # the SPA the shell serves
cargo build --manifest-path src-tauri/Cargo.toml
```

## Running it here

The shell finds the built web UI in this order: `UGUISU_WEB_DIR`, Tauri's
resource directory (where a package installs it, and where a cargo build copies
it), then, in debug builds only, `web/dist` relative to the working directory.
With none of them it refuses to start. It reads the data and media directories
through the ordinary Uguisu configuration, so `UGUISU_DATA_DIR` works exactly
as it does for `uguisu serve`.

Lifecycle, capabilities, file locations, packaging and how each format is
verified: [`docs/DESKTOP.md`](../docs/DESKTOP.md).

```
UGUISU_WEB_DIR=../web/dist UGUISU_LOG=info ../target/debug/uguisu-desktop
```

`--print-port` writes one line, `port=<n>`, to stdout for the smoke harness.
Nothing else is ever written to stdout, and the port is not a secret.

## Icons

Every packaged icon is rasterised from one source, `branding/uguisu.svg`:

```
rsvg-convert -w 1024 -h 1024 branding/uguisu.svg -o /tmp/uguisu-1024.png
cargo tauri icon /tmp/uguisu-1024.png
```

Do not hand-edit the files in `src-tauri/icons/`.
