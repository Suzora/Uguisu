# ADR 0016 — Refresh coalescing, single-process lock and post-commit events

**Status:** accepted, amended 2026-10-03 (Phase 10) · **Date:** 2026-09-17 · amends [ADR 0002](0002-sqlite-storage.md), [ADR 0003](0003-single-binary-cli-over-api.md), [ADR 0010](0010-event-system.md)

## Context

Several callers can ask for the same refresh at once (a user click, the scheduler, `refresh --all`, an API retry). SQLite has one writer. Events must describe committed state, or a subscriber acts on an episode that a rollback removed. The CLI runs stateful commands embedded when no server is given (ADR 0003, amended), so two processes could open the same data directory.

## Decision

- **Coalescing.** The engine keeps a map `podcast_id → shared future` of in-flight refreshes. The first caller spawns the run as a task (it completes even if the caller stops waiting); every concurrent caller awaits the same shared result and receives the same `RefreshReport`. The entry is removed after commit and publish, so the next call starts a fresh run. A caller that joins a run inherits its `force` flag. `refresh_all` goes through the same path with a semaphore (`refresh_concurrency`).
- **Single process per data directory.** `Engine::open` takes `uguisu.lock` with an OS advisory lock (`std::fs::File::try_lock`), writes its pid to a sibling `uguisu.pid` (write, fsync, rename; removed when the lock is released) and holds the lock until the last engine handle drops. `uguisu serve` and every embedded stateful CLI command hold it; a second holder fails immediately with `locked` and a hint to use `--server`. `feed inspect`, `search` and `resolve` need no lock. The lock replaces the per-row sequence numbers a multi-process design would need. The pid lives outside the lock file because a Windows lock is mandatory: a refused process cannot read the file another one holds, so it could not name the holder.
- **Post-commit publication.** Events are appended to the `events` table inside the transaction that produced the state and published on the broadcast bus only after that transaction committed. `podcast.feed.refresh.started` is committed in its own small transaction before the fetch so `feed status` shows the attempt.

## Consequences

- No duplicate fetches under concurrent requests; no lost updates between two refreshes of one podcast; other podcasts refresh in parallel up to the concurrency bound.
- A crash between commit and publish loses the *live* notification but not the stored event; SSE consumers can catch up via `GET /api/v1/events?after=`.
- The lock is advisory: a process that ignores it (not Uguisu) can still corrupt the single-writer assumption; `busy_timeout` and `BEGIN IMMEDIATE` limit the damage.
- Desktop and server topologies are unchanged: the Tauri shell hosts the same server, so exactly one process owns the directory.

## Alternatives considered

- **Per-podcast async mutex:** serializes but does not coalesce — the second caller would refetch.
- **Database-level fetch sequence numbers:** needed only for multi-process writers, which the lock rules out.
- **Publish before commit:** simpler, but subscribers would observe phantom state on rollback.
