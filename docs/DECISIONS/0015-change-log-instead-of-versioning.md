# ADR 0015 — Episode change log instead of row versioning

**Status:** accepted, amended 2026-10-03 (Phase 10), amended by [ADR 0055](0055-archiving-and-removing-a-podcast.md) · **Date:** 2026-09-17

## Context

Users want to know what a host changed ("why did the title change?", "was the audio replaced?"), Phase 4 wants to react to enclosure changes, and the brief asks for change detection with a record. Full row versioning (a history table with a copy of every episode version) doubles storage for every edit and complicates every query.

## Decision

- `episodes` holds the current state only. Every refresh that changes a stored episode appends one row per changed field to `episode_changes` (`id, episode_id, podcast_id, fetch_id, changed_at, field, old_value, new_value`), values truncated to 1 KiB. The comparable fields are the ones in the content hash (title, subtitle, description HTML, link, publication instant, duration, season, episode number, type, explicit, artwork, author, enclosures, extras) plus `guid`, `malformed` and the `identity_signals` note of ADR 0014.
- The log is append-only and keyed by `fetch_id`, so `feed_fetches` and `episode_changes` together answer "what did the refresh at 10:42 change?". A duplicate resolution also writes rows, with no `fetch_id`, and a merge moves the candidate's rows to the original ([ADR 0051](0051-candidate-duplicates-and-orphans.md)).
- `content_hash` decides *whether* an item changed in O(1); the diff is computed only for changed items by loading their stored rows inside the refresh transaction.

## Consequences

- Storage grows with edits, not with refreshes; unchanged feeds write nothing.
- Old values are available for explanations and for "enclosure changed after download", but a full previous row cannot be reconstructed byte for byte (values are capped). That detection is built (Phase 10) on the refresh's own diff rather than on the log: a changed primary enclosure of an archived episode sets `archive_files.source_changed_at` and emits `archive.source_changed`, and the file is kept (`ARCHIVE_ENGINE.md` §8). No route, command or view shows the log itself yet.
- Retention: the log is kept for the life of its episode (decided in Phase 10). It grows with edits rather than refreshes, and it carries people's decisions (`duplicate_resolved`). Housekeeping prunes events, never this log; a removed podcast takes its rows with it (ADR 0055), and `db vacuum` reclaims the space.

## Alternatives considered

- **History table with full row copies:** simple to query, expensive on every edit, and most edits touch one field.
- **JSON diff blobs per fetch:** compact but hard to query by field.
- **No log, events only:** events are retained for 30 days by default; the change log must outlive them.
