# ADR 0055 — Archiving and removing a podcast

**Status:** accepted, amends [ADR 0015](0015-change-log-instead-of-versioning.md) · **Date:** 2026-10-03

## Context

`uguisu podcast remove <id> [--delete-files]` has answered "not implemented" since Phase 3. The `archived` podcast status exists in the model, the API and the library's filter, and the `disabled` fetch state in the model and `STATE_MACHINES.md`, but nothing sets either. `--delete-files` contradicts the one rule Uguisu does not bend: nothing deletes a media file.

## Decision

### Archiving

`uguisu podcast archive <id>`, `POST /api/v1/podcasts/{id}/archive` and "Archive" on the podcast page: the show has ended, or the person no longer wants it fetched, and wants to keep it.

- The podcast becomes `archived` and its current source `disabled`.
- It is never refreshed: the scheduler and `podcast refresh --all` skip it, and a refresh asked for by name is a conflict. Without refreshes the archive policy never queues anything for it.
- Everything else stays as it is: episodes, files, records, history. An episode can still be downloaded on request.
- `podcast resume` brings it back like a paused podcast: `active`, the source `never_fetched`, due somewhere in the next interval.

There is no separate way to disable a source. Archiving is that, and `disabled` now means it.

### Removing

`uguisu podcast remove <id> --yes`, `DELETE /api/v1/podcasts/{id}` and "Remove from the library" on the podcast page, which asks first.

- The podcast's rows go, with everything the database holds about it: sources, episodes, archive records, download jobs, the policy, artwork and manifest rows, the change and fetch logs, its search entries.
- **Every file stays**: media, sidecars, manifests, artwork, partial downloads.
- A removal is refused while one of its downloads is running or finalizing; a queued or paused one goes with it.
- `podcast.removed { title, feed_url, episodes, files }` records it in the event log, which keeps the podcast's earlier events.
- The kept files are reported by `archive orphans` as unknown media and orphan parts. Adding the feed again and running `archive reconcile --rebuild --apply` restores the records from the sidecars, which name the feed and each episode's identity key.

There is no `--delete-files`. Removing a podcast's files is the person's own `rm`.

## Consequences

- A removal cannot be undone from the database: the change log, the fetch log and every decision about the podcast's episodes are gone. The files and their sidecars are what survives, and they are enough to rebuild the archive's records.
- The change log of ADR 0015 is append-only except for a removal, which takes the podcast's rows with it, and a merge (ADR 0051).
- `STATE_MACHINES.md` §5 changes: `archived → active` is allowed, through `resume`.
- The CLI asks for `--yes` before a removal, with or without `--json`.

## Alternatives considered

- **Deleting the files on request.** Rejected: nothing deletes media.
- **A soft delete** that hides the podcast. Rejected: that is archiving, under another name.
- **Disabling a source on its own**, separately from the podcast. Rejected: a podcast has one current source, so the two would always be set together.
