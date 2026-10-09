# ADR 0003 — One `uguisu` binary; CLI commands run over the API

**Status:** accepted, amended 2026-09-17 (Phases 2 and 3), amended by [ADR 0016](0016-refresh-coalescing-lock-and-post-commit-events.md) · **Date:** 2026-09-17

## Context

The brief asks for a serious CLI (`uguisu search podcast …`, `uguisu archive verify`, `--json`) and for every important operation to be available without the web UI. SQLite requires a single writer, so a CLI that opens the database while the server runs is unsafe.

## Decision

- A single binary `uguisu` (crate `uguisu-cli`). `uguisu serve` runs the server; all other subcommands are **API clients** of a running server (address/token from flags or env `UGUISU_SERVER`/`UGUISU_TOKEN`; there is no config file, [ADR 0028](0028-persisted-settings-and-precedence.md)). Output is human-readable by default and JSON with `--json`.
- A small set of **maintenance commands** (`archive verify|reconcile|import`, `config validate`, `db migrate|backup|check|vacuum` ([ADR 0056](0056-database-maintenance.md)), `auth set-password`) may run **embedded** when no server is running; they acquire the same lock file the server uses and fail fast with a clear message if it is held.
- The CLI never contains business logic; it maps arguments to API calls or to engine service calls.

## Consequences

- Everything the UI can do is scriptable, and the API is exercised by the CLI's integration tests for free.
- Users on the desktop can use the CLI against the local server (not with the launch token, which lives only in the shell's memory, [ADR 0042](0042-launch-credential-exchange.md)).
- Embedded maintenance commands need the engine in the binary; binary size grows slightly — acceptable.

## Alternatives considered

- **Separate `uguisud` and `uguisu` binaries:** cleaner separation, but two artifacts to package for three platforms.
- **CLI always embedded (direct DB access):** breaks the single-writer rule and duplicates the service layer.

## Amendment 2026-09-17 (Phase 2)

Stateless discovery commands (`uguisu search podcast`, `uguisu resolve`) run **embedded by default** and only talk to a server when `--server` (or `UGUISU_SERVER`) is given explicitly. They touch no persistent state, so the single-writer concern that motivates API mode does not apply, and a user can search before ever starting a server. Once discovery records are persisted (Phase 3), `podcast add` remains an API command; `search`/`resolve` keep the embedded default.

## Amendment 2026-09-17 (Phase 3)

The library and feed commands (`podcast add|list|show|refresh`, `feed refresh|status`) also run **embedded by default** and use the API only when `--server`/`UGUISU_SERVER` is given. Embedded stateful commands open the engine on the data directory (`--data-dir`/`UGUISU_DATA_DIR`, platform default otherwise) and take `uguisu.lock`; while `uguisu serve` holds it they fail with exit 1 and point to `--server` (ADR 0016). `feed inspect` opens neither. The Phase 2 sentence "`podcast add` remains an API command" is withdrawn: with the lock in place, embedded mode is as safe as API mode and lets a user build a library before ever starting a server. The CLI still contains no business logic: both modes call the same engine services, one in-process and one through `/api/v1`.
