# ADR 0004 — Svelte 5 + Vite + TypeScript SPA served by axum; Tauri 2 desktop

**Status:** accepted (user decision), amended by [0031](0031-serving-the-web-ui.md), [0034](0034-web-ui-architecture.md), [0041](0041-desktop-shell-and-embedded-server.md) · **Date:** 2026-09-17

## Context

Uguisu needs a fast, information-dense web UI for Docker deployments and native Windows/Linux/Flatpak applications, without duplicating business logic. The project owner fixed the stack: Rust core, axum server, Svelte for the interface, Tauri for native packaging.

## Decision

- **Web UI:** Svelte 5 (runes) + Vite + TypeScript, as a plain SPA (not SvelteKit — no SSR, no server routes; the API is axum). Built with pnpm to `web/dist`, embedded into `uguisu-server` with `rust-embed` and served with long-cache hashed assets and an SPA fallback. API client generated from the OpenAPI document.
- **Desktop:** Tauri 2 shell in `desktop/`. It starts `uguisu-server` in-process on `127.0.0.1:<ephemeral>` with a per-launch token and loads the same SPA in the system WebView. Tauri commands are limited to OS integration (folder picker, tray, autostart, notifications, reveal in file manager). Bundles: NSIS/MSI (Windows), deb/rpm/AppImage and a Flatpak manifest (Linux, GNOME runtime with webkit2gtk-4.1).

## Consequences

- One UI code path and one API for browser and desktop; small binaries thanks to the OS WebView.
- WebView differences (WebKitGTK on Linux, WebView2 on Windows) need a CSS/JS compatibility baseline and CI smoke tests per platform (Phase 9).
- Flatpak sandboxing requires portals for folder selection and explicit filesystem permissions for the archive root; documented in Phase 9.
- SvelteKit features (file routing, load functions) are replaced by a small client router; acceptable for an app with a dozen views.

## Alternatives considered

- **Rust-native UI (Leptos/Dioxus/Yew):** single language, but slower UI iteration and larger WASM payloads; PinePods' Yew frontend was fully reworked once — a warning sign.
- **SvelteKit:** adds a Node/adapter story that the axum server would have to replace anyway.
- **Electron:** far larger bundles; no advantage for this UI.
