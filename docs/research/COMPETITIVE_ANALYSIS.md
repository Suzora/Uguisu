# Competitive Analysis — Self-hosted Podcast Downloaders and Archivers

*Desk research, September 2026. Sources: project READMEs, release notes, issue trackers and community write-ups as summarized by web search. Live inspection of the repositories was not possible from the research environment; version numbers and feature claims should be spot-checked before they are cited externally. Nothing here is a criticism of the projects — they solve different problems than Uguisu.*

## 1. Method

Each tool is assessed on the axes the Uguisu brief cares about:

- **Architecture** — language, storage, deployment model
- **Discovery** — how a user finds and adds a podcast
- **Archival** — file organization, metadata, integrity, recovery
- **Scale** — behavior with thousands of feeds / 100k+ episodes
- **Automation** — CLI, API, scripting
- **Maintenance** — activity and dependency risk

Categories: **feature-complete server apps** (Podgrab, PodFetch, PinePods, Audiobookshelf), **desktop clients** (gPodder), **archival CLIs** (podcast-archiver, podcast-dl).

## 2. Tool profiles

### 2.1 Podgrab (akhilrex/podgrab)

- **Architecture:** Go (Gin + GORM), SQLite, single Docker image, server-rendered UI with an integrated player.
- **Discovery:** built-in search powered by the iTunes Search API only; RSS URL entry; OPML import/export.
- **Archival:** downloads to `Podcast Name/episode title.ext` (fixed layout, one naming toggle), settings for "download all / latest N / none", optional cover download. No hashing, no tag writing, no verification, no import of existing files. Duplicate downloads after restarts are a recurring issue theme.
- **Scale:** UI loads whole lists; refresh runs all feeds at once on a cron-like interval.
- **Automation:** small REST surface used by the UI; no CLI.
- **Maintenance:** repository created 2020, last push April 2024, issues unanswered — **effectively unmaintained**. This is the tool Uguisu explicitly succeeds; its users are the primary migration audience, so the **Podgrab folder layout must be importable**.
- **Lesson:** Podgrab proved that "iTunes search + auto-download + Docker" is the minimum viable product people want. Everything past that is where it stops.

### 2.2 PodFetch (SamTV12345/PodFetch)

- **Architecture:** Rust backend (actix-web, Diesel), SQLite or PostgreSQL, React/TypeScript frontend, Docker-first, single image, very active CI.
- **Discovery:** search via iTunes and Podcast Index (user supplies Podcast Index credentials), RSS URL entry, OPML import.
- **Archival:** downloads episodes and artwork; file/folder naming configurable through a handful of environment variables (podcast/episode format strings); local RSS feed re-export; no hashing, no verification, no archive import; metadata writing not a focus.
- **Scale:** reasonable; multi-user with per-user subscriptions; gpodder-compatible sync server; playlists; player-centric UI.
- **Automation:** REST API used by UI; OIDC/basic auth; no first-class CLI.
- **Maintenance:** active (thousands of CI runs, frequent releases). Community sees it as "Podgrab but alive".
- **Lesson:** PodFetch shows the Rust-server + SPA shape works well in Docker. Its gravity is *listening* (gpodder, playlists, players), not *archiving*. Uguisu should not compete on player features.

### 2.3 PinePods (madeofpendletonwool/PinePods)

- **Architecture:** Rust backend (axum), Yew/WASM frontend (reworked in 0.9), PostgreSQL or MySQL/MariaDB **plus** Valkey/Redis, process supervision inside the container, native iOS/Android and desktop clients.
- **Discovery:** own search API service in front of Podcast Index/iTunes, YouTube channel subscriptions, OPML, gpodder (built-in server, external servers, Nextcloud).
- **Archival:** downloads are a feature of the player (offline listening), not an archive: no templates, no hashing, no import/verify.
- **Scale:** designed for multi-user, "Raspberry Pi to Kubernetes"; requires three services (app, DB, cache).
- **Automation:** REST API; no CLI.
- **Maintenance:** very active; large feature surface (video podcasts, CarPlay, host following).
- **Lesson:** PinePods is the "complete podcast ecosystem" — a streaming service you host. Its deployment weight (Postgres + Valkey) is exactly what an archive tool should avoid. Uguisu must stay **one binary + one SQLite file**.

### 2.4 gPodder (gpodder/gpodder)

- **Architecture:** Python + GTK desktop application, since 2005, latest 3.11.x; gpodder.net / mygpo sync; extensions system.
- **Discovery:** gpodder.net directory search, iTunes search extension, OPML, RSS URL, YouTube/SoundCloud extensions.
- **Archival:** downloads into per-podcast folders with configurable naming (extension); no server mode, no hashing/verification, no scheduled headless operation without workarounds.
- **Scale:** desktop-sized libraries.
- **Automation:** `gpo` CLI exists; scripting via extensions.
- **Maintenance:** maintained, slow cadence; Python/GTK on Windows is the weakest platform.
- **Lesson:** gPodder is the reference for a *native desktop* podcast manager and for feed-URL edge cases collected over 20 years (its issue tracker is a good source of messy-feed fixtures). Uguisu's Tauri desktop app targets the same niche with a shared server core.

### 2.5 Audiobookshelf (advplyr/audiobookshelf) — reference

- **Architecture:** Node.js server, SQLite, Vue frontend, mobile apps; audiobooks first, podcasts second.
- **Discovery:** search (iTunes), add, choose episodes, auto-download new episodes, "download episodes after date" with limits.
- **Archival:** fixed library folder structure (`Podcast/episode`), metadata JSON per item, some tag writing for audiobooks; no hashing/verification; scanner can re-import a folder.
- **Lesson:** its **folder scanner + per-item metadata JSON** is the closest thing to Uguisu's "rebuild from filesystem" goal and is worth studying for the archive-import UX. Its podcast side is still player-first.

### 2.6 podcast-archiver (janw/podcast-archiver) — reference CLI

- **Architecture:** Python CLI, no database; state is the filesystem plus an optional database of seen episodes; Docker image.
- **Archival:** `--filename-template` with `episode.title`, `episode.published_time` (strftime), `show.title`, etc.; idempotent re-runs; "archive everything from the feed" semantics; concurrency; no tagging, no hashing, no discovery, no UI.
- **Lesson:** the best existing model for **deterministic path templates** and idempotent archival runs. Uguisu's template language should be at least as expressive and must add sanitization profiles and collision rules.

### 2.7 podcast-dl (lightpohl/podcast-dl) — reference CLI

- **Architecture:** Node CLI; templates for filenames; archive file to skip already-downloaded items; metadata JSON per episode; optional exec hook after each download.
- **Lesson:** per-episode sidecar JSON and post-download hooks are cheap and valued. Uguisu adopts sidecars as a first-class archive object and an event system instead of shell hooks.

## 3. Comparison matrix

| Capability | Podgrab | PodFetch | PinePods | gPodder | Audiobookshelf | podcast-archiver | **Uguisu (target)** |
|---|---|---|---|---|---|---|---|
| Language | Go | Rust | Rust | Python | Node | Python | Rust |
| Storage | SQLite | SQLite/PG | PG/MySQL + Valkey | SQLite | SQLite | none | SQLite (WAL) |
| Deployment | Docker | Docker | Docker (3 services), apps | Desktop | Docker | CLI | Docker, Windows, Linux, Flatpak, one binary |
| Directory search | iTunes | iTunes + Podcast Index | Podcast Index/iTunes | gpodder.net, iTunes | iTunes | — | Multi-provider, fuzzy, deduplicated, explainable |
| Website → feed | — | — | — | — | — | — | Yes (bounded autodiscovery) |
| OPML | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Path templates | toggle | env format strings | — | limited | fixed | ✓ | Full template language + OS profiles |
| Tag writing | — | partial | — | — | partial | — | Format-aware, policy-driven |
| Artwork embed/sidecar | cover file | cover | — | cover | cover | — | Both, configurable |
| Chapters / P2.0 | — | — | partial (player) | — | chapters (player) | — | Parsed, stored, embedded where format allows |
| Hashes / verify | — | — | — | — | — | — | SHA-256, verify, repair |
| Resume downloads | — | — | — | partial | — | — | Yes, atomic finalization |
| Crash recovery | weak | ok | ok | n/a | ok | idempotent | Persistent job state machine |
| Import existing archive | — | — | — | — | folder scan | idempotent | Scan, match, hash, manual match |
| Rebuild DB from FS | — | — | — | — | partial | n/a | Sidecars + reconcile |
| Archive policies | all/latest N | limited | limited | limited | after date / N | all | Rule system (type, season, date, duration, title, tags) |
| CLI | — | — | — | `gpo` | — | ✓ | Full CLI with `--json` |
| API docs | — | partial | partial | — | ✓ | — | OpenAPI, versioned |
| Event/webhooks | — | — | — | — | notifications | exec hook | Event bus + webhooks (post-v1) |
| Player | ✓ | ✓ | ✓✓ | ✓ | ✓✓ | — | minimal |
| Multi-user | — | ✓ | ✓ | — | ✓ | — | single admin (v1) |
| gpodder sync | — | ✓ | ✓ | ✓ | — | — | — |
| Maintained (2026) | no | yes | yes | yes | yes | yes | — |

## 4. Commodity features

These are table stakes; Uguisu must have them but they are not differentiators and should be implemented plainly:

- RSS URL subscribe, OPML import/export
- iTunes directory search
- Scheduled refresh, auto-download of new episodes, "latest N" limits
- Docker image with persistent volumes, health check
- Basic web UI with podcast list, episode list, download queue
- Cover art download
- Basic auth

## 5. Gaps that define Uguisu

| Gap in the ecosystem | Uguisu answer | Brief § |
|---|---|---|
| Discovery is single-source, exact-match, opaque | Multi-provider aggregation, fuzzy ranking with explanations, dedup with confidence, provenance | 4–7, 19 |
| Website-only podcasts cannot be added | Bounded website → feed resolver | 8–10 |
| Feed URL changes create duplicate podcasts | Source history, feed-change detection, explicit migration | 14–15 |
| Nobody knows if the archive is complete or intact | Hash + size + source recorded; verify / reconcile / repair modes | 27 |
| Naming is a toggle | Template language, OS sanitization profiles, deterministic + tested | 28 |
| Metadata is ignored or clobbered | Source vs normalized vs embedded vs sidecar; format capabilities; write policies | 29–30 |
| Partial files and restarts corrupt state | Persistent download state machine, `.part` + atomic rename, resume | 33, 46 |
| Existing archives must be re-downloaded | Import with matching, hashing and manual resolution; Podgrab layout aware | 36–37 |
| UIs assume small libraries | Pagination/streaming everywhere, indexed queries, benchmarks | 34–35, 51 |
| Heavy deployments | One binary, one SQLite file, same core in Docker and on the desktop | 22–24 |
| No scripting story | Full CLI with `--json`, OpenAPI | 42–43 |

## 6. What to borrow, what to avoid

**Borrow**

- Podgrab: simplicity of first run (paste a name, it works), folder-per-podcast layout as the default import shape.
- PodFetch: Rust + SPA served from one binary; Podcast Index as optional credentialed provider.
- Audiobookshelf: per-item metadata JSON, folder scanner UX, "download episodes after date".
- podcast-archiver / podcast-dl: template variables with date formatting, idempotent runs, sidecar JSON.
- gPodder: two decades of feed edge cases as fixture inspiration; native desktop expectations.

**Avoid**

- PinePods' three-service deployment and streaming-service scope.
- Player-first information architecture (episodes as a playlist rather than as archive records).
- Naming/metadata behaviour controlled by environment variables only.
- Silent behaviors: skipped episodes without a reason, deleted "duplicates", overwritten tags.

---

*Status: Phase 1 research. To be refreshed before the v1 release candidate (Phase 11) with a live re-check of each project's current feature set.*
