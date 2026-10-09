# ADR 0025 — Rebuilding the index, and importing someone else's archive

**Status:** accepted, amended by [ADR 0050](0050-podgrab-migration.md) · **Date:** 2026-09-19 · relates to [ADR 0021](0021-archive-file-and-verification.md), [ADR 0024](0024-sidecars-and-manifests.md)

## Context

Two operations read a directory tree Uguisu did not necessarily write. `reconcile --rebuild` reads the user's own archive after the database is gone. `archive import` reads an archive another tool produced — Podgrab first — and copies what it can place. They share a scanner, a threat model and a failure mode: both can destroy data by being confident about something they cannot actually know.

## Decision

### Both default to a dry run

`--apply` is how a user says they have read the plan. The dry run does the whole scan and match, so the plan they read is the plan that would run.

### Rebuild never invents anything

A record is restored only when the document parses, the media file it names exists, the path is free or already this episode's, and the episode exists in the library. Everything else is counted and named:

- a document whose media file is gone,
- one that does not parse or claims an unreadable schema,
- one naming an episode this library does not have,
- one whose path another episode owns,
- one that contradicts a record Uguisu has **checked**.

That last case is the important one. A `verified` record was compared against the bytes; a sidecar was not. It is a conflict, not an update.

Resolution goes through two doors, because after a total loss the identifiers no longer exist: the podcast by identifier, then by the feed URL the document records; the episode by identifier, then by its stable identity key. Without the second door a rebuild after a real disaster would restore nothing, since re-adding the feed gives every episode a fresh identifier.

### Import copies and never touches the source

No move mode exists. The source tree is opened read-only, and a test hashes it whole before and after every operation — including the one that copies out of it. If a move mode is ever added it will be explicit and separately documented.

### An ambiguous file is never imported

This is the rule the matcher is built around: **a wrong import is worse than an unresolved one.** A file placed under the wrong episode is a quiet, permanent error that nobody notices until they play it; an unmatched file is a line in a report. So there is no best-guess outcome.

Matching is a pure, deterministic function with no I/O and no randomness, and ties break on a total order, so the same directory yields the same verdicts on every run and every machine.

| Signal | Weight | Notes |
|---|---:|---|
| title | 40 | folded, then the better of a token-set ratio and Jaro–Winkler |
| publication date | 25 | same day 1.0, ±1 day 0.85 (time zones), ±3 days 0.5, beyond 0 |
| season and episode number | 15 | a contradiction scores 0 |
| duration | 12 | within 2% or 30 seconds is the same recording |
| declared size | 8 | smallest weight: `enclosure.length` is often simply wrong |

Weights are **renormalized over the signals available on both sides**. Without that, a feed carrying no season numbers would cap every score below the threshold and nothing would ever import.

Two mechanisms carry the hard cases. The integers in a title are compared separately and a disagreement multiplies the title score by 0.6, because `Teil 1` and `Teil 2` are about 0.97 alike to any string metric. And two dates more than three days apart cap the total below any usable threshold, so a rerun titled identically to the original can never be matched on its title.

A match needs the configured threshold (85% by default) **and** a ten-point margin over the runner-up. Two episodes that explain a file equally well mean the file does not identify either of them, however high the raw score is.

### The podcast is decided per directory

Not per file. That bounds the memory an import needs to one podcast's episode index rather than the library's, and it means an unrecognisable directory is reported once instead of once per file inside it. The gate uses the same threshold-and-margin shape, and the same comparison function — a second, subtly different one would have let the two disagree.

### Scanning streams and never follows a link

`walkdir` with `follow_links(false)`, control directories pruned so the walk never descends into them, symlinks and unreadable entries yielded as items rather than ending the scan. An archive of 500 000 files costs what one of 500 does, and a planted symlink cannot walk Uguisu out of the root it was given.

Reports count and name at most a hundred paths per finding group. A list of half a million paths would be a worse problem than the one it describes.

### Execution order, and what a crash leaves

Copy into `.uguisu/tmp/` while hashing → `sync_all` → rename into the rendered target → register → sidecar → mark the manifest stale.

The target is chosen in two steps, and the order matters. First the template's natural path: if those exact bytes are already there, that is what a crash between the rename and the registration leaves, and the run finishes the record instead of copying the file again. Going straight to collision handling would have created a suffixed duplicate as the *recovery* for a crash, which is worse than the crash. Only when the path holds something else does Phase-5 collision handling pick a free one; when the disambiguated form is taken too, it is a reported conflict and nothing is written.

An imported record carries `origin = import` and `unchecked`/`imported`: the bytes were hashed as they were copied, which makes the record true, but nobody has read them back since.

## Consequences

- A user who lost their database can get it back, and knows exactly what has and has not been checked.
- A user migrating from Podgrab can look at the plan before anything is copied, and their old archive is still there afterwards, byte for byte.
- Some files will not be imported that a human would have matched. That is the intended direction of the error, and the report names them so the user can act.
- A new foreign layout is one file implementing `SourceFormat`; nothing outside `podgrab.rs` knows the word "podgrab".
