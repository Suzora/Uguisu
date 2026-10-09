# ADR 0031 — Serving the built web UI from a directory

**Status:** accepted, amends [ADR 0004](0004-web-ui-and-desktop-stack.md), amended by [ADR 0061](0061-the-docker-image.md) · **Date:** 2026-09-21

## Context

ADR 0004 said the SPA would be embedded into `uguisu-server` with `rust-embed` and served with hashed assets and an SPA fallback. Phase 8 built the UI and none of that existed: `uguisu-server` had no static-file serving, no `tower-http`, and `docs/ARCHITECTURE.md` recorded that serving `web/dist` was not built. A browser could reach the API but nothing served the page that talks to it.

Embedding means `web/dist` has to exist whenever the workspace is compiled. It does not: `web/dist` is generated and git-ignored, the Rust CI job never runs pnpm, and `include_dir!`/`rust-embed` fail on a directory that is not there. Putting the embed behind a cargo feature would leave the shipped code path out of every check — the worst of the options, because it is the path users get.

## Decision

**`uguisu serve` serves a directory, named by `--web` (`UGUISU_WEB_DIR`), defaulting to `web/dist` relative to the working directory.** A directory that is not there is not an error: the API runs and the fallback answers `404`. Startup logs the resolved path either way, so "no UI" is never a silent state.

This is the same shape as `--bind`/`UGUISU_BIND`: a deployment knob on the one command that starts something long-lived, not a row in `SETTINGS`. Nothing about it belongs in the database, so Phase 8 adds no settings key and no migration.

**The fallback is one handler with three answers**, in `crates/uguisu-server/src/web.rs`:

| Request | Answer |
|---|---|
| a path that names a file under the directory | that file, with a type from its extension |
| a path with no extension that names no file | `index.html`, so a refresh on `/podcasts/01ABC` works |
| a path whose last segment contains a dot, or anything under `/api` | JSON `404` |

The dot rule matters: answering `/assets/index-abc123.js` with the shell would turn a missing bundle into a blank page that looks like a working one.

**Paths are resolved through `uguisu_archive::path`** — `RelativePath::parse` then `resolve_checked` then `symlink_metadata().is_file()` — the same three steps the archive uses. A request path is attacker-controlled in a way an archive record is not, so it gets the machinery that was already proven rather than a second implementation.

**The request path is not percent-decoded.** Every name Vite emits is plain ASCII, so decoding buys nothing and removes the decode-then-traverse class of bug entirely: `%2e%2e` stays a filename that does not exist and falls through to the rules above.

`assets/` is served `public, max-age=31536000, immutable` because Vite content-hashes those names; everything else, the shell above all, is `no-cache`, or a deploy would never reach an open tab. Every response carries `X-Content-Type-Options: nosniff`.

## Consequences

- No new dependency: `tower-http` stays out of the tree, and `cargo deny`'s crate-boundary rule is untouched.
- A plain `cargo build` needs no pnpm, and the code that serves the UI is compiled and tested by every run of `check.py`.
- `uguisu serve` from somewhere other than the repository root needs `--web` or `UGUISU_WEB_DIR`. Acceptable while packaging is out of scope; a later phase that ships a Docker image or a distribution package will set it, or revisit embedding with a build script that can produce the directory. *(Amended by ADR 0061: the image sets `UGUISU_WEB_DIR=/usr/share/uguisu/web`.)*
- The served bytes are on disk, so an operator can replace them — which is the same trust as the binary itself, on a server that binds to loopback.

## Alternatives considered

- **`rust-embed` / `include_dir!` as ADR 0004 planned.** Rejected for now: it makes `cargo build` depend on a generated directory, and the feature-gate workaround leaves the shipped path unchecked.
- **`tower-http`'s `ServeDir` with a fallback.** A new dependency for a 120-line handler, and its path handling is a second implementation of a problem `uguisu-archive` already solves with tests.
- **A settings key instead of a flag.** Where the UI lives is not library state; it belongs with `--bind`, and `docs/DEVELOPMENT.md` is explicit about what a settings key costs.
