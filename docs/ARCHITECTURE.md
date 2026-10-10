# Uguisu Architecture

This document is the map. Decisions are recorded individually in [`DECISIONS/`](DECISIONS/README.md); the data model is in [`DATA_MODEL.md`](DATA_MODEL.md); lifecycles are in [`STATE_MACHINES.md`](STATE_MACHINES.md); discovery is in [`DISCOVERY.md`](DISCOVERY.md); the feed engine is in [`FEED_ENGINE.md`](FEED_ENGINE.md); the download engine is in [`DOWNLOAD_ENGINE.md`](DOWNLOAD_ENGINE.md); configuration is in [`CONFIGURATION.md`](CONFIGURATION.md); the threat model is in [`SECURITY.md`](SECURITY.md).

## 1. Shape

One reusable Rust core, three frontends, two binaries (`uguisu` and the desktop shell's `uguisu-desktop`).

```text
                          ┌──────────────────────────────────────────────┐
                          │                 uguisu-engine                │
                          │  application services · scheduler · pipelines│
                          │  event bus · config · policy evaluation      │
                          └──────────────────────┬───────────────────────┘
                                                 │
           ┌────────────────────┬────────────────┴───┬────────────────────┐
   ┌───────┴────────┐   ┌───────┴────────┐   ┌───────┴────────┐   ┌───────┴────────┐
   │uguisu-discovery│   │uguisu-download │   │uguisu-archive  │   │uguisu-metadata │
   │providers, rank,│   │job queue,      │   │paths, verify,  │   │tags, cover art │
   │dedup, resolve  │   │resume, hashing │   │sidecars, import│   │(lofty)         │
   └────┬──────────┬┘   └───┬────────┬───┘   └────────────────┘   └────────────────┘
        │          │        │        │
   ┌────┴───────┐ ┌┴────────┴────┐ ┌─┴──────────────┐
   │uguisu-feed │ │uguisu-http   │ │uguisu-storage  │
   │parse, probe│ │client + SSRF │ │SQLite/sqlx     │
   │identity,   │ └──────────────┘ └────────────────┘
   │OPML        │
   └────────────┘

   Every crate depends on uguisu-core (domain, ids, errors, events, config types);
   the engine also uses uguisu-feed, uguisu-http and uguisu-storage directly.

   Frontends:   uguisu-server (axum REST + static SPA)  ──►  web/ (Svelte 5 SPA)
                uguisu-cli   (bin `uguisu`: serve + commands, embedded or over the API)
                desktop/     (Tauri 2 shell around the same server + SPA)
```

Rules that keep this honest:

1. **`uguisu-core` depends on nothing Uguisu-specific.** It holds domain types, IDs, errors, the event enum and config types. It defines no traits: `DiscoveryProvider` and `CacheStore` are in discovery, `EventSink` in download, `SourceFormat` in archive.
2. **Discovery never touches storage or the archive.** It returns a `ResolvedFeed`; the engine persists it. A failing provider cannot affect an existing subscription.
3. **`uguisu-http` is the only crate that creates HTTP clients.** SSRF policy, user agent, limits and conditional requests live in one place and are used by discovery, feed fetching and downloads.
4. **Frontends contain no business logic.** The server maps HTTP to engine services; the CLI maps arguments to engine calls, or to API calls with `--server`; Tauri hosts the SPA and the server. If a feature needs logic in a frontend, it belongs in the engine.
5. **Filesystem and database are ordered.** Write the file, fsync, rename atomically, then record it. On conflict the filesystem wins and reconciliation explains the difference.

## 2. Workspace

| Crate | Responsibility | May depend on |
|---|---|---|
| `uguisu-core` | The domain model, `Id<T>` (ULID, monotonic), `UguisuError`, `Event`/`EventKind`, the configuration schema with value provenance, the settings registry, the scheduling arithmetic and the search-query builder. No I/O of any kind | std, serde, serde_json, ulid, time, url, thiserror, directories, subtle, utoipa |
| `uguisu-http` | The only crate that builds HTTP clients: `HttpClient`, the two-layer SSRF policy, manual redirect re-validation, throttles, conditional-GET helpers, size and time caps, the retry policy, and streaming with validated `Range` resumption | core, reqwest, governor |
| `uguisu-discovery` | Providers (apple, podcastindex, gpoddernet), the registry with throttle, circuit breaker and cache, normalization, dedup, merge, explainable ranking, the search engine with deadlines and streaming, the resolver, and the `CacheStore` port the engine implements | core, http, feed, strsim, moka, scraper |
| `uguisu-feed` | The `quick-xml` parser for RSS 2.0, Atom, RSS 1.0, iTunes and Podcasting 2.0 with per-item isolation and typed `truncated`/`partial` facts; normalization (tolerant dates with quality, durations, HTML to text); the identity cascade and comparable hash; the streaming `probe` the resolver uses; reading and writing OPML subscription lists | core, quick-xml |
| `uguisu-download` | The persistent job queue: one scheduler task with bounded global and per-host admission, a per-job worker (validated `Range` resume, streaming SHA-256, throttled progress, atomic finalization), typed error classification, the retry plan, a pure state table, startup reconciliation, and a scenario media server behind the `testing` feature | core, http, storage |
| `uguisu-archive` | No database, HTTP or engine: the ADR 0009 template language, sanitization profiles, component- and symlink-aware path resolution, collision handling, the verification algorithm over a `&Path`, policy evaluation, the reserved `.uguisu` layout, sidecar JSON, the `sha256sum -c` manifest, the streaming scanner, the import framework and its deterministic matcher, and image sniffing | core |
| `uguisu-metadata` | The only crate that uses `lofty`: reads and writes the thirteen managed tag fields (ID3v2, MP4, Vorbis/Opus), a capability table per container and field, cover-art embedding, two modes, and reads what a foreign file says it is for an import. Knows a path and a set of values and nothing else — no archive, no database, no template — which is what lets the engine hand it a copy | core, lofty |
| `uguisu-storage` | The only crate that uses `sqlx`: SQLite (see [ADR 0002](DECISIONS/0002-sqlite-storage.md)), the embedded migrations under `crates/uguisu-storage/migrations/`, a repository per entity, keyset paging, batch upserts, the FTS5 search index, and a read-only reader for Podgrab's database | core, sqlx |
| `uguisu-engine` | The `Engine` handle: the data-directory lock, storage, the HTTP clients, the discovery stack, the event bus (published after commit), the refresh coordinator; and the services — credentials, sessions and tokens, library and OPML import/export, refresh, inspect, downloads, archive, the refresh scheduler and its housekeeping, persisted settings, local search, discovery persistence | everything above |
| `uguisu-server` | The axum router under `/api/v1`, the authentication layer and its three access levels ([ADR 0036](DECISIONS/0036-three-access-levels.md)), the exposure gate ([ADR 0037](DECISIONS/0037-exposure-gate.md)), the SSE event stream, the archived-media and artwork routes ([ADR 0032](DECISIONS/0032-media-and-artwork-over-http.md)), the built SPA ([ADR 0031](DECISIONS/0031-serving-the-web-ui.md)), the OpenAPI document ([ADR 0039](DECISIONS/0039-openapi-from-the-types.md)) and graceful shutdown | engine, axum |
| `uguisu-cli` | The `uguisu` binary: `serve`, and the discovery, library, episode, feed, download, archive, scheduler, config, search and auth commands — embedded by default, an API client with `--server` | engine, server, clap |
| `web/` | Svelte 5 + Vite + TypeScript SPA, no runtime dependencies; talks only to `/api/v1` through the typed layer in `src/lib/api/` ([ADR 0034](DECISIONS/0034-web-ui-architecture.md), `docs/WEB_UI.md`) | — |
| `desktop/` | The Tauri 2 shell, its own cargo workspace: embeds `uguisu-server`, one window, tray, folder picker, notifications, reveal, autostart; packaging for six formats ([`DESKTOP.md`](DESKTOP.md)) | tauri |

Why not the brief's exact list: `uguisu-scheduler` is orchestration and lives in `uguisu-engine`; `uguisu-api` is `uguisu-server`; `uguisu-web`/`uguisu-desktop` are not Rust library crates. `uguisu-http` is added because SSRF policy must exist exactly once. See [ADR 0001](DECISIONS/0001-workspace-layout.md).

## 3. Runtime topologies

### Server (Docker, bare metal)

`uguisu serve` starts: configuration from the environment → data-directory lock → migrations → HTTP clients and the discovery stack → download reconciliation (`STATE_MACHINES.md` §3) → stored settings → the archive repair (downloads a crash left without a record are registered, interrupted tag writes settled) → the exposure gate, which refuses a network address without a password before any worker starts ([ADR 0037](DECISIONS/0037-exposure-gate.md)) → download workers, the refresh scheduler, the search-index build and the background archive check → axum on the configured bind address. Ctrl-C and SIGTERM (Ctrl-Break on Windows) stop accepting connections and give in-flight requests 2 s; then running downloads are parked as `queued(shutdown)` within the grace period, the scheduler, the index build and the archive check stop, stale manifests are written and the database pools close. The built SPA is served from a directory (`--web`/`UGUISU_WEB_DIR`, default `web/dist`), not embedded in the binary ([ADR 0031](DECISIONS/0031-serving-the-web-ui.md)). Details: [`SERVICE.md`](SERVICE.md) §1.

Data directory layout:

```text
/data/
  uguisu.db            # SQLite (WAL: uguisu.db-wal, uguisu.db-shm)
  uguisu.lock          # held by the one process that has the directory open
  uguisu.pid           # that process's id, removed when it lets go
  media/               # default media root (UGUISU_MEDIA_DIR; /media/podcasts in Docker)
```

### Desktop (Windows, Linux, Flatpak)

The Tauri shell starts the **same server** in-process (the `uguisu-server` library) bound to `127.0.0.1:<ephemeral>` with authentication always required, mints a per-launch `write` token that the WebView exchanges once for an ordinary session ([ADR 0042](DECISIONS/0042-launch-credential-exchange.md)), then loads the SPA in the WebView pointed at that origin. IPC is granted at runtime to that exact origin and nothing else ([ADR 0041](DECISIONS/0041-desktop-shell-and-embedded-server.md)). Tauri commands are used only for OS integration (folder picker, tray, autostart, notifications, "open in file manager"). Consequences: one UI code path, one API, no duplicated command surface, and a desktop user can point the CLI or a browser at the local server. See [ADR 0004](DECISIONS/0004-web-ui-and-desktop-stack.md).

### CLI

`uguisu <command>` runs **embedded by default**: discovery commands (`search podcast`, `resolve`) and `feed inspect` touch no persistent state; every other built command, `serve` included, opens the engine on the data directory (`--data-dir`/`UGUISU_DATA_DIR`, platform default otherwise) and holds `uguisu.lock` for the duration of the command. While `uguisu serve` holds the lock they fail fast (exit 1) and point to `--server`; with `--server`/`UGUISU_SERVER` a command becomes an API client of the running server, authenticated with `--token`/`UGUISU_TOKEN`. Download workers run in the command's own process only with `download run` or `--wait`. See [ADR 0003](DECISIONS/0003-single-binary-cli-over-api.md) and [ADR 0016](DECISIONS/0016-refresh-coalescing-lock-and-post-commit-events.md).

## 4. The engine

### Services

- `Engine` (Phase 3): one cheap-to-clone handle owning the data-directory lock, storage pools, the feed `HttpClient`, the discovery stack, the event bus and the refresh coordinator; frontends call its methods.
- Library (`library.rs`, built): `add_podcast` (resolve → idempotent insert → first refresh; `conflict` for a second feed with a known `podcast:guid`), `list_podcasts`, `podcast`, `episodes` (keyset paging), `source`, `sources`, `fetch_log`; and in `duplicates.rs` the candidate duplicates and their resolution by a person (ADR 0051).
- Refresh (`refresh.rs` + `sync.rs` + `migration.rs`, built): conditional fetch → sniff → fingerprint → parse → normalize → identity → pure plan (unchanged / updated / added / candidate duplicate / malformed placeholder, conservative removal) → one transaction → post-commit events → `RefreshReport`; feed URL migration with verification, the announced source an unverified one leaves, and a person's move (`move_feed`, ADR 0052); failure safety for every error kind. Details in [`FEED_ENGINE.md`](FEED_ENGINE.md). The refresh also runs the archive policy over the episodes it discovered, after the transaction commits and without being able to fail it; the policy is off by default and calls the same `enqueue_episode` a user command uses (ADR 0023).
- Discovery (`Engine::discovery()`, a `uguisu_discovery::Discovery`): `uguisu-discovery` assembled by the engine, with the cache and the resolution records persisted ([ADR 0030](DECISIONS/0030-discovery-persistence.md)).
- `DownloadService` (`uguisu-download`, built): enqueue (idempotent per episode), list and inspect, job commands (cancel/pause/resume/retry), global pause, stats, reconciliation, and the scheduler that claims jobs under global and per-host limits. The engine owns its media HTTP client, its `EventSink` (the bus) and its lifecycle; `start_downloads()` is explicit, `close()` parks running jobs. See [`DOWNLOAD_ENGINE.md`](DOWNLOAD_ENGINE.md).
- Archive (`archive.rs`, `archive_meta.rs`, `rebuild.rs`, `import.rs`, `artwork.rs`, `tagging.rs`, `orphans.rs`, built): register a completed download, render and preview template paths, verify at three depths, reconcile, relocate, evaluate the policy; then sidecars and manifests with a stale marker, `reconcile --rebuild` from sidecars, foreign-archive import (copy, never move), podcast artwork through `Profile::Artwork`, tag writing that never touches the only copy of a file, and the read-only orphan report. See [`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md).
- Service (`scheduler.rs`, `settings.rs`, `search.rs`, `discovery.rs`, built): the feed-refresh scheduler and its daily housekeeping inside `uguisu serve`, persisted settings with the environment winning, local FTS5 search, and discovery persistence. See [`SERVICE.md`](SERVICE.md). Database maintenance — a backup, the integrity checks and vacuum — runs only when asked for, through `uguisu db` or `/api/v1/db/*` ([ADR 0056](DECISIONS/0056-database-maintenance.md)).
- Startup recovery is split across the services that own it — download reconciliation, archive reconciliation, interrupted-tagging recovery — because each knows what its own leftovers mean. Orphan `.part` files and scratch files are **reported, never removed**: the download reconciliation names the first, `archive orphans` all of them.

### Pipeline for a new episode

```text
feed refreshed → episode discovered → policy evaluated (kept / skipped:reason)
→ job queued → download (.part, resumable) → response validated (status, length, type)
→ hashed while streaming → moved atomically to final path → archive_file recorded (verified)
→ sidecar written → the podcast's manifest marked stale
→ events emitted at every step
```

Every step is idempotent and re-entrant so a crash at any point resumes cleanly.

Tag writing and artwork are **not** in that line, which is a decision rather than an omission (ADR 0026). Both are explicit commands — artwork also follows a refresh when `UGUISU_ARCHIVE_ARTWORK_FETCH` is on (ADR 0047) — because a download must not depend on an image fetch it does not need, and rewriting a file's bytes the moment it arrives would make the archive's hashes move for a reason the user never asked for. Manifests are written in batches — when the queue goes idle, on `close`, or on `archive manifest write` — because a manifest is derived data and a finished download must never wait on it.

### Scheduler

- **Built in Phase 7** ([`SERVICE.md`](SERVICE.md), [ADR 0027](DECISIONS/0027-service-and-refresh-scheduler.md)): one task inside `uguisu serve` refreshes the podcasts the database says are due, through the same coalescer and the same HTTP stack as a manual refresh. Phase 4's download scheduler is a separate loop and never refreshes feeds.
- Per-podcast next-refresh time = now + interval (global default, per-podcast override, never below `Cache-Control: max-age`) + the podcast's own share of that interval: **0…+10 %**, one-sided. ADR 0027 amends the "±10 %" this document used to state — subtracting could fetch earlier than the origin allowed. A podcast that is overdue by more than one interval, or has never been fetched, is spread across a whole interval instead, which is what makes a restart after downtime a queue rather than a stampede.
- The share comes from the identifier, so the same library produces the same schedule on every machine.
- Bounded concurrency for refreshes (default 8) separate from downloads (default 3 global, 1 per host). The due query excludes what is already being refreshed, so a pass fills its capacity.
- Housekeeping (event pruning, discovery-cache expiry, pruning sessions that can no longer authenticate) rides in the same task on its own daily due time. Archive verification and reconciliation stay manual.
- Manual triggers bypass the schedule but obey concurrency limits, and a paused podcast can still be refreshed by hand.

### Event bus

`Event { schema, id, occurred_at, podcast_id?, episode_id?, kind + payload }` in core (`podcast.added`, `podcast.feed.refresh.*`, `podcast.feed.not_modified`, `podcast.metadata.updated`, `episode.discovered`, `episode.updated`, `episode.removal_detected`, `episode.identity_ambiguous`, `episode.duplicate_resolved`, `feed.url.*`, `download.*`, `archive.*`, `podcast.artwork.*`, `scheduler.*`, `settings.*`, `search.reindexed`), appended to the `events` table inside the producing transaction and published on a `tokio::sync::broadcast` channel after commit. `download.progress` is transient: published on the bus, never stored, and droppable with `?exclude=`. Consumers: `GET /api/v1/events` (SSE and stored list), which the web UI and, through it, the desktop's notifications read; structured logs; webhooks post-v1. See [ADR 0010](DECISIONS/0010-event-system.md) and [ADR 0016](DECISIONS/0016-refresh-coalescing-lock-and-post-commit-events.md).

### Configuration

Layers, later wins: built-in defaults → settings stored in the database (edited via API/CLI) → environment (`UGUISU_*`, see [`CONFIGURATION.md`](CONFIGURATION.md)) → command-line flag → per-podcast overrides. **The environment beats the database**, which inverts what this document said before Phase 7: a variable an operator put in a unit file has to be reliable, so a key set there is *pinned* and a write to it is refused rather than silently ignored ([ADR 0028](DECISIONS/0028-persisted-settings-and-precedence.md)). There is no `config.toml` in v1; an environment file is the file-based configuration (ADR 0028). Validation produces field-level, actionable errors naming the variable; `Config::describe()` (`uguisu config list`) lists the effective configuration with the origin of each value.

## 5. HTTP policy (`uguisu-http`)

- rustls, HTTP/1.1 + HTTP/2, gzip/br/deflate, connection pooling, configurable `User-Agent` (`Uguisu/<version> (+https://…)`).
- **SSRF (two layers, as built):** (1) before connecting, the URL is checked (scheme, port, local host names, literal IPs including IPv4-mapped/NAT64/6to4/Teredo forms and decimal/octal/hex spellings, which the URL parser canonicalizes) and the host is resolved and every address classified; (2) a DNS resolver hook returns only policy-approved addresses, so the socket can only reach what was validated (closes the rebinding gap). Redirects are walked manually, re-validated per hop (max 5), credentials dropped on host change. An operator can allow named private hosts (`UGUISU_HTTP_ALLOW_PRIVATE_HOSTS`); a `Trusted` profile exists only for the user-configured local server.
- Limits: connect/read timeouts, max response size per purpose (HTML 5 MB, feed 50 MB, media configurable), per-host concurrency and rate.
- Conditional GET helpers (ETag / If-Modified-Since) and `Range` support with validation (`Accept-Ranges`, `Content-Range` starting at the requested offset, strong validator in `If-Range`). Media transfers use `get_stream()`: the head is returned and the body is pulled chunk by chunk under an idle timeout, a cancellation token and a byte cap, so memory does not scale with file size. `Profile::Media` disables decompression so `Content-Length` matches the bytes on disk; `Profile::Artwork` (Phase 6) caps a podcast image at 16 MiB and sends `Accept: image/*`, so artwork inherits the whole SSRF stack instead of reimplementing a weaker one. `HostKey`/`HostThrottles` provide the synchronous per-host admission the download scheduler needs.
- Retry policy: idempotent GETs, exponential backoff with jitter, honour `Retry-After`.

## 6. Storage

SQLite via `sqlx`, configured as [ADR 0002](DECISIONS/0002-sqlite-storage.md) decided: one writer connection whose transactions open with `BEGIN IMMEDIATE`, a read-only reader pool, WAL. One process owns the data directory (`uguisu.lock`, ADR 0016). Migrations are embedded and applied when the engine opens, never downgraded, and never edited once shipped — `sqlx` checksums them. Repositories take a connection or a transaction, never a pool, so a refresh is one transaction; batch upserts run in chunks of 500; paging is keyset on `(sort_at, id)`, never `OFFSET` on a large table. Full-text search is FTS5 over content-owning tables kept current by SQL triggers rather than by repository calls, so every writer maintains them ([ADR 0029](DECISIONS/0029-local-search-index.md)). The database is the **index** of the archive: one `archive_files` row per episode, with unique `episode_id` and `relative_path` so registration and relocation are arbitrated by the database rather than by checks that could race. It is not the *only* index — each media file has a sidecar beside it and each podcast a `manifest.sha256`, and `archive reconcile --rebuild` reconstructs the rows from them as `unchecked`, because a sidecar is metadata and not evidence. The database stays the operational source of truth; the sidecars are what make the archive survive losing it. See [ADR 0024](DECISIONS/0024-sidecars-and-manifests.md) and [`DATA_MODEL.md`](DATA_MODEL.md) §§7–11.

## 7. Observability

`tracing` everywhere, on stderr as text or one JSON object per event (`UGUISU_LOG_FORMAT`, [`CONFIGURATION.md`](CONFIGURATION.md)), a URL with its userinfo, query and fragment redacted ([`SECURITY.md`](SECURITY.md) §3.5); levels ERROR…TRACE; events carry structured fields such as `podcast`, `episode`, `job`, `provider`, `url`, `status`, `bytes`, `duration_ms` and `error`. `GET /api/v1/health` answers liveness and the version; `GET /api/v1/status` reports the scheduler, the search index, the download queue and settings problems, and `GET /api/v1/discovery/providers` provider health. Metrics endpoint (Prometheus text) is planned post-v1.

## 8. Security summary

Single admin identity with argon2id password, cookie sessions with CSRF protection for the SPA, bearer API tokens for CLI/automation, loopback-only binding by default outside Docker, path sanitization with traversal checks against the archive root, SSRF policy above, resource limits. Full threat model in [`SECURITY.md`](SECURITY.md).

## 9. Performance principles

Async I/O with bounded concurrency; streaming hashing during download (no second pass); incremental feed diffing by identity keys; keyset pagination; no whole-library loads; caches only where benchmarks show value. Benchmarks (criterion) live under `crates/<crate>/benches/` and are part of the acceptance criteria of each phase. Uguisu adds two rules of its own: a scan streams (`walkdir`, no whole-tree vector), and a recovery query is **partial-indexed** so it costs what there is to do rather than what the archive holds.

## 10. Extensibility hooks

- Providers: implement `DiscoveryProvider`, register in the registry.
- Storage backends for media: none; the archive is one local media root, and cloud storage is post-v1.
- Integrations: subscribe to the event bus (webhooks, ntfy, Discord, Jellyfin/Audiobookshelf notifications) — post-v1.
- Metadata sources: none; episode metadata comes from the feed.
- Foreign archives: implement `SourceFormat` in `uguisu-archive::import` (scan a tree into `Candidate`s, optionally refine one from a side file). Podgrab and a generic layout are the two implementations; outside them, only `archive import --podgrab-db` knows Podgrab, through the reader for its database in `uguisu-storage` (ADR 0050). The matcher, the plan and the engine's copy-and-register path are shared.
- Transcription: `episode_extras` already stores transcript references; a processor can add local transcripts later.

## 11. Packaging

- **Docker** ([ADR 0061](DECISIONS/0061-the-docker-image.md), [`DOCKER.md`](../DOCKER.md)): multi-stage (`node` for the SPA, `rust` for the binary, a distroless glibc runtime), uid 65532 and no `PUID`/`PGID`, health check `uguisu health`, SIGTERM parks running downloads. Built from the checkout, amd64, not published.
- **Windows:** NSIS (per user) and MSI (per machine), WebView2 embedded bootstrapper.
- **Linux:** `.deb`, `.rpm`, AppImage, built on Ubuntu 22.04 for a floor of glibc 2.35 and WebKitGTK 4.1; **Flatpak** on GNOME 50, built offline with `flatpak-builder`.
- Every format is installed and smoke-tested in CI before packaging counts as verified ([ADR 0043](DECISIONS/0043-desktop-packaging.md), [`DESKTOP.md`](DESKTOP.md)).
- **Plain binary:** `uguisu` for servers and scripts.

---

*The crate boundaries hold: `uguisu-http` is the only crate that builds HTTP clients, `uguisu-storage` the only one that uses `sqlx`, `uguisu-metadata` the only one that uses `lofty`, and `uguisu-archive` touches neither the database nor HTTP. `cargo deny` enforces the first of those.*
