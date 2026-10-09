# ADR 0027 — The service, and the refresh scheduler

**Status:** accepted · **Date:** 2026-09-21 · relates to [ADR 0016](0016-refresh-coalescing-lock-and-post-commit-events.md), [ADR 0018](0018-download-state-machine-and-queue.md), [ADR 0022](0022-template-grammar-and-path-safety.md), [ADR 0023](0023-archive-policy.md)

## Context

Through Phase 6, nothing happened unless somebody typed a command. `podcasts.next_refresh_at` had been maintained since Phase 3 and read by nobody; `uguisu serve` opened the engine, started the download workers and ran the HTTP server. An archiver whose archive only grows while a person is watching it is a toolbox.

## Decision

### `serve` is the only long-lived process

`uguisu serve` starts the download workers, the feed-refresh scheduler and the background search-index build. Every other command does what it was asked and exits. No scheduler hides in `podcast add`, in a refresh, in a download or in an HTTP request.

Shutdown is one cancellation token: cancel, park the download queue, join the scheduler under the shutdown grace, join the index build, join the archive watcher, flush manifests, close the pools. In-flight refreshes see the cancel through `RefreshOptions.cancel`, so a fetch aborts while a transaction that is mid-commit finishes.

### The loop is the download scheduler's, with feeds in place of jobs

Return on shutdown; read the persisted pause; fill the free capacity from the due query; sleep until the earliest of the next feed, the next spacing floor and the next housekeeping pass, clamped to `[1 s, 15 min]`; wake early when a slot frees or a due time changes. An idle process wakes at most every fifteen minutes and asks three indexed questions.

Refreshes go through `Engine::refresh_podcast` like every other caller, so an automatic refresh and a manual one are the same code path, the same HTTP stack and the same SSRF policy. **The scheduler opens no network path of its own.** ADR 0016's coalescer is what keeps a tick that collides with a `podcast refresh` from fetching the feed twice — the scheduler needs no lock, and adds none.

### The due set is filtered in SQL, and it is two queries

A pass asks for the podcasts that are due *and not already being refreshed*, the way the download scheduler asks for jobs on hosts that are not saturated. Filtering afterwards would spend a `LIMIT` on work already in flight and leave real capacity idle until the next tick.

"Never fetched or overdue" reads as one `OR`, but SQLite cannot answer it from one ordered index seek: the status predicate is already an `OR`, and a second one over the time column makes the planner fall back to `idx_podcasts_status_next` plus a sort of every active podcast — on every wake, for the life of the process. Asked separately, each half is a seek into the partial index `idx_podcasts_due` that returns rows in order; the halves are disjoint, so concatenating them needs no de-duplication, and "never fetched" comes first because that is what maximally overdue means. `INDEXED BY` pins it and an `EXPLAIN QUERY PLAN` test fails if an edit loses it.

**No claim column.** A compare-and-set claim would duplicate the coalescer and add a stale-claim recovery story for runs that last seconds and are idempotent. One process owns a data directory (the lock file), and within it the coalescer arbitrates.

### Due times are spread deterministically, and written only by a refresh

`next_refresh_at` is written at the three points where a refresh ends, and nowhere else — the scheduler never rewrites due times, so a restart costs no writes and a hand-set time keeps meaning what it says.

Every one of those writes goes through `schedule::plan_next`, which adds the podcast's own share of the interval:

- **Ordinary:** `now + interval + 0…10 % of interval`.
- **Catching up** (overdue by more than one interval, or never fetched): `now + interval + 0…100 %`.

The share comes from the identifier, so the same library produces the same schedule on every machine and a scheduling bug is reproducible from the database alone. It is a SplitMix64 mix of the ULID's random tail rather than that tail modulo 1000: the generator increments within a millisecond, so podcasts added in one batch would otherwise land in adjacent slots — the herd the spread exists to break up.

**The spread is one-sided.** `ARCHITECTURE.md` said "±10 %"; this ADR amends it. Subtracting could fetch earlier than the origin's `Cache-Control: max-age` allowed, and honouring that header is deliberate.

That is also what handles a week of downtime: the drain is rate-limited by the capacity, and because every drained row is replanned in catch-up mode the library de-correlates after one pass instead of re-synchronising into a six-minute window.

### Pause is two separate decisions

A `scheduler_control` singleton stops automatic refreshing, persisted across restarts. It is deliberately **not** `download_control`: a full disk pauses transfers by itself, and an operator who stops transfers is not asking the library to go stale. `PodcastStatus::Paused` takes one show out of the schedule, which the due query excludes by construction; a manual refresh of a paused podcast still runs and leaves it paused, and resuming lands inside the next interval, spread, so resuming two hundred podcasts is not two hundred simultaneous fetches.

### Failure never stops the loop

A per-podcast error is logged and releases its slot. A sixty-second floor sits under every start, so a podcast whose refresh fails *before* the pipeline can write a back-off cannot spin. A storage error is treated exactly like a pause — warn, start nothing, look again — because returning would end automatic refreshing for the life of the process because one query failed once.

### Housekeeping rides in the same task, on its own due time

The wake instant is the earlier of the next feed and the next maintenance pass, computed independently, so an empty library does not stop housekeeping and a busy one does not delay it. A second task would need its own shutdown join, its own pause semantics and its own contention story with the single writer, for work that runs once a day and takes milliseconds.

It runs whether or not the scheduler is enabled and whether or not it is paused: both of those say "stop fetching things", and neither is a request to let the event log grow without bound. It prunes events past the retention limits and drops expired discovery-cache rows — derived data only — and records `last_maintenance_at` in the same transaction, so a restart works out when the next pass is due instead of running one on every boot. Both limits treat `0` as "no limit", never as "delete everything".

## Consequences

Turning on the scheduler is what finally makes the Phase-5 archive policy fire, which ADR 0023 anticipated. `UGUISU_ARCHIVE_AUTO_DOWNLOAD` stays `false`, policy semantics are untouched, enabling a policy still backfills nothing, and a failed refresh still enqueues nothing — policy runs only on the committed body path.

`podcasts.next_refresh_at` stops being decoration. A library upgraded from Phase 6 has due times in the past and drains at the configured concurrency on the first `serve`.

## Alternatives considered

**A claim column with a lease.** Correct for several processes; there is one. It buys nothing the lock file and the coalescer do not already give, and costs a recovery story.

**A cron-style schedule per podcast.** Expressive, and wrong for the problem: publishers do not publish on the hour, and a per-podcast interval with a deterministic spread is both simpler and kinder to origins.

**Random jitter.** One line shorter and not reproducible. A schedule that differs per process is a schedule nobody can debug.
