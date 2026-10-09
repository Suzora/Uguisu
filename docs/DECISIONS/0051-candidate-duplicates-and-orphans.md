# ADR 0051 — Resolving candidate duplicates, and reporting what nothing owns

**Status:** accepted, amends [ADR 0006](0006-episode-identity.md), [ADR 0014](0014-identity-fallback-and-guid-changes.md), [ADR 0024](0024-sidecars-and-manifests.md) · **Date:** 2026-10-03

## Context

ADR 0014 stores an item that contradicts a stored episode's identity as a **candidate duplicate**: a new episode with `duplicate_of_episode_id`, `skipped`, never downloaded. Resolving one was left to "a user action", deferred from Phase 3 to 8 and from 8 to 10, and never built. So a feed that rewrites its GUIDs leaves every candidate skipped for good, while the original, whose item is gone, drifts towards "removed from feed" although the episode is still published. And a refresh that updated a candidate's item erased its `duplicate_reasons`: the upsert wrote the empty list every new row starts with.

ADR 0006 also names "the downloaded content hash" as a signal for candidates, and `DATA_MODEL.md` §4 said content-hash matches were flagged after download. Nothing ever did.

ADR 0024 and `layout::tmp_suffix` say leftovers are reported. Five writers leave scratch files after a crash or an error — an import's `.import` copy, a tag write's `.tagtmp` (after a crash only, since a write removes its own copy of a file it never touched), and the `*.<pid>.<n>.tmp` of manifests, sidecars and artwork — and a relocation leaves the old sidecar behind. Only download `.part` files without a job were reported; nothing listed the others, nor media files without a record.

## Decision

### Two resolutions, made by a person

A candidate is resolved one at a time, as **`same`** or **`separate`**: `uguisu episode duplicates|resolve`, `GET /api/v1/episodes/duplicates`, `POST /api/v1/episodes/{id}/resolve`, and two buttons on the candidate's row in the episode list. Nothing resolves a candidate on its own.

### `same`: the candidate merges into the original

The original keeps its id, identity key and source, `first_seen_at`, archive state, file, sidecar, manifest entry and job. The candidate's row is deleted; its enclosures, extras and search entry go with it, and its change rows move to the original first. Candidates that named the deleted one are repointed to the original.

What the original takes over depends on which of the two the feed showed last:

| Last seen | Result |
|---|---|
| the candidate (a rewritten GUID) | the original adopts the candidate's GUID, signal keys and presence, so the next refresh matches it by GUID without contradiction (ADR 0014's same-episode path) and updates its content with the usual change rows |
| the original (the candidate's item left again) | nothing is adopted |
| both, by the same fetch | refused: both items are in the feed, and a merge would turn the second into a new, downloadable episode on the next refresh |

Sightings are stored to the second, so the missed-fetch streak breaks a tie: one fetch saw both items only when both carry the same sighting and neither was missed since.

A merge is also refused when the episode is not a candidate, and when the candidate has an archive record (an import can give it one): a merge never drops a file or its record. A candidate never has a download job, because the queue refuses candidates and an episode becomes one only when it is inserted.

This is the first code path that deletes an episode row. It never touches a file.

### `separate`: the candidate becomes an episode of its own

The link and the reasons are cleared. `skipped` becomes `expected` only when the skip was the candidacy; a candidate that an import archived stays `archived`. The podcast's archive policy then runs for it as for a discovered episode, so an `auto` policy queues it.

### What is recorded

Each resolution writes a `duplicate_resolved` change row on the episode that remains (old value the other id, new value the resolution; no fetch), a `guid` row when a GUID was adopted, and one `episode.duplicate_resolved` event. Events about a merged candidate stay in the log.

`duplicate_of_episode_id` and `duplicate_reasons` are written when a row is inserted and afterwards only by a resolution, as `archive_state` and `skip_reason` already were by the archive engine.

### Content-hash duplicates are not flagged

A hash match exists only once both episodes hold a file, and the one resolution that removes a duplicate must refuse an episode with a file. A flag nobody can act on is noise. The identity signals catch the duplicates a feed creates before anything is downloaded, and an import reports a copy of archived bytes as `already_present`. The unused lookup that was to serve the flag is removed.

### `archive orphans`: what nothing owns, reported

One read-only report, `uguisu archive orphans` and `GET /api/v1/archive/orphans`, walks the media root and counts, with up to a hundred paths each:

- **leftovers** — every file in `.uguisu/tmp/`, and files named by `tmp_suffix` under `.uguisu/artwork/`, `.uguisu/manifests/` and in the tree;
- **orphan parts** — `.part` files no download job owns;
- **unknown media** — media files no archive record names;
- **stray sidecars** — `<name>.<media extension>.json` whose media file is absent; a user's `notes.json` is not one;
- **unreadable** — what the walk could not enter.

A scratch file a writer of this process still holds is not a leftover: every writer holds its scratch name in a process-wide registry from before the file exists until it is renamed or abandoned. One process owns a data directory (ADR 0016), so outside it there are no live writers to exclude. The records and job ids that judge a file are read after the file was seen, so a download or import finishing during the walk is not reported. The report never runs at start-up, records no event, deletes nothing, and exits 1 when it found anything.

## Consequences

- A feed that rewrites every GUID needs one resolution per episode. Noisy, but each is one command or one click.
- A wrong `same` cannot be undone: after the next refresh the original's metadata follows the candidate's item. The web UI asks once more before a merge.
- The original's sidecar keeps the old GUID until it is written again. A rebuild matches by episode id and identity key, which do not change.
- An unresolved candidate makes its episode `ambiguous` to an archive import; resolve candidates before importing.
- A host that brings the original's old item back next to the new one after a merge produces a new episode.
- The orphan report compares paths exactly. On a filesystem that ignores case or normalizes Unicode, a file written under another spelling can show up as unknown. It walks twelve levels deep and reports a symlink instead of following it.
- A media root shared by two data directories would report each other's work. That setup is unsupported already.

## Alternatives considered

- **Merging automatically** on a strong signal. Rejected by ADR 0006 and 0014: a silent merge fuses distinct episodes.
- **Letting the person choose which episode survives.** Rejected: the original holds the file, the history and the job; a surviving candidate would need all of them moved.
- **A tombstone instead of deleting the candidate.** Rejected: it would stay in lists, counts, search and the import index, and the original would never be matched again.
- **A suppression table**, so a merge holds while both items are in the feed. Rejected for now: refusing that case is simpler and loses nothing.
- **Copying the candidate's content into the original.** Rejected: the next refresh does it with change rows.
- **Flagging hash matches.** Rejected: see above.
- **Telling live scratch files by pid or modification time.** Rejected: in a container the server is pid 1 on every start, and times depend on the clock.
- **A lock between the report and the writers.** Rejected: it would stall sidecar writes for the length of a walk.
- **Walking the tree at start-up.** Rejected: every embedded CLI command opens the engine.
- **Loading every record before the walk.** Rejected: memory grows with the archive, and a file registered during the walk would be reported.
- **Any cleanup option.** Struck by design: nothing deletes media.
