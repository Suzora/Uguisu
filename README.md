# Uguisu

**Uguisu — podcast downloader and archive manager by Suzora.** Consumer-grade discovery, professional-grade archival.

Uguisu is part of **Suzora**, a family of self-hosted tools.

> *"I search for a podcast, Uguisu finds the right feed, and from there my archive just takes care of itself."*

Uguisu is a self-hosted **podcast archive engine**. It finds podcasts by name (no RSS knowledge required), resolves the authoritative feed, ingests it, downloads every episode you want, preserves the publisher's original media, writes clean metadata, organizes files deterministically and keeps proving that the archive is complete and intact.

It is the intended successor to tools like Podgrab, designed for people who care about long-term archival, very large libraries, integrity, reliability, automation and local ownership of their data. Playback is secondary.

## Status

**Pre-alpha, and it works.** Uguisu finds a podcast from a URL, a website or a search, ingests its feed and keeps it in sync with conditional requests, change detection and explainable episode identity. It downloads episodes with bounded global and per-host concurrency, validated `Range` resume and SHA-256 computed while streaming, so a file under a final path is always complete and a kill at any point is recovered on the next start.

Those files are an **archive**, not a download folder. They land on readable, deterministic paths (`Darknet Diaries/2024/2024-01-15 - The Pizza Problem.mp3`, configurable with a template language that cannot escape the media root), each one recorded with its size and hash. `uguisu archive verify --full` proves the archive is still intact and changes nothing — **no command in Uguisu deletes a media file.** Every file has a `<name>.json` sidecar beside it and every podcast a `manifest.sha256` you can check with plain `sha256sum -c`, so losing the database is recoverable: `archive reconcile --rebuild` puts the records back from what is on disk. `archive import` reads an archive another tool wrote and refuses to guess — a file two episodes explain equally well is reported, never imported.

And it **runs by itself**. `uguisu serve` refreshes feeds on a schedule it works out per podcast, spread deterministically, so a library never fetches everything in the same second and a restart after a week of downtime drains as a queue rather than a stampede. Settings live in the database and can be changed while it runs, with the environment still winning. `uguisu search library "…"` searches what you already have.

The same engine is exposed at `/api/v1/{discovery,podcasts,episodes,feeds,downloads,archive,scheduler,settings,search,status,health,auth,db}/*` and as live events at `/api/v1/events` — 96 operations, specified in [`docs/api/openapi.json`](docs/api/openapi.json), which is generated from the Rust types and checked against the router in CI. [`docs/API.md`](docs/API.md) is the contract a client needs: versioning, the error vocabulary, pagination, the byte routes.

And it **authenticates**. `uguisu auth set-password` sets one password; after that a browser logs in and holds a session cookie, and the CLI or a cron job holds an API token (`read` or `write`, printed once, revocable). Nothing accepts a credential in a URL. Binding to anything other than loopback **without a password refuses to start**, rather than starting quietly the way it used to.

And it has a **web UI**. `uguisu serve` serves it: a dashboard, the library, a podcast page with its episodes, directory search and feed resolution, local search, the download queue with live progress, the archive with its verification state, settings that show what the environment pins, the service controls, and a player for archived episodes. Everything it does, the CLI does too.

It also runs as a **desktop application**: a Tauri shell around the same server and web UI, packaged as NSIS and MSI installers, `.deb`, `.rpm`, AppImage and Flatpak ([`INSTALL.md`](INSTALL.md)).

**What is missing:** TLS, which is a reverse proxy's job — [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md) shows nginx and Caddy. There are no user accounts and no roles either, by design: Uguisu has one operator. See the [roadmap](docs/ROADMAP.md).

## Stack

| Layer | Choice |
|---|---|
| Core, engine, server | Rust (stable), tokio, axum, sqlx on SQLite |
| Web UI | Svelte 5 + Vite + TypeScript SPA, no runtime dependencies, served by the server |
| Desktop (Windows, Linux, Flatpak) | Tauri 2 shell around the same server and UI |
| Deployment | The `uguisu` binary, a Docker image built from the checkout ([`DOCKER.md`](DOCKER.md)), native Windows installers, native Linux packages and Flatpak |

## Documentation

Product and research
- [Product definition](docs/PRODUCT.md) — identity, principles, non-goals, differentiators, v1 definition of done
- [Competitive analysis](docs/research/COMPETITIVE_ANALYSIS.md) — Podgrab, PodFetch, PinePods, gPodder and reference tools
- [Discovery ecosystem](docs/research/DISCOVERY_ECOSYSTEM.md) — directories, APIs, how websites expose feeds, provider recommendation

Design
- [Architecture](docs/ARCHITECTURE.md) — workspace, layering, runtime topologies, engine, HTTP policy, storage, packaging
- [Discovery](docs/DISCOVERY.md) — as built: providers, registry, dedup, merge, explainable ranking, search engine, feed resolution
- [Feed engine](docs/FEED_ENGINE.md) — as built: fetch, parse, normalize, identity, change detection, removal, migration, events
- [Download engine](docs/DOWNLOAD_ENGINE.md) — as built: queue, admission, resume, finalization, recovery, events, limits
- [Archive engine](docs/ARCHIVE_ENGINE.md) — as built: the record, path templates and sanitization, collisions, verification, relocation, the archive policy, sidecars and manifests, rebuild, import, artwork, tagging
- [Service](docs/SERVICE.md) — as built: what `serve` starts, the refresh scheduler, housekeeping, settings, local search
- [Configuration](docs/CONFIGURATION.md) — every environment variable with its default
- [HTTP API](docs/API.md) — versioning, authentication, errors, pagination, events, bytes
- [Deployment](docs/DEPLOYMENT.md) — binding, a reverse proxy, what is not trusted, backups
- [Desktop](docs/DESKTOP.md) — the shell, where it keeps things, its packages and how they are verified
- [Web UI](docs/WEB_UI.md) — as built: the pages, the API boundary, the event stream, media, security, local development
- [CLI](docs/CLI.md) — commands, JSON output, exit codes
- [Data model](docs/DATA_MODEL.md) — entities, tables, episode identity, sidecars and rebuildability
- [State machines](docs/STATE_MACHINES.md) — discovery, feed resolution, feed fetch, download job, archive file lifecycle, tag state, manifest freshness, import items
- [Security](docs/SECURITY.md) — threat model and controls
- [Migration](docs/MIGRATION.md) — moving from Podgrab or another app: subscriptions, then files, without downloading again
- [Decision records](docs/DECISIONS/README.md) — every architectural decision and its alternatives

Planning and process
- [Roadmap](docs/ROADMAP.md) — phases 1–11 with tasks, tests and acceptance criteria
- [Risk register](docs/RISKS.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Contributing](CONTRIBUTING.md)

- [Benchmarks](docs/benchmarks/) — measured, not claimed

- [Docker](DOCKER.md) — the image, its first start, compose, upgrades and backups

Planned with its phase: `TROUBLESHOOTING.md`.

## Try it

```bash
cargo build --release
./target/release/uguisu search podcast "Darknet Diaries" --explain      # Apple by default, no key needed
./target/release/uguisu resolve https://podcasts.apple.com/us/podcast/darknet-diaries/id1296350485
./target/release/uguisu podcast add https://rss.buzzsprout.com/1.rss     # stores the podcast and ingests the feed
./target/release/uguisu podcast list
./target/release/uguisu podcast refresh --all                            # conditional; --force re-parses
./target/release/uguisu feed inspect https://feeds.twit.tv/sn.xml        # parse only, nothing stored
./target/release/uguisu download podcast <podcast-id>                    # queue every episode
./target/release/uguisu download run --until-idle                        # download the queue, then stop
./target/release/uguisu download list                                    # states, progress, attempts
./target/release/uguisu archive verify --all --full                      # hash every archived file
./target/release/uguisu archive manifest write                           # sha256sum -c lists, one per podcast
./target/release/uguisu archive reconcile --rebuild                      # what a rebuild would restore (dry run)
./target/release/uguisu archive import /srv/podgrab --format podgrab     # the plan; add --apply to copy
./target/release/uguisu archive tags write <episode-id> --mode sync      # never touches the only copy
./target/release/uguisu serve                                           # http://127.0.0.1:8484/api/v1/podcasts
```

Data lives in the platform data directory (`UGUISU_DATA_DIR` / `--data-dir` to change it) and media under `<data dir>/media` (`UGUISU_MEDIA_DIR`); the Docker image uses `/data` and `/media/podcasts`. Commands, options and exit codes: [`docs/CLI.md`](docs/CLI.md). Every check is `python3 scripts/check.py` (see [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md)).

## License

Copyright (C) 2026 Suzora contributors.

Uguisu is free software: you can redistribute it and/or modify it under the terms of the [GNU Affero General Public License](LICENSE) as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version (`AGPL-3.0-or-later`). It is distributed without any warranty. Why this licence: [ADR 0048](docs/DECISIONS/0048-agpl-licence.md).
