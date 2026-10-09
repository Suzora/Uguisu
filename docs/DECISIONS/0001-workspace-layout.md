# ADR 0001 — Cargo workspace layout and crate boundaries

**Status:** accepted · **Date:** 2026-09-17

## Context

The brief proposes twelve crates (`uguisu-core`, `-discovery`, `-feed`, `-downloader`, `-archive`, `-metadata`, `-storage`, `-api`, `-scheduler`, `-cli`, `-web`, `-desktop`) and asks for a justified final layout. Goals: shared core across Docker, CLI and desktop; discovery independent from archival; no business logic in frontends; compile times and ownership boundaries that stay manageable for a small team.

## Decision

Eleven Rust crates under `crates/`, plus `web/` (SPA) and `desktop/` (Tauri) outside the Cargo workspace's library graph:

`uguisu-core`, `uguisu-http`, `uguisu-discovery`, `uguisu-feed`, `uguisu-download`, `uguisu-archive`, `uguisu-metadata`, `uguisu-storage`, `uguisu-engine`, `uguisu-server`, `uguisu-cli`.

Changes relative to the brief:
- **`uguisu-scheduler` folded into `uguisu-engine`.** Scheduling is orchestration over the other services; a separate crate would need to depend on the engine or duplicate its service types.
- **`uguisu-api` renamed `uguisu-server`** (it also serves the SPA and is embedded by the desktop shell).
- **`uguisu-web` and `uguisu-desktop` are not Rust library crates**; the SPA is a pnpm project, the desktop shell is a Tauri project that depends on `uguisu-server` as a library.
- **`uguisu-http` added** so the SSRF policy, rate limiting and conditional-request logic exist once and are used by discovery, feed fetching and downloads.

Dependency direction is strictly downward (see the table in `ARCHITECTURE.md`). CI does not check it; the one crate boundary `cargo deny` enforces is `reqwest` behind `uguisu-http` (`deny.toml`).

## Consequences

- Discovery can be developed and tested in Phase 2 without storage or archive code.
- The desktop app reuses the server crate instead of re-implementing commands.
- Eleven crates is more than a small project strictly needs today; the boundaries are chosen to match ownership and test surfaces, not to minimize count. Merging two crates later is cheap; splitting a tangled one is not.

## Alternatives considered

- **Single crate with modules:** faster to start, but discovery/archive independence and frontend reuse become conventions rather than compiler-enforced boundaries.
- **The brief's twelve crates verbatim:** scheduler and API crates would be thin and create dependency cycles with the engine.
