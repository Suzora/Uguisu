# ADR 0002 — SQLite via sqlx as the only v1 store

**Status:** accepted, amended 2026-09-17 (Phase 3), 2026-09-18 (Phase 4), amended by [ADR 0016](0016-refresh-coalescing-lock-and-post-commit-events.md), [ADR 0018](0018-download-state-machine-and-queue.md), [ADR 0056](0056-database-maintenance.md) · **Date:** 2026-09-17

## Context

Uguisu must run identically in Docker, as a Windows executable and as a Flatpak, with one binary and no external services, while handling 10k podcasts and 500k+ episodes. The database must be rebuildable from the filesystem.

## Decision

SQLite is the only supported database in v1, accessed through `sqlx` (async, compile-time-checked queries, embedded migrations). Settings: WAL journal, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5s`, one writer connection pool of size 1 plus a read pool. FTS5 for text search. Migrations are forward-only and applied at startup; `uguisu db backup` writes a consistent copy with `VACUUM INTO` rather than the online backup API, which `sqlx` does not expose ([ADR 0056](0056-database-maintenance.md)).

## Consequences

- Zero-dependency deployment; desktop and server share one code path.
- Single-writer discipline: the CLI's embedded mode refuses to run while the server holds the lock file.
- No multi-node deployments; Postgres support would be a later ADR and would need `sqlx`'s `Any` driver or a repository abstraction — `uguisu-storage` has none: its functions take a `SqliteConnection`.
- Benchmarks in Phases 3 and 11 must confirm the 500k-episode targets (SQLite handles this comfortably with proper indexes; the risk is query shape, not the engine).

## Alternatives considered

- **PostgreSQL (like PinePods):** better for multi-user scale, unacceptable operational weight for a desktop app and for the "one binary" promise.
- **`rusqlite` (sync):** simpler, mature; would require a blocking-thread bridge in an async engine. `sqlx` keeps the engine uniformly async and provides migrations; revisit if `sqlx`'s SQLite driver becomes a bottleneck.
- **Embedded KV stores (sled, redb):** no SQL for ad-hoc queries, weaker tooling for users who want to inspect their data.

## Amendment 2026-09-17 (Phase 3)

As built: `sqlx` 0.8.6 (0.9 needs Rust 1.94, above the workspace MSRV) with `runtime-tokio`, `sqlite`, `migrate`, `macros`, `time`, `json`, `tls-none`; queries are plain `sqlx::query`/`query_as` with `FromRow` row structs rather than the compile-time `query!` macros, so CI needs no offline query data. Writer pool of size 1 whose transactions open with `BEGIN IMMEDIATE`; reader pool of 4 read-only connections; WAL, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5s` as decided. Migrations are embedded from `crates/uguisu-storage/migrations` (`0001_phase3.sql`). Timestamps are UTC RFC 3339 at second precision (`…Z`) written by one helper so text order equals time order. FTS5, `settings` and `discovery_cache` are deferred to the phases that use them. The single-writer discipline across processes is enforced by the data directory lock (ADR 0016). `uguisu-core` stays free of `sqlx`: storage decodes rows into the core model.

## Amendment 2026-09-18 (Phase 4)

Migration `0002_phase4` adds `download_jobs` (one row per episode, `episode_id UNIQUE`), `download_attempts` (unique `(job_id, attempt_no)`) and `download_control`, a single-row table holding the global pause so it survives restarts. No `ALTER TABLE`: the migration only creates. Indexes serve the three queue reads — claim (`state, priority DESC, created_at, id`), the retry tick (partial index on `next_attempt_at WHERE state = 'retrying'`) and listing (`podcast_id, created_at DESC, id DESC` and `created_at DESC, id DESC`).

The writer discipline holds under downloads: a running job writes a claim, at most one throttled progress `UPDATE` per second (autocommit, never inside a transaction that spans I/O) and one transaction at the end. Network and file I/O never happen inside a transaction. An upgrade test applies migration 1 alone, inserts Phase-3 rows, reopens the database and asserts both migrations, intact rows, an empty `foreign_key_check` and `integrity_check ok`.
