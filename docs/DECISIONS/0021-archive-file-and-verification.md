# ADR 0021 — The archive record and what verification may do

**Status:** accepted · **Date:** 2026-09-18 · relates to [ADR 0007](0007-integrity-hashing.md), [ADR 0018](0018-download-state-machine-and-queue.md)

## Context

Phase 4 leaves a file on disk and a `completed` job row. That is enough to say a transfer finished; it is not enough to say the archive is intact a year later. Nothing records what the file is supposed to be once the job row has aged out of anyone's attention, nothing notices a file deleted by a backup restore or edited by a tagger, and nothing tells the two apart.

Phase 5 needs a record of the artifact, a way to check it, and — most importantly — a clear rule about what checking is allowed to change.

## Decision

### One active record per episode

A new `archive_files` table, `episode_id` unique, `relative_path` unique. Registration is an upsert keyed on the episode, so a repeat or a concurrent registration converges on one row instead of racing; the row keeps its identifier and its `registered_at`. History of moves lives in the event log, not in extra rows.

The record separates three kinds of fact:

- **Immutable**: `episode_id`, `podcast_id`, `hash_algo`, `hash_value`, `size_bytes`, `registered_at` — they describe the bytes that were downloaded.
- **Derived**: `relative_path`, `content_type`, `sniffed_type`, `mtime_unix`.
- **Verification**: `verification_state`, `verification_reason`, `verified_at`.

The stored hash is the download's, computed while the bytes streamed in. Verification compares against it and never replaces it.

### Verification is read-only, without exception

A pass may read the file and write the database. It may not write, truncate, rename or delete the file, and it may not delete the record. Concretely:

- A file that was edited in place is reported `invalid` and left exactly as it is.
- A file that is gone leaves its record, hash included, so it can be recovered.
- A directory found where the artifact should be is reported, not removed.
- A file Uguisu has no record of is reported, not removed.

### Three depths

`existence` (one `stat`), `light` (type, size, mtime), `full` (SHA-256 over the whole file in 1 MiB chunks). Startup uses `existence`; the default `verify` uses `light`; hashing is always an explicit `--full`.

### A separate state, projected onto the episode

`VerificationState` is `unchecked | verified | missing | invalid`, independent of `DownloadState`: one says how the bytes arrived, the other whether they are still right. The episode's existing `ArchiveState` is projected from it (`verified → archived`, `missing → missing`, `invalid → modified`, `unchecked → unchanged`), so no `ArchiveState` variant and no episode-state migration were needed.

### Registration is ordered, not atomic

1. The Phase-4 completion transaction (unchanged).
2. Registration, in its own transaction.
3. Verification, outside any transaction.
4. The verification transaction and its event.

## Consequences

- The Phase-4 transaction keeps its exact shape, and hashing never happens inside the single writer transaction — an important property when an artifact is several gigabytes.
- The cost is a crash window: a completed job with no record. Startup reconciliation looks for exactly that and registers the file **where it lies**, so reconciliation never moves anything.
- "Could not check" (permissions, I/O) is not a finding: it leaves the record `unchecked` rather than `missing`, because calling it missing would invite a re-download of a file that is sitting right there.
- A light pass cannot detect an edit that preserved the length. This is a documented limitation with a documented answer (`--full`), not a silent gap.
- Because nothing is ever deleted, a user who wants an inconsistency resolved has to say so. That is the intended trade: an archiver that deletes to stay tidy is not an archiver.

## Alternatives considered

- **A row per verification run (an audit log).** Useful for history, but the common query is "what is wrong now", and the event log already records every transition. Rejected as a table that grows without being read.
- **Repairing on detection** (re-download an `invalid` file automatically). Silently replaces a file a user may have deliberately edited — tagged, trimmed, re-encoded — and turns one bad byte into unbounded traffic. Rejected.
- **Deleting the record when the file is gone.** Loses the hash, which is the only thing that lets Uguisu recognize the file if it comes back. Rejected.
- **Trusting mtime alone.** Archives get copied, rsynced and restored, all of which change mtime without changing bytes. Mtime is recorded and used to decide whether a size match is meaningful, never as a failure on its own.
- **Verifying inside the completion transaction.** Simpler ordering, but a gigabyte-scale hash inside the writer transaction stalls every other command. Rejected.
