# ADR 0024 — Sidecars, manifests and where they live

**Status:** accepted, amended by [ADR 0051](0051-candidate-duplicates-and-orphans.md) · **Date:** 2026-09-19 · relates to [ADR 0007](0007-integrity-hashing.md), [ADR 0021](0021-archive-file-and-verification.md), [ADR 0022](0022-template-grammar-and-path-safety.md)

## Context

After Phase 5 the SQLite database is the only index over the archive. Lose it — a disk failure, a bad restore, a user who deleted the data directory and kept the media — and the files are anonymous bytes: correct, complete, and unattributable. ADR 0007 already committed to SHA-256 manifests and algorithm-tagged sidecars; this decides their format, their location and when they are written.

Three things have to be true at once. The archive has to describe itself well enough to be rebuilt. The description has to be checkable by something other than Uguisu. And neither may put a write on the path of a download.

## Decision

### The sidecar sits beside its media file

`<media file>.json`, derived from the final media name and never templated on its own. That is what makes the two impossible to separate: copy one episode out of the archive and its metadata goes with it, which is the entire point of having a sidecar rather than an export.

It records the podcast, the episode, the archive record and the provenance. Two fields exist specifically for the disaster case: the **feed URL** and the episode's **identity key**. After a total loss the podcast is added again and the feed refreshed, so every identifier in every document names something that no longer exists, and those two are the only things still pointing at anything.

Forward compatibility is asymmetric on purpose. An unknown **field** is ignored, because a newer Uguisu adding one must not make its sidecars unreadable. An unknown **schema** is refused by name, because that is a statement this build cannot interpret and guessing would be worse than saying so. Reading is capped at 256 KiB: a rebuild reads whatever is on disk, and a scan of an untrusted archive must not become an out-of-memory because one file has a `.json` name.

### A sidecar is metadata, not evidence

It says what Uguisu knew when it was written. That is not the same as having read the bytes since, so a record rebuilt from one is `unchecked` with reason `rebuilt`, and only a verification may ever write `verified`. This is the single most important sentence in the phase: without it, a rebuild would launder a claim into a fact.

### Manifests live under the media root, not under a podcast directory

`<media>/.uguisu/manifests/<podcast-id>/manifest.sha256`.

`docs/DATA_MODEL.md` §5 sketched `<podcast dir>/.uguisu/manifest.sha256`. That cannot be implemented as written, because **there is no podcast directory**. Where a podcast's files land is decided entirely by the path template (ADR 0009), which a user may point at `{episode.year}/{podcast.title}/…` — podcast directories nested under year directories — or at a flat layout with no podcast directory at all. `podcasts.directory_name` exists in the schema and is `None` at every construction site; nothing computes it.

Anchoring at the media root also means a template change never strands a manifest: it only marks it stale. And one reserved top-level name (`.uguisu/`) gives every scanner one exclusion rule instead of one per asset kind.

That reservation required a Phase-5 correction: `sanitize::segment` preserved a leading dot, so a podcast titled `.uguisu` rendered a segment that shadowed Uguisu's own bookkeeping. It is escaped to `._uguisu`, on every profile — the collision is a property of the archive layout, not of an operating system.

### The manifest is rendered from the index, and says so

GNU coreutils' checksum format, two spaces, paths relative to the **media directory** so they are byte-identical to `archive_files.relative_path`. `cd <media directory> && sha256sum -c …` checks a podcast without Uguisu, which is the whole reason for the format.

It is **not** independent evidence, and the header says so where `sha256sum -c` skips it as a comment. Re-hashing while writing was considered and rejected on three counts: it proves nothing to the external tool the format exists for; it costs hours of disk I/O on every registration; and — decisively — it would have nowhere to record a disagreement it found. If a hashing writer read a file and got a different answer than the row, writing the disk hash would contradict the database and writing the row hash would make the hashing pointless. There is no good answer, which is the tell that the operation is wrong. Reading the bytes is `archive verify --full`'s job: it already batches, streams with a bounded buffer, and writes its findings onto the record.

GNU escaping is implemented even though Uguisu's own paths never need it, because an imported name can carry a newline — unescaped it would turn one entry into two.

### Manifests are marked stale and written in batches

A change marks the podcast's manifest stale **inside the transaction that made the change**. The flag and the fact commit together, so a crash can only ever leave "marked stale but actually fresh" — never a manifest that silently disagrees with the archive.

The file is written when the event bus has been quiet for five seconds, on explicit command, and on `close`. Writing one per finished episode would mean five hundred rewrites of a growing file for a five-hundred-episode backlog. `mark_written` is guarded on the row's timestamp, so a registration landing while the file is being rendered leaves it stale for the next flush rather than being lost.

The guarantee worth stating: **after a clean close, every manifest agrees with the index.** Between closes, `archive manifest status` says which have fallen behind.

### Every atomic write uses a unique temporary name

Both writers go through a temporary file in the target's own directory, then a rename. The temporary name is unique per writer (process id plus a counter). A *fixed* name made concurrent writes race: two writers of one manifest took turns renaming each other's file away and the loser failed with "no such file".

The cost is that a crash can leave more than one leftover per target instead of exactly one. Leftovers are reported and never removed, which is the same rule as everywhere else in Phase 6 — a scratch file silently deleted is a copy of somebody's media that nobody got to look at. The one exception is a tag write's copy of a file it never touched, which the write itself removes when it fails or changes nothing (`ARCHIVE_ENGINE.md` §20).

## Consequences

- The database can be lost and rebuilt from disk, and the rebuild is honest about what it has and has not checked.
- A user can verify their archive with `sha256sum`, with no Uguisu involved and no trust in it required.
- A manifest is at most one flush behind the index, and always correct after a clean shutdown. A stale one is visible rather than silent.
- `docs/DATA_MODEL.md` §5's sketch is superseded; the sketch's location is not implementable.
- One Phase-5 behaviour changed: a podcast whose title begins with `.uguisu` now renders a different path. Vanishingly rare, and a migration note rather than a silent relocation.
