# ADR 0019 — Validated resume and atomic finalization

**Status:** accepted, amended 2026-10-04 · **Date:** 2026-09-18 · relates to [ADR 0007](0007-integrity-hashing.md), [ADR 0011](0011-http-client-policy.md)

## Context

Podcast media is large and podcast hosts are ordinary CDNs: some honour `Range`, some ignore it, some rotate ETags per edge node, some answer `416`, some send a `Content-Length` they do not keep. Resuming the wrong bytes produces a file that is the right size and the wrong content — the worst possible failure for an archive, because nothing complains until someone plays it.

At the same time a partially written file must never be mistaken for an archived episode, and a crash can happen between any two syscalls.

## Decision

**Resume is a claim that must be proven.**

- The offset is `min(file length, acknowledged bytes)` and the `.part` is truncated to it. The tail a crash left unacknowledged — at most one progress interval — is discarded, never trusted.
- The existing prefix is re-hashed before the transfer continues (1 MiB buffer, blocking task). SHA-256 has no resumable state to persist, and the read doubles as proof that the prefix is still readable.
- A resume is attempted only with `Accept-Ranges: bytes` and a **strong** `ETag` or a `Last-Modified`; weak validators never go into `If-Range`.
- A `206` must carry a `Content-Range` starting exactly at the requested offset; anything else is `range_invalid` and not retried. A `200` after a Range request means the validator changed or the host ignores ranges: truncate to zero and restart inside the same attempt. A `416` finalizes if the `.part` already holds the known total, otherwise restarts.
- `Range` and `If-Range` are sent on every redirect hop, so a CDN that redirects per request still resumes.

**Finalization is one move that never replaces a file, after a complete, hashed body.**

1. Flush, `sync_all`, drop the file handle (Windows cannot rename an open file).
2. Compare-and-set `downloading → finalizing` with the hash and the byte count: the durable "the body is complete" marker, and the point from which cancellation no longer applies.
3. Link the `.part` to the target, which fails when the name is taken in any case the file system folds, then remove the `.part` name; where links are unsupported, check that the target is absent and rename. On Unix, fsync the parent directory. *(Amended 2026-10-04: the check-then-rename alone left a window, and on a case-insensitive file system compared one spelling while `rename` replaced the other.)*
4. Commit `completed`, the closed attempt, `archive_state = archived` and `download.completed` in one transaction; publish after it.

Windows sharing violations (os error 32/5, typically a virus scanner) are retried five times with a doubling 100 ms delay before `failed(finalization)`. An existing target is never overwritten: the job ends `failed(target_exists)`.

## Consequences

- A resumed file is either byte-identical to the source or the transfer restarts; there is no third outcome.
- Resuming costs a re-hash of the prefix (≈ 1 GiB/s locally). On a real connection that is far cheaper than re-downloading; on loopback it makes a resume roughly as expensive as a fresh transfer, which the benchmark shows and which is fine.
- A file under a final path is complete by construction, so readers, verification (Phase 5) and users need no "is this finished?" check.
- Recovery after a crash while finalizing has three cases (target present, `.part` present, neither) and the third is a typed failure a user can retry, not silent data loss. A crash between the link and the unlink counts as "target present"; the `.part` name it leaves is reported as a leftover.
- Not fsyncing progress updates is a deliberate hole: the database may claim fewer bytes than the file holds, which the `min(file, database)` rule turns into a slightly longer transfer, never into corruption.

## Alternatives considered

- **Trusting the stored byte count without re-hashing:** faster, but a torn write or a truncated tail would be baked into the archive undetectably.
- **Persisting hasher state:** SHA-256 exposes none; a resumable construction (tree hashing) would change the integrity contract of ADR 0007 for a benefit measured in milliseconds.
- **Writing straight to the target path:** removes the rename, but every crash then leaves a partial file where a complete one belongs.
- **Copy instead of rename when the temporary directory is elsewhere:** avoided by keeping `.uguisu-tmp` inside the podcast directory, so the rename is always intra-filesystem.
- **Deleting a conflicting target:** never; the archive is the crown jewel and the user decides.
