# ADR 0010 — In-process event bus with persisted event log

**Status:** accepted, amended 2026-09-17 (Phase 3), 2026-09-18 (Phase 4), amended by [ADR 0016](0016-refresh-coalescing-lock-and-post-commit-events.md) · **Date:** 2026-09-17

## Context

The UI needs live progress, users need a per-episode history ("why was this skipped?"), and future integrations (webhooks, ntfy, Discord, media servers) must attach without coupling to the downloader.

## Decision

- `uguisu-core` defines a single `Event` enum with typed payloads (`podcast.added`, `podcast.feed.updated`, `podcast.feed.failed`, `episode.discovered`, `episode.skipped`, `episode.download.{queued,started,progress,completed,failed}`, `episode.metadata.updated`, `archive.file.{verified,missing,modified}`, `discovery.provider.{degraded,recovered}`, …). The kinds that exist are `EventKind`'s in `uguisu-core::events` (see the amendments); `discovery.provider.*` has none.
- `uguisu-engine` publishes on a `tokio::sync::broadcast` channel (lagging subscribers drop, never block producers) and appends every non-progress event to the `events` table with retention.
- Consumers in v1: SSE endpoint for the UI, structured logging, the episode timeline view. The timeline view is not built: `uguisu episode show` shows an episode's state, not its events, and no route or web view reads one episode's events. Webhooks/notifications subscribe to the same channel post-v1.
- High-frequency `progress` events are throttled (≥ 500 ms per job) and not persisted.

## Consequences

- One integration point; no shell hooks in v1.
- In-process only — a second process cannot subscribe; acceptable for the single-binary design. A durable queue would be a later ADR.

## Alternatives considered

- **Callbacks/exec hooks (podcast-dl style):** simple but insecure and untyped.
- **External broker (Redis/NATS):** operational weight contrary to the one-binary promise.

## Amendment 2026-09-17 (Phase 3)

The wire shape is fixed: `{ schema: 1, id (ULID), occurred_at (RFC 3339), podcast_id?, episode_id?, kind, ...payload }`, one flat object per event; kinds are dotted names and the `Event`/`EventKind` types in `uguisu-core::events` are the schema. Phase 3 kinds: `podcast.added`, `podcast.feed.refresh.started`, `podcast.feed.refresh.completed`, `podcast.feed.refresh.failed`, `podcast.feed.not_modified`, `podcast.metadata.updated`, `episode.discovered`, `episode.updated`, `episode.removal_detected`, `episode.identity_ambiguous`, `feed.url.change_detected`, `feed.url.changed` (payloads in `docs/FEED_ENGINE.md` §11). The original sketch's `podcast.feed.updated`/`podcast.feed.failed` are these `podcast.feed.*` kinds. Events are written inside the producing transaction and published after commit (ADR 0016). `episode.discovered` means "this episode exists in the feed" and never "download it". Subscribers that lag skip the lost events with a warning; `GET /api/v1/events` streams live events over SSE or lists stored ones with `after`/`limit`. Retention (`events::prune`) runs in the scheduler's maintenance pass, daily by default ([ADR 0027](0027-service-and-refresh-scheduler.md)).

## Amendment 2026-09-18 (Phase 4)

Download kinds: `download.queued`, `download.started`, `download.progress`, `download.paused`, `download.resumed`, `download.retry_scheduled`, `download.completed`, `download.failed`, `download.cancelled`, `download.paused_all`, `download.resumed_all` (payloads in `docs/DOWNLOAD_ENGINE.md` §12). The original sketch's `episode.download.*` names are these `download.*` kinds.

Every payload but `download.paused_all`'s and `download.resumed_all`'s carries `job_id`; there is no `events.job_id` column, because the job is a payload fact and the envelope keeps `podcast_id`/`episode_id` for the entities. `EventKind::is_transient()` marks `download.progress` (and `scheduler.tick`): it is published on the bus and never written to the table, so the event log does not grow with bytes transferred. The throttle is one report per job per `progress_interval` (default 1 s) and only after 1 MiB or 1 % of movement, which also bounds broadcast traffic. `GET /api/v1/events?exclude=download.progress` lets a consumer drop kinds it does not want; the comma-separated list applies to the live stream.

Failure payloads carry a typed `error_kind` plus a detail that never contains headers or a URL query; the enclosure URL appears only in `download.queued` and `download.started`, where it is the user's own input.
