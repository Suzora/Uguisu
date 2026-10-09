# Roadmap — Phase 1 to v1

Each phase lists objective, implementation tasks, dependencies, tests and acceptance criteria. A phase is done only when its tests exist and pass in CI and its documentation is updated. CI evidence is a green `ci` check on a pull request head whose tree is the tree that was merged, and a green `ci.yml` run on `main` after the merge, which adds the whole Windows suite ([ADR 0063](DECISIONS/0063-ci-within-five-minutes.md)). Phases are sequential by default; Phase 2 and Phase 3 can proceed in parallel because discovery has no storage dependency. Commit hashes and CI run numbers that predate publication refer to the pre-publication history, which this repository does not contain.

| Phase | Name | Status |
|---|---|---|
| 1 | Discovery and Architecture | **done** (2026-09-17) |
| 2 | Discovery Engine | **done** (2026-09-17); pending: Podcast Index with a real key |
| 3 | Core Engine | **done** (2026-09-17); its scheduler and persistence deferrals were built in Phase 7 |
| 4 | Download Engine | **done** (2026-09-18) |
| 5 | Archive Engine | **done** (2026-09-19); its deferrals were built in Phase 6 |
| 6 | Metadata Engine | **done** (2026-09-20) |
| 7 | Service, Scheduler and Persistence | **done** (2026-09-21) |
| 8 | Web UI | **done** (2026-09-21) |
| 9 | API hardening, authentication and the API contract | **done** (2026-09-22) |
| 9a | Desktop shell and native packaging | **done** (2026-10-01): `desktop-packaging` passed in Desktop run 36845457461; the journeys on real machines → 11 |
| 9b | CI within the minutes budget | **done** (2026-10-01): CI run 36873422108; a draft push billed 28 minutes, the Windows job 84, a merge none; superseded in part by [ADR 0063](DECISIONS/0063-ci-within-five-minutes.md) (2026-10-09) |
| 10 | Migration and Polish | **done** (2026-10-09): CI run 37950883844 passed the tree this repository was imported with; the MSI's login item passed Desktop run 37985697410 |
| 11 | v1 Release Candidate | |

Every suite of Phases 1–9a passed in CI for the first time on 2026-10-01: CI run 36845457492 and Desktop run 36845457461, on `b8d73d7`, the same tree as `main` at `3e7dbfb`. Until then each "done" above rested on local runs.

## Open items of Phases 1–9a

From an audit of every phase against its code, tests and CI on 2026-10-01. **Pending:** something a phase promised (a task, a test, an acceptance criterion) that is not delivered or not verified. **TODO:** work a phase deferred that v1 still needs, with where it goes. What is deliberately left out of v1 is listed once, at the end, rather than under every phase. Deferrals that were built later are marked built in their phase's section.

### Pending

| Phase | Item | Goes to |
|---|---|---|
| 2 | Podcast Index's authenticated responses were never recorded; only the 401 path is verified | 11 (needs a key) |
| 9a | Never observed on Windows: a logoff closing the engine (F11), autostart restored after an NSIS upgrade (F12), the quoted login item starting Uguisu (F16), `e2e.py` and `desktop_upgrade.py`, a machine without network or WebView2 | 11 (Windows clean machine) |
| 9a | The graphical Flatpak journey, including its zenity dialogs | 11 |

### TODO before v1

| From | Item | Goes to |
|---|---|---|
| 3, 8 | The 500k-episode, 10k-podcast benchmark (ADR 0002) | 11 |
| 5 | Verifying an archive of 100k files | 11 |
| 2 | Live latency of a multi-provider search | 11 |
| 8 | `TROUBLESHOOTING.md` | 11 |
| 1, 2 | Refreshing `research/COMPETITIVE_ANALYSIS.md`; re-reading the providers' terms | 11 |
| 3, 6 | Fuzzing the feed parser and the tag reader with cargo-fuzz, run locally or by hand, never nightly | 11 |
| 9a | A desktop journey with a native Linux package | 11 |
| 9a | Whether a folder granted through the Flatpak portal persists | 11 |
| 8 | A screen-reader pass | 11 |
| — | A first tagged pre-release, which the upgrade gates need | 11 |
| 3, 4 | The Phase 3 and 4 benchmarks re-run against their baselines | 11 |

### Not planned for v1

- **Post-v1:** the fyyd provider (ADR 0005); downloading episode artwork, chapters and transcripts, writing chapters into files, and showing chapters, transcripts and an episode's timeline (`PRODUCT.md` v1 #16); a configuration file, since an environment file is one (ADR 0028); scheduled archive verification and reconciliation; bandwidth limiting and parallel ranges; commercial providers, cloud storage, transcription, metrics; Playwright, Lighthouse and visual regression in CI (they would not fit CI's five minutes, ADR 0063); per-route rate limits; one walker for the probe and the parser; multi-row inserts unless the 500k benchmark needs them; modelling Media RSS, `liveItem` and `podcast:images`; per-podcast include and exclude rules; virtualized lists unless the benchmark needs them; a template preview and per-provider settings editors; web pages for archive maintenance, which the CLI and the API do (`PRODUCT.md` v1 #24); release binaries with an SBOM; an arm64 Docker image and a registry; the tray in the Flatpak; the two limits in Tauri's installer templates (F7, F15).
- **Struck by design:** anything that deletes media (retention, garbage collection, "orphan cleanup"); the `overwrite` and `custom` tag policies; automatic tagging after a download; `.bak` copies; Swagger UI; automatic path migration; `insta` snapshots; Spotify and YouTube as feed sources; a per-podcast tag mode (ADR 0012); pruning the episode change log (ADR 0015); multiple users, roles and OAuth.

---

## Phase 1 — Discovery and Architecture ✅

**Objective:** understand the ecosystem, define product, architecture, data model, state machines and provider strategy before writing feature code.

**Delivered:** `docs/PRODUCT.md`, `docs/research/*`, `docs/ARCHITECTURE.md`, `docs/DISCOVERY.md`, `docs/DATA_MODEL.md`, `docs/STATE_MACHINES.md`, `docs/SECURITY.md`, `docs/RISKS.md`, `docs/DECISIONS/0001–0013`, this roadmap, a compiling workspace skeleton with CI.

**Acceptance (met):** every deliverable above exists; `cargo test`, `clippy -D warnings`, `fmt --check` and the web build pass in CI, first on 2026-10-01.

---

## Phase 2 — Discovery Engine ✅

**Objective:** `uguisu search podcast "Darknet Diaries"` returns ranked, deduplicated candidates with explanations and, on selection, a verified feed.

**Done 2026-09-17.** As built: [`DISCOVERY.md`](DISCOVERY.md), [`research/DISCOVERY_ECOSYSTEM.md`](research/DISCOVERY_ECOSYSTEM.md), ADRs 0005 and 0011, [`benchmarks/2026-09-17-phase2.md`](benchmarks/2026-09-17-phase2.md).

**Still open** (tracked in [`research/DISCOVERY_ECOSYSTEM.md`](research/DISCOVERY_ECOSYSTEM.md) §7 and the [open items](#open-items-of-phases-19a)): Podcast Index authenticated responses (no key was available; → 11), fyyd (after v1, ADR 0005), and the gpodder.net popularity signal, which is inert because every live subscriber count is 0. Misspelling recall (no provider is fuzzy) was built in Phase 10 by query relaxation (ADR 0005).

**Tasks**
1. `uguisu-http`: client builder, SSRF policy with address classification, manual redirect validation, size/time caps, per-host rate limiting, retry/backoff. (Needed by every later phase; built here first.)
2. `uguisu-feed` minimal: fetch + parse enough of a feed to validate it (title, items, enclosures, self link, `new-feed-url`, `podcast:guid`). Full parser lands in Phase 3.
3. `uguisu-discovery`: `DiscoveryProvider` trait, registry with priority/config, per-provider limiter, circuit breaker, TTL cache (memory now; DB spill when storage exists). ✅ — the database tier was built in Phase 7 (ADR 0030).
4. Providers: Apple iTunes Search, Podcast Index (HMAC auth), fyyd, gpodder.net; fixture recorder tool (`tools/record-fixtures`) run in a network-enabled environment.
5. Query normalization, dedup with confidence keys, explainable ranking with configurable weights.
6. Input classification and resolver: RSS/Atom URL, directory page URLs, website autodiscovery, platform patterns, well-known paths, canonicalization.
7. CLI stub wired to the discovery crate (embedded, no server yet): `uguisu search podcast`, `uguisu resolve`, `--json`, `--explain`.
8. `docs/DISCOVERY.md` updated with measured defaults; provider docs in `docs/CONFIGURATION.md` (providers section).

**Dependencies:** Phase 1.

**Tests:** unit (normalization, dedup, ranking determinism, URL classification, autodiscovery parsing, SSRF classifier tables), provider fixture tests with `wiremock`, property tests (ranking order-independence), messy discovery fixtures (ambiguous names, misspellings, website-only shows, redirects, `new-feed-url`), benchmarks (rank/dedup 1k candidates, SSRF check).

**Acceptance:** all v1 providers pass fixture suites; a website-only fixture resolves to its feed; a private-address redirect is blocked; provider outage yields per-provider status not a global failure; search of 3 providers completes under the soft deadline with fixtures.

---

## Phase 3 — Core Engine ✅

**Objective:** a testable core with persistence, full feed parsing, scheduling foundation, configuration and logging.

**Done 2026-09-17.** As built: [`FEED_ENGINE.md`](FEED_ENGINE.md), ADRs 0014–0017, [`benchmarks/2026-09-17-phase3.md`](benchmarks/2026-09-17-phase3.md).

**Deferred with reasons:** a UI or command to resolve candidate duplicates (sent to Phase 8, which closed without it; built in Phase 10, [ADR 0051](DECISIONS/0051-candidate-duplicates-and-orphans.md)). `insta` snapshots are not needed: the corpus tests assert fields directly.

**Tasks**
1. `uguisu-core`: finalize domain types, `Id<T>`, error taxonomy, `Event` enum, config schema with validation and value provenance. ✅
2. `uguisu-storage`: SQLite setup (WAL, pragmas), migrations 0001+, repositories with keyset pagination, `events`. ✅ — FTS5, `settings` and `discovery_cache` were built in Phase 7.
3. `uguisu-feed`: full parser (RSS 2.0, Atom, iTunes, Podcasting 2.0 subset per ADR 0008), tolerant dates/durations, warnings, raw extension map; identity_key cascade (ADR 0006/0014); differential tests against `feed-rs` on the corpus. ✅ — conditional GET and `feed_fetches` logging live in the engine.
4. `uguisu-engine`: library service, refresh service (diff by identity, candidate duplicates, change log, removal detection, feed URL migration), event bus + persistence, coalescing and bounded concurrency, startup sequence (lock, migrate). ✅ — the timer loop was built in Phase 7 (ADR 0027).
5. Structured logging (`tracing`, pretty to stderr, secrets never logged). ✅ — JSON output and redaction filters were sent to Phase 7, which closed without them, and built in Phase 10: `--log-format json`, and no log carries a URL's userinfo, query or fragment (`SECURITY.md` §3.5).
6. Fixture corpus `tests/fixtures/feeds/parser/` (broken dates, duplicate/missing GUIDs, odd Unicode, missing metadata, malformed items, truncated documents, odd prefixes, Podcasting 2.0, version series). ✅
7. Docs: `CONFIGURATION.md`, `FEED_ENGINE.md`, `DATA_MODEL.md` §7 aligned with the migration, ADRs 0014–0017, amendments 0002/0003/0008/0010. ✅

**Dependencies:** Phase 1 (Phase 2 for the HTTP crate).

**Tests (as built):** 30 feed unit tests, 14 parser corpus tests, feed-rs differential, 9 property tests; storage and core unit tests; engine: library, refresh (incl. every failure kind), migration, coalescing, removal and large-feed suites; server and CLI end-to-end tests with temp data directories and wiremock. Benchmarks: parse/normalize/identity for 10–10 000 items; refresh cold import, fingerprint hit, forced full parse, +1/updated for 1 000 and 10 000 items.

**Acceptance (met):** add podcast by RSS → refresh → episodes stored with identity keys and warnings; second refresh with 304 or an identical body does no parsing; the corpus, the probe fixtures and the recorded live feeds ingest without panics and idempotently; a 10 000-item feed imports in ≈ 3 s and a forced identical refresh takes ≈ 0.7 s; paging 10 000 episodes in pages of 500 is instantaneous. Not measured: 500k-episode listings (no such fixture yet; Phase 11).

---

## Phase 4 — Download Engine ✅

**Objective:** a crash-safe, resumable, rate-limited job system that never exposes partial files.

**Done 2026-09-18.** As built: [`DOWNLOAD_ENGINE.md`](DOWNLOAD_ENGINE.md), ADRs 0018–0020, [`benchmarks/2026-09-18-phase4.md`](benchmarks/2026-09-18-phase4.md).

**Deferred with reasons:** bandwidth limiting and chunked parallel ranges (post-v1) — a single stream already saturates loopback, and multi-part fetching multiplies load on publishers.

**Tasks**
1. `uguisu-download`: persistent queue on `download_jobs`, one scheduler with global and per-host admission, priority ordering. ✅
2. Streaming download to `.uguisu-tmp/<job>.part` with SHA-256 computed inline, validated `Range` resume, content sniffing, length checks. ✅
3. Atomic finalization (fsync, rename), target-exists policy, disk-full handling (global pause). ✅
4. Retry schedule with backoff and `Retry-After`; state machine per `STATE_MACHINES.md` §3; attempt log. ✅
5. Progress events (throttled), pause/resume/cancel/retry APIs in `DownloadService`. ✅ — plus stats, bulk enqueue, retry-failed, global pause/resume and reconcile.
6. Startup recovery procedure; orphan `.part` reporting. ✅ — plus `reconcile --deep` for missing targets.
7. Graceful shutdown (finish finalization, persist state, SIGTERM). ✅ — in a container: pending until Phase 11 has an image.
8. Surfaces: `/api/v1/downloads/*`, `?exclude=` on the event stream, the `download` CLI group with `--wait` and `run`, exit codes 10/11. ✅

**Dependencies:** Phases 2 (HTTP), 3 (storage, engine).

**Tests (as built):** scenario media server with 18 routes (ranges, weak/changed ETags, redirects incl. to private addresses, 416, truncation, pacing, stalls, wrong lengths, chunked bodies, rate limits, flaky sequences); worker suite (status matrix, resume matrix, cancel/pause/shutdown, disk full, fail points); service suite (idempotency, ordering, per-host limits, persisted pause, retry schedule across restarts, recovery of every crash boundary, orphans, 100 mixed downloads with injected failures); crash suite over real restarts; security suite (refused URLs and redirects, path shape, byte cap, no secrets in details); large suite (1/10/100 MiB always, 1 GiB opt-in, peak-RSS bound); property tests for the transition table, retry bounds, paths and progress accounting; server and CLI end-to-end tests incl. kill-and-restart and SIGTERM. Benchmarks: worker scaling, resume, finalization, enqueue and first-claim latency.

**Acceptance (met):** 100 mixed downloads with injected failures complete with no duplicates, no partial files in final paths and correct hashes; a process kill at any fail point is recovered on restart and the retry resumes with a single validated `Range` request; the per-host limit is never exceeded; 512 MiB downloads in 2.87 s with a 17.9 MiB peak resident set and a matching hash; a 1 GiB body stays within 64 MiB of the baseline RSS.

---

## Phase 5 — Archive Engine ✅

**Objective:** deterministic layout, integrity, verification, reconciliation and an automatic archive policy.

**Done 2026-09-19.** As built: [`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md), ADRs 0021–0023, [`benchmarks/2026-09-19-phase5.md`](benchmarks/2026-09-19-phase5.md).

**Deferred with reasons:** sidecar JSON and `manifest.sha256`, `reconcile --rebuild`, archive import including Podgrab layouts, and podcast-level artwork went to Phase 6, which built them all (ADRs 0024–0026); reading the tags of foreign files during an import was built in Phase 10 ([ADR 0050](DECISIONS/0050-podgrab-migration.md)). Struck by design: automatic migration of existing paths beyond the explicit `archive relocate` (a configuration edit must not rewrite an archive), and retention and auto-deletion (nothing in Uguisu deletes media).

**Dependencies:** Phases 3, 4.

**Tests:** template golden tests across profiles (Windows reserved names, Unicode, long paths, empty values, collisions), property tests (root containment, determinism, no separator injection, idempotent sanitization), verification tests with tampered, truncated, emptied, deleted, symlinked and unreadable files, crash tests across the registration boundary, concurrency tests (repeated registration, verify racing relocation), policy idempotency across repeated refreshes.

**Acceptance:** identical output paths on every profile for the golden corpus; `archive verify` reports exactly the injected faults and changes nothing; a crash between the completion transaction and the registration is repaired on the next start; three refreshes of one feed create one job per episode.

---

## Phase 6 — Metadata Engine ✅

**Objective:** clean, policy-driven tags and artwork across formats.

**Objective (as executed):** the archive becomes portable and self-describing — and, on top of that, tagged.

**Done 2026-09-20.** As built: [`ARCHIVE_ENGINE.md`](ARCHIVE_ENGINE.md) §§15–20, ADRs 0024–0026, [`benchmarks/2026-09-20-phase6.md`](benchmarks/2026-09-20-phase6.md).

**Tasks**
1. Normalized metadata model and source/normalized/embedded/sidecar separation. ✅ — `TagSet` + `Field::ALL` are the managed set; the sidecar records what a container could not hold.
2. `lofty` writers with the capability table (ID3v2.4, MP4, Vorbis, Opus, FLAC). ✅ — chapters deferred, see below.
3. Policies. ✅ as **two** modes (`fill-missing`, `sync`) rather than five; both preserve unmanaged tags by construction (ADR 0026). The original tag snapshot was built in Phase 10, on the record and in the sidecar, as a record rather than a restore (ADR 0012).
4. Artwork pipeline: podcast, size limits, format sniffing, fallback rules. ✅ — podcast-level, content-addressed, through `Profile::Artwork`; episode-level artwork after v1 (`PRODUCT.md` v1 #16); since Phase 10 the sidecar keeps its URL.
5. Chapters and transcript references. ❌ deferred with a reason (below); since Phase 10 the sidecar keeps the references as the feed declared them, and fetching what they point at comes after v1.
6. Integration into the pipeline; re-tag command. ✅ as an **explicit** command — see below.

**Also built (the portability half, which is what made the phase):** per-episode sidecar JSON; per-podcast `manifest.sha256` with a stale marker and batched writes; `archive reconcile --rebuild`; import of a foreign archive behind a `SourceFormat` trait with a Podgrab adapter and a deterministic matcher; the reserved `.uguisu` layout; `/api/v1/archive/*` endpoints and the `archive sidecar|manifest|import|artwork|tags` CLI groups; ADRs 0024–0026.

**Dependencies:** Phases 4, 5.

**Tests:** round-trip tests for MP3 and FLAC, and since Phase 10 for MP4, Ogg Vorbis and Opus; the matching matrix, artwork validation tests, crash boundaries for every Phase-6 write (the artwork's since Phase 10), concurrency, security and idempotency suites; benchmarks in `docs/benchmarks/2026-09-20-phase6.md`. Fuzz targets for tag reading: → 11.

**Acceptance:** MP3 and FLAC fixtures are tagged and read back identically, and a second `sync` write is proven to be a no-op — which matters more than it sounds: a field that does not round-trip would move the archive's hash on every run. Four fields that failed that test were removed from the managed set rather than shipped (ADR 0026). No mode removes a tag Uguisu does not manage. The external-reference comparison on CI (`ffprobe`) is **not** built: the CI image has no ffmpeg, and adding one to assert what a second Rust parse already asserts was not worth the image.

**Deferred with reasons:** chapters (`podcast:chapters` → ID3 CHAP/CTOC, MP4 `chpl`; after v1, as `PRODUCT.md` v1 #16 now says) — `lofty` exposes chapter frames only through its ID3v2-specific API and not for MP4, so it would work for MP3 and silently not for M4A, and the chapters themselves are a JSON document at a URL that nothing fetches; episode-level artwork (the podcast image is what a player shows for a feed; per-episode images multiply storage for a rare case; after v1); ADR 0012's `overwrite` and `custom` policies (unbuilt rather than half-built: `overwrite` destroys a publisher's data and `custom` needs a rule language); automatic tagging after a download (it would move the hash of a file the moment it arrives, for a reason the user never asked for); `.bak` copies (a copy-then-rename leaves no moment where one would help); retention, auto-deletion and archive garbage collection (out of scope by design: nothing in Uguisu deletes media).

---

## Phase 7 — Service, Scheduler and Persistence ✅

**Objective:** Uguisu keeps working while nobody is watching — `uguisu serve` becomes a daemon, feeds refresh on their own, settings outlive the process, discovery stops forgetting, and the library is searchable.

**Done 2026-09-21.** As built: [`SERVICE.md`](SERVICE.md), ADRs 0027–0030, [`benchmarks/2026-09-21-phase7.md`](benchmarks/2026-09-21-phase7.md).

**Tasks**
1. `uguisu serve` as a long-lived process: the refresh scheduler, the housekeeping tick and the index build, all stopped by one cancellation token. ✅
2. A persistent, deterministic refresh schedule: due query, one-sided spread, catch-up after downtime, per-podcast and global pause. ✅
3. The archive policy applied after successful refreshes — unchanged Phase-5 semantics, now with something to fire it. ✅
4. Persisted settings with provenance and quarantine; `config list|get|set|unset|validate`. ✅
5. Discovery persistence: a second cache tier and resolution records. ✅
6. FTS5 search over the library, with an index the database keeps current. ✅
7. Docs: `SERVICE.md`, ADRs 0027–0030, and every stale fact these six corrected. ✅ the stale facts by Phase 10's documentation pass.

**Moved out of this phase:** authentication (ADR 0013), the OpenAPI document and the generated TypeScript client — see Phase 9. They are orthogonal to making the service run by itself, and building them here would have delayed the part that makes Uguisu an archiver. Phase 9 built them; the client became generated types only (ADR 0039).

**Dependencies:** Phases 2–6.

**Tests:** scheduler suites (capacity, pause, restart, the switch), settings suites (precedence, refusal, quarantine across a restart), search suites (hostile input, outcomes, rebuild), discovery persistence across a restart, the Phase-7 API and CLI suites, and an `EXPLAIN QUERY PLAN` guard on the due query.

**Acceptance:** an installation left alone refreshes its feeds, applies its policy, prunes its log and can be asked what it is doing — and a restart after a week of downtime drains the backlog as a queue rather than a stampede.

**Deferred with reasons:** retention, auto-deletion and archive GC (nothing in Uguisu deletes media); weekly archive verification and daily reconcile schedules (both are expensive and neither has a failure mode that waiting makes worse; decided in Phase 10: manual in v1); `config.toml` (decided in Phase 10: no file layer in v1, an environment file is one, ADR 0028). A full settings UI was deferred here and built in Phase 8 after all.

---

## Phase 8 — Web UI ✅

**Status 2026-09-21 — complete.** As built: `docs/WEB_UI.md`, ADRs [0031](DECISIONS/0031-serving-the-web-ui.md)–[0034](DECISIONS/0034-web-ui-architecture.md).

**Objective:** a fast, dense management interface centred on the archive.

**Tasks**
1. Svelte 5 SPA: typed API layer with an error taxonomy, History-API navigation, one shared event stream, the application shell. (Phase 9 generated the types from the OpenAPI document, ADR 0039.)
2. Views: Dashboard, Podcasts (list and detail with episodes), Discovery (search, provenance, resolution, add), Local search, Downloads (live queue), Archive, Settings, Service.
3. A player for archived episodes, on the browser's own `<audio>`.
4. Accessibility and keyboard navigation; light/dark from `color-scheme`.

**Dependencies:** Phase 7.

**Tests:** 110 frontend tests (vitest + jsdom + `@testing-library/svelte`) covering the API error taxonomy, the router, the event stream's lifecycle, each view's loading/empty/error/refused paths and the structural accessibility properties; `scripts/e2e.py` drives the shipped binary and the built UI through the whole journey over HTTP.

**Acceptance:** met, with the ceilings named rather than hidden. `docs/benchmarks/2026-09-21-phase8.md` has the measurements.

**Deferred with reasons:** first-run setup and login (built in Phase 9) and token management (built in Phase 10, on the Settings page); episode timelines, chapters and transcripts (the fields are parsed and stored, and no view needed them to make the archive usable; after v1); a template preview and per-provider settings editors (the API exposes no per-key metadata, so a form would have to hard-code shapes `docs/CONFIGURATION.md` already documents); virtualized lists (cursor paging plus "load more" keeps the DOM bounded — see the benchmarks for where that stops being enough); `INSTALL.md` (written in Phase 9a) and `TROUBLESHOOTING.md` (→ 11); Playwright in CI and Lighthouse budgets (§34 of the phase brief: no browser-automation dependency was added; a real-Chromium pass was run by hand and is recorded in `docs/benchmarks/2026-09-21-phase8-verification.md`); visual regression.

**Open, and needing a backend change:** ~~`GET /api/v1/podcasts` takes no parameters and has no cursor, so the library page filters and sorts in the browser; `GET /api/v1/archive` has no cursor either; a download job row carries ids but no titles, so the queue joins against the library; and there is no `GET /api/v1/episodes/{id}`, so an episode is always reached through its podcast.~~ **All four closed in Phase 9** (ADR 0040), in the API, and in the web UI in Phase 10: the library pages from the server ([ADR 0057](DECISIONS/0057-library-paged-by-the-server.md)), the archive list follows its cursor, the download queue shows the titles its rows carry, and an episode has its own page. The archive view and Discover still read every podcast: an archive row carries no podcast title, and Discover marks the feeds already added.

---

## Phase 9 — API hardening, authentication and the API contract ✅

**Objective:** the API becomes something a third party can consume on purpose: authenticated, specified, and safe to expose deliberately rather than by accident.

**Re-scoped (2026-09-22).** This phase was written as "API surface, authentication **and native applications**", bundling the auth work with the Tauri desktop shell, OS integration, NSIS/MSI/deb/rpm/AppImage/Flatpak bundles, a Windows and Linux CI matrix and `INSTALL.md`. Those are packaging and desktop work, orthogonal to authenticating an API, and building them here would have delayed the control that makes exposure safe. **Tasks 3–7 move to a later phase**, which became Phase 9a, recorded here the way the Phase-7 re-scope was; Phase 11 already owns the packaging gates.

**Struck from the original task 1:** Swagger UI at `/api/docs` is **struck**, not deferred. Vendored assets conflict with `deny.toml`'s crates-io-only `sources` policy, it needs axum features this workspace does not enable, and it would be one more unauthenticated HTML surface (ADR 0039). The document is a file; a client renders it.

**Tasks (done)**
1. One error envelope on every failure, one kind vocabulary, archive kinds on the wire (ADR 0038).
2. Migration 0006, argon2id credentials, server-side sessions, hashed API tokens with two scopes (ADR 0035).
3. Three access levels from one classifier, default deny, CSRF for cookies only (ADR 0036); security headers.
4. `uguisu auth set-password` / `token create|list|revoke`, with the password never in argv or the environment.
5. A non-loopback bind without a credential refuses to start, exit code 12 (ADR 0037).
6. One cursor contract, then the four gaps Phase 8 worked around: a parameterised and paged podcast list, a paged archive list, queue rows that carry their own titles, and `GET /api/v1/episodes/{id}` (ADR 0040).
7. OpenAPI derived from the Rust types, drift-checked in CI, and TypeScript types generated from it with no production dependency (ADR 0039); `API.md` and `DEPLOYMENT.md`.

**Moved out of this phase:** the `desktop/` Tauri 2 project and its per-launch token, OS integration (folder picker, tray, autostart, notifications, reveal file), every native bundle, the Windows and Linux CI matrix, and `INSTALL.md`. Phase 9a built them all.

**Dependencies:** Phase 8.

**Tests:** 815 Rust tests and 126 frontend tests. New suites: `crates/uguisu-server/tests/{auth,errors,pagination}.rs`, `crates/uguisu-{engine,storage,cli}/tests/auth.rs`, `expose.rs`'s loopback table, the OpenAPI drift and coverage tests, `web/src/views/Login.test.ts` and the shell's gate. `scripts/e2e.py` grew from 36 to 53 checks across three phases: the loopback walk, the same walk behind a password, and the exposure refusal.

**Acceptance:** met. Every `/api/` route except `health`, `auth/session` and `auth/login` refuses an anonymous request, an unknown `/api/` path answers 401 rather than 404, the document matches the implementation in CI, and the browser journey works with no `Authorization` header anywhere. `docs/benchmarks/2026-09-22-phase9.md` has the measurements.

**Deferred with reasons:** configured-proxy `X-Forwarded-*` trust (a spoofable header is a worse identity than the socket peer, and there is no trusted-proxy setting to hang it on — `docs/DEPLOYMENT.md` says what that costs behind a proxy); per-route rate limiting beyond login (every other route is authenticated, and the existing concurrency caps plus the 2 MB body limit bound a credentialed client); OAuth/OIDC and any external identity provider; multi-user, roles and registration; a token management panel in the web UI (built in Phase 10, on the Settings page); §3.7's `/data/secret.key` encryption at rest, which was **corrected rather than built** — both provider secrets are non-persistable, so they never reach the database and there is nothing to encrypt.

---

## Phase 9a — Desktop shell and native packaging ✅

**Done 2026-10-01** (built 2026-09-24). As built: [`DESKTOP.md`](DESKTOP.md), [`../INSTALL.md`](../INSTALL.md), ADRs [0041](DECISIONS/0041-desktop-shell-and-embedded-server.md)–[0044](DECISIONS/0044-archive-folder-and-flatpak.md).

**Objective:** the same core, launched as a desktop application and installed the way each platform installs things.

**Moved here from Phase 9** (2026-09-22 re-scope), unchanged in substance.

**Tasks (done)**
1. `uguisu-server` binds before it serves, so an embedder learns an ephemeral port without a second server path; the exposure gate still runs before the socket exists.
2. `desktop/`, its own Cargo workspace: a Tauri 2.11.6 shell that opens the engine, serves it on `127.0.0.1:0` with authentication required, and grants seven commands to exactly that origin (ADR 0041). A start that fails shows why in a native dialog.
3. A per-launch write token, exchanged once at `POST /api/v1/auth/exchange` for an ordinary session and revoked at shutdown (ADR 0042).
4. OS integration: tray, folder picker (the xdg portal in the Flatpak), notifications from the existing event stream, reveal in the file manager, autostart (refused in the Flatpak). The archive folder is a desktop launch setting that `UGUISU_MEDIA_DIR` overrides (ADR 0044).
5. NSIS, MSI, deb, rpm, AppImage and Flatpak; the Linux floor amd64 / glibc 2.35 / WebKitGTK 4.1, built on Ubuntu 22.04; AppImage tools pinned by SHA-256; one version checked everywhere (ADR 0043).
6. `.github/workflows/desktop.yml`: the desktop crate and an unpackaged smoke on Linux and Windows for every change, and for a change that ships, six `package-*` jobs that install their artifact and smoke it behind the `desktop-packaging` gate, plus an in-place `.deb` upgrade. Since Phase 9b the crate and the unpackaged smoke run in `ci.yml`, and packaging runs on a tag or by hand (ADR 0046).
7. `INSTALL.md` and `DESKTOP.md`.

**Dependencies:** Phase 9.

**Tests:** `scripts/desktop_smoke.py` (13 checks against an installed package; its self-test has 20), `scripts/desktop_upgrade.py` (6 checks), `check_needs.py` for the gate and the workflow's shape, `check-version.py`, `check-desktop-layout.py`, the desktop crate's 8 tests, and the server's exchange tests. 825 Rust tests in the workspace and 133 frontend tests.

**Acceptance (met 2026-10-01).**
- **CI:** Desktop run 36845457461 on `b8d73d7` (the same tree as `main` at `3e7dbfb`) bundled all six formats, installed and smoked each one, including the AppImage through FUSE on the runner, and upgraded a `.deb` in place: `desktop-packaging` passed, and Desktop run 36850039976 repeated it on `main`. CI run 36845457492 passed the workspace on Linux and Windows.
- **Locally, on Windows 11 (2026-09-26, `392e1f9`):** the NSIS and MSI packages pass the smoke (13 checks each), an uninstall keeps the data, NSIS over NSIS and an MSI major upgrade keep the library and the media byte for byte, and a person went through the journey from install to playback and uninstall. The same session smoked every Linux package in containers and ran `dnf upgrade`. Its 18 findings were fixed or documented in `3e7dbfb`, which has not been run on a Windows desktop since.
- **Not claimed (→ 11):** see [Pending](#pending): the Windows journeys after the fixes (F11, F12, F16, `e2e.py`, `desktop_upgrade.py`, no network or WebView2) and the graphical Flatpak journey.

**Deferred with reasons:**
- **Automated upgrades other than the `.deb`:** NSIS over NSIS, MSI major upgrades and `dnf upgrade` were verified once by hand; Flatpak updates not at all. Each needs a second build at a lower version, and only the `.deb` upgrade runs in CI (→ 11).
- **A silent NSIS downgrade, and the MSI's folder after an NSIS install:** both come from tauri-bundler's templates ([`DESKTOP.md`](DESKTOP.md) § Known limits). Fixing them means maintaining Uguisu's own copies of the templates (post-v1).
- **Persistence of a folder granted through the Flatpak portal:** it needs a graphical Flatpak session (ADR 0044; → 11).
- **The holder's pid in the lock refusal on Windows:** a Windows lock is mandatory, so the pid had to move out of `uguisu.lock`; built in Phase 10, in a sibling `uguisu.pid` (ADR 0016).
- **An MSI uninstall leaves the login item behind:** the MSI template does not know the Run value; built in Phase 10 as a WiX fragment of Uguisu's own, for the user who uninstalls ([`DESKTOP.md`](DESKTOP.md) § Known limits).

---

## Phase 9b — CI within the minutes budget ✅

**Priority: before any Phase 10 work.** As built: [ADR 0046](DECISIONS/0046-ci-within-a-minutes-budget.md).

**Superseded in part** by [ADR 0063](DECISIONS/0063-ci-within-five-minutes.md) on 2026-10-09: the repository is public, every job starts at once on every pull request, and a push to `main` runs the whole suite. What follows is what this phase built and measured.

**Objective:** CI that fits GitHub Free's 2000 Actions minutes a month, with a Windows minute billed as two, while every check still runs somewhere.

**Why first:** the minutes ran out in September before Phase 9a's gate could run. The quota reset on 2026-10-01, and that day alone billed 1566 of the 2000 minutes: about 259 per pull request push, 174–300 again per merge, 62 % of it on Windows. Without minutes no phase can show CI evidence.

**Tasks**
1. `main` green again: `fold_text` folded `İ` differently the second time, which a random property-test seed found on `main` (CI run 36850040009). ✅
2. `ci.yml`: three jobs, cheapest first: `quick` (no compiler), `linux` (the workspace, the web UI, e2e and the desktop crate in one job), and `windows`, which runs only on a pull request that is ready and only after `linux` passed. No run on a push to `main`; a timeout on every job. ✅
3. `desktop.yml`: packaging only on a `v*` tag or by hand; the gate includes the `.deb` upgrade; tauri-cli prebuilt. ✅
4. `check.py`: `desktop-meta` split from `desktop`, `deny` for both workspaces, `--locked` for clippy and test; `check_needs.py --workflow` enforces the triggers and the timeouts. ✅
5. ADR 0046; `DESKTOP.md`, `DEVELOPMENT.md`, `CONTRIBUTING.md`, `RISKS.md` (R30) and the CI-budget rules in `CLAUDE.md`. ✅
6. The Rust toolchain pinned to 1.98.1, so a new stable release cannot turn CI red on its release day. ✅ Moving to 1.99 is its own change: its new `clippy::assert_is_empty` lint flags 69 assertions.

**Dependencies:** Phase 9a.

**Tests:** `check_needs.py` (the gate requires all seven jobs; `--workflow` refuses a pull-request trigger in `desktop.yml` and a job without a timeout in either workflow); the regression seed for `folding_is_idempotent`.

**Acceptance (met 2026-10-01)**, measured from the job timestamps of the Phase 9b pull request on `1fb4295`, the tree merged as `c453ea6`:
- A draft push runs `quick` and `linux` only, for at most 35 billed minutes on a cold cache; a Markdown-only push runs `quick` only.
- Marking it ready runs `windows` once, for at most 125 billed minutes.
- A merge runs nothing; `desktop.yml` does not start on a pull request.
- Every check in `scripts/check.py` runs in a CI job, except `build`, `bench` and `openapi`, which ADR 0046 explains.
- **Measured:** CI run 36869936979, the draft, billed 28 minutes (`quick` 1, `linux` 27). CI run 36873422108, marked ready, billed 105 (`quick` 1, `linux` 20, `windows` 84 for 42 minutes). The merge started no run, and `desktop.yml` ran on no pull request.
- **Measured since:** a later pull request changed only Markdown; its CI runs 37035041912 (draft) and 37035134113 (ready) ran `quick` alone, 1 billed minute each.

---

## Phase 10 — Migration and Polish ✅

**Done 2026-10-09** (built 2026-10-01 to 2026-10-04): CI run 37950883844 passed the tree this repository was imported with, the whole suite on Linux and Windows, and Desktop run 37985697410 packaged it. As built: [`MIGRATION.md`](MIGRATION.md), ADRs 0047 and 0049–0060 but 0058.

**Objective:** make moving to Uguisu painless.

**Fix first** ✅, defects the 2026-10-01 audit found, all fixed:
- the web UI and the CLI over the API showed at most 50 podcasts; both follow the cursor now;
- `UGUISU_ARCHIVE_ARTWORK_FETCH` did nothing; a refresh now fetches artwork when it is on (ADR 0047);
- the 2 MB body limit had no test; it has one;
- the Windows rename retry differed from ADR 0019; it follows it, with tests on Windows.

**Defects and test gaps**, built 2026-10-02. The Pending rows above that went to Phase 10 and one TODO row, plus what fixing them found:
- the archive view kept only the first 200 files; it pages with the cursor ("Load more");
- the mass-removal guard was a constant the docs told users to lower; it is `UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT`, and the docs say to raise it;
- found on the way: the first stored setting reset a `--data-dir` or the desktop's archive folder, so verification, sidecars, tags and artwork used the platform default while downloads did not; the data section now survives;
- the desktop now checks the remembered archive folder before the engine opens and asks for another one (ADR 0044);
- MP4 cannot hold the publisher, which `sync` rewrote on every run; it is unsupported there. MP4 covers were added again on every tag write; they are found;
- new tests: the Settings password panel, three refreshes queueing each episode once, the artwork write's crash boundaries, tag round trips through M4A, Ogg Vorbis and Opus.

**Tasks**
1. OPML import/export (with per-podcast policy defaults on import). ✅ built 2026-10-02 ([ADR 0049](DECISIONS/0049-opml-import-and-export.md)): `podcast import|export`, `GET|POST /api/v1/podcasts/opml`, an import section on Discover and an export link in the library.
2. Podgrab migration guide + importer refinements on real user layouts, including reading the tags of foreign files; generic archive import UX. ✅ built 2026-10-03 ([ADR 0050](DECISIONS/0050-podgrab-migration.md)), the web page `/archive/import` on 2026-10-04 ([ADR 0059](DECISIONS/0059-archive-import-in-the-web-ui.md)): Podgrab's real file names, its database (`archive import --podgrab-db`), the tags of foreign files, [`MIGRATION.md`](MIGRATION.md); found and fixed on the way, an import that rewrote an archived episode's record, two files planned onto one episode and two episodes onto one path, a hard-coded `archive.imported`, a declared length of 0 sinking every match, and `archive import --server` cut off after 30 s.
3. Feed-URL changes: an explicit source-migration UI for the announcements that cannot be verified. Detection and verified switching (`new-feed-url`, permanent redirects, GUID match) were built in Phase 3. ✅ built 2026-10-03 ([ADR 0052](DECISIONS/0052-moving-a-feed-by-hand.md)): an unverified announcement is kept as the podcast's announced source and shown by `podcast show`, the API and the podcast page; `podcast move-feed`, `POST /api/v1/podcasts/{id}/move-feed` and "Move to this feed" move a podcast to it, or to any feed URL, after the same check, with `force` when it fails; the library marks a podcast whose feed announces a move.
4. Error recovery UX: orphan reporting, a repair wizard, and resolving candidate duplicates. Retry-failed sweeps exist since Phase 4, and nothing deletes a file. ✅ built 2026-10-03 ([ADR 0051](DECISIONS/0051-candidate-duplicates-and-orphans.md)), the repair of a missing file on 2026-10-04 ([ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)): `archive restore` puts the record's exact bytes back from a folder, `archive redownload` downloads the episode again, both through the API and the web page `/archive/repair`; `episode duplicates|resolve`, `GET /api/v1/episodes/duplicates`, `POST /api/v1/episodes/{id}/resolve` and the two buttons on a candidate's row; `archive orphans` and `GET /api/v1/archive/orphans`, which report the leftovers of every writer, orphan parts, unknown media and stray sidecars and delete nothing; found and fixed on the way, a refresh that updated a candidate's item erased its reasons.
5. Localization readiness (message catalogue in the SPA; English only shipped). ✅ built 2026-10-03 ([ADR 0053](DECISIONS/0053-message-catalogue.md)): every piece of interface text comes from the typed catalogue in `web/src/lib/i18n/en/`; plurals go through `Intl.PluralRules`, dates and numbers through the catalogue's locale, the API's vocabularies through `label()`; `literal-text.test.ts` fails on text written into a Svelte file. Found and fixed on the way: about twenty counts read "1 items", "1 seconds" and the like at one. Choosing between locales comes with the second one.
6. Upgrade migrations tested from every tagged pre-release. ✅ built 2026-10-03 ([ADR 0054](DECISIONS/0054-upgrade-fixtures.md)) for the tags to come, since none exists yet: `scripts/upgrade_fixture.py` records the data directory a build leaves and what its `--json` read commands say, and `crates/uguisu-cli/tests/upgrade.rs` opens every fixture under `tests/fixtures/upgrade/` with the current build — recorded output kept, every byte and manifest verified, the password and token still accepted. The first fixture is the build of `3bb9e20`; tagging a pre-release adds its own (`DEVELOPMENT.md`). The SQL-seeded storage tests now start from every earlier schema, Phase 7's included.
7. Docs: `MIGRATION.md` (written with task 2), full pass over all docs, starting with the drift the audit listed. ✅ done 2026-10-03: every document and ADR read against the code, and what no longer held corrected — about two hundred statements, the audit's list among them, and the ADR index's status column, which now names every amendment on both sides. Where a document described something not built, it now says so. The pass found defects in the code, listed under **Found by the documentation pass**, all fixed since.
8. The rows of [Pending](#pending) and [TODO before v1](#todo-before-v1) that go to Phase 10. ✅ built 2026-10-03: the token panel on the Settings page (listing, creating and revoking API tokens once a password is set); archiving and removing a podcast ([ADR 0055](DECISIONS/0055-archiving-and-removing-a-podcast.md)): `podcast archive|remove`, `POST /api/v1/podcasts/{id}/archive`, `DELETE /api/v1/podcasts/{id}` and both on the podcast page; a removal keeps every file, and `archive reconcile --rebuild` restores its records once the feed is added again; the rest of the scaffolded commands, `episode list|show`; the database's integrity check, vacuum and backup ([ADR 0056](DECISIONS/0056-database-maintenance.md)): `db migrate|backup|check|vacuum` and `POST /api/v1/db/{backup,check,vacuum}`, a backup consistent while the server writes and, through the API, written only into the server's own `backups/`, so no command answers "not implemented" any more; the original tag snapshot, recorded before Uguisu first writes to a file, on the record and in the sidecar, as a record rather than a restore (ADR 0012); `--log-format json`, and no log carries a URL's userinfo, query or fragment ([`SECURITY.md`](SECURITY.md) §3.5); an archived episode whose feed now points at other audio is flagged and its file kept, `archive list --source-changed` (ADR 0015); the library paged, filtered and sorted by the server ([ADR 0057](DECISIONS/0057-library-paged-by-the-server.md)), and an episode page at `/episodes/{id}`; misspelling recall: a term query that found nothing is asked once more with looser queries (ADR 0005); the episode artwork URL and the chapter and transcript references kept in the sidecar, their download after v1 (`PRODUCT.md` v1 #16); the lock holder's pid on Windows, from `uguisu.pid` (ADR 0016); an MSI uninstall removes the login item ([`DESKTOP.md`](DESKTOP.md)). Decided rather than built: fyyd after v1 (ADR 0005), the episode change log kept (ADR 0015), no `config.toml` in v1 (ADR 0028) and no scheduled verification ([`SERVICE.md`](SERVICE.md) §3), no per-podcast tag mode (ADR 0012). Found and fixed on the way: `cargo run -p openapi-doc` overflowed Windows' 1 MiB main-thread stack in a debug build; two log lines carried a failed refresh's detail, a search logged its query whole, and a URL with an apostrophe in its path kept its query in a log; the download queue read every podcast for titles its rows carry; `search --server` and `download reconcile --server` could not read an older server's answer; five tests depended on timing. The MSI change passed Desktop run 37985697410.

**Found by the documentation pass**, all fixed 2026-10-03 with a regression test each:
- a stored setting that needs a restart now takes effect after one: `Engine::open` merges the stored layer before it builds the HTTP clients, the discovery stack, the download queue and the path template;
- `GET /api/v1/events?limit=` without `after` answers the newest events, so the dashboard's "Recent activity" is recent; the server reads them through the engine, not from `uguisu-storage`;
- `archive relocate` moves the download job's `target_path` with the file, and a verified record stays verified as `relocated`, the reason nothing wrote before;
- recovering a `finalizing` job adopts only a file of the recorded size; anything else fails it as `target_exists` and keeps both files;
- a light verification compares the recorded mtime: a moved mtime is `unchecked` (`mtime_changed`), and only a `verified` pass records the mtime it found;
- tag writes: recovery records `unchecked`, a write with nothing to change keeps `written`, and the docs now say a tag write removes its own scratch copy, which is never the only copy;
- `config validate` fails for every stored value that is not in force, the environment-pinned ones included;
- `SETTINGS` describes what the parser enforces; `UGUISU_DISCOVERY_LIMIT` above 100 and an interval above a year are refused;
- a search behind open breakers reports `all_providers_failed`;
- smaller: `/discovery/resolve`'s OpenAPI operations list all seven statuses; a corrupt `host_key` fails its job through the state table; the stale comments are corrected.

Found while fixing them, and fixed: a tag write that failed after its rename cleared the pending marker, so Uguisu's own write would have been reported as a hash mismatch; `db backup` failed on Windows, which refuses to flush a read-only handle.

**Dependencies:** Phases 5, 8, 9, 9a and 9b.

**Tests:** OPML round-trip (✅ `export_then_import_adds_nothing`, `export_moves_to_fresh_library`), candidate duplicates (✅ `merged_candidate_stays_merged`, `separated_candidate_stays_separate`, `both_in_feed_refuse_merge`), the orphan report (✅ `orphan_report_deletes_nothing`, `held_scratch_is_not_reported`), feed-change fixtures (no duplicate podcasts; ✅ `unverified_announcement_is_kept_as_source`, `unverified_move_needs_force`, `verified_move_keeps_episodes`, `move_refuses_another_podcast`), upgrade tests from previous schema versions (✅ `phase3_database_upgrades_keeping_rows` to `phase7_upgrade_keeps_the_service_state`, and `every_fixture_upgrades`).

**Acceptance:** a Podgrab data directory imports without re-downloading; a feed that moves keeps its podcast and history; upgrading from the previous pre-release preserves all data. The first is `podgrab_migration_downloads_nothing` (`crates/uguisu-engine/tests/podgrab.rs`) and the second `a_verified_new_url_keeps_episodes`, `verified_move_keeps_episodes` and `unverified_move_needs_force` (`crates/uguisu-engine/tests/migration.rs`). The third is `every_fixture_upgrades` (`crates/uguisu-cli/tests/upgrade.rs`), which opens the untagged fixture of `3bb9e20` with today's build; the gate against two real pre-releases is Phase 11's (Upgrade).

---

## Phase 11 — v1 Release Candidate

**Objective:** trust. Gates from the brief §53/§54.

**Checklist**
- Reliability: no known data-loss bugs; recovery scenarios (power loss, process crash, container restart, incomplete downloads, unavailable feed/enclosure, disk full, DB restart) scripted and green.
- Discovery: common, obscure, similarly named, misspelled, multi-feed, provider-disagreement, website-only, dead-provider scenarios in the fixture suite.
- Performance: benchmark report for 100/1k/10k feeds, large episode lists, large archives, multi-provider search (latency, startup, refresh, memory, CPU, DB size, throughput, scan speed).
- Security: threat model review against `SECURITY.md`; dependency audit clean.
- Packaging: the multi-stage Docker image and `DOCKER.md` (moved here from Phase 8 in the 2026-09-21 re-scope: image < 60 MB compressed, starts with its health check green), the Windows installer and the Flatpak, each tested from a clean machine, and one native Linux package on a desktop. The Windows run covers Phase 9a's pending items: a logoff closing the engine, autostart after an NSIS upgrade, the quoted login item, `e2e.py`, `desktop_upgrade.py` and a machine without network or WebView2. A container binds `0.0.0.0` inside its namespace, so it inherits ADR 0037: setup sets a password, or the image passes the override deliberately; the image also has to show that SIGTERM drains downloads.
- Upgrade: from the last two pre-releases, which means tagging pre-releases first.
- Archive: verification of an archive with tens of thousands of episodes.
- Documentation: a new user installs without reading source; `TROUBLESHOOTING.md` exists.
- Open items: the rows of [Pending](#pending) and [TODO before v1](#todo-before-v1) that go to Phase 11.

**Found before the checklist and fixed**, 2026-10-04:
- with no password, any web page could fire a body-less mutation by a form post; a change whose `Origin` names another site is refused ([ADR 0058](DECISIONS/0058-refusing-cross-site-changes.md));
- an episode archived by an import or a rebuild could be queued and downloaded again; every enqueue and retry path refuses it while its file is in place, and an import's plan names a job it cannot stop;
- finishing a download, an import or a relocation renamed over whatever had taken the target meanwhile, or a case variant of it; all three now move through a hard link that never replaces a file, falling back to check-then-rename only where links are unsupported (FAT, some network shares), and the import fsyncs the directory;
- `COM¹`–`LPT³` are Windows device names too;
- an episode title of markup only was stored empty; it falls back like a missing one, and stored identities do not move;
- the Archive page's "Check that the files are there" and "Verify every hash" answered 400 unless a podcast was chosen: they sent an empty podcast id, which is now left out; and a row's "Verify" of a missing file then said "Verified.", which now keeps the finding;
- a long CJK or emoji title rendered a file name Linux refuses (over 255 bytes); segments are also cut at 200 bytes ([ADR 0022](DECISIONS/0022-template-grammar-and-path-safety.md), amended);
- a file that took a download's target mid-transfer, a case variant on NTFS included, failed it as `finalization`; it is `target_exists`;
- artwork the network policy refuses answered as a network failure (exit 8); it is `blocked_by_policy` (exit 6), as for a feed;
- the second of two similar shows in a search named the first by its folded key ("the daily").

Scripted since: the discovery scenarios (`tests/fixtures/discovery/README.md`), hostile names on NTFS and ext4, case-only collisions, a newer database refused unchanged, no `Authorization` from the browser in three layers, a real process killed mid-download and mid-import ([`docs/benchmarks/2026-10-04-phase11-verification.md`](benchmarks/2026-10-04-phase11-verification.md)), and the Docker image ([ADR 0061](DECISIONS/0061-the-docker-image.md), [`DOCKER.md`](../DOCKER.md)): 22.8 MB compressed, `scripts/docker_smoke.py` green in nine checks, SIGTERM mid-download parking the job and a restart resuming it with a range request; trusted reverse proxies ([ADR 0062](DECISIONS/0062-trusted-proxies.md)). The threat model review is done, with seven gaps fixed in code and `pnpm audit` clean (`docs/SECURITY.md` §3.10).

Settled rather than fixed: RUSTSEC-2024-0436 (`paste` through lofty) stays accepted, since lofty 0.25.4 still uses it and upstream declined a replacement (`docs/SECURITY.md` §3.10).

**Acceptance:** all items checked; `v1.0.0` tagged.

---

*Status: living document; update the status column as phases complete.*
