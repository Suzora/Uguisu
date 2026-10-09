# ADR 0020 — Identifier-based media paths for Phase 4

**Status:** accepted (interim), layout superseded by [ADR 0022](0022-template-grammar-and-path-safety.md) · **Date:** 2026-09-18 · relates to [ADR 0009](0009-path-templates.md)

## Context

The download engine needs a destination before the archive engine exists. ADR 0009 specifies the real layout: a deterministic template language with per-platform sanitization profiles, producing human-readable paths like `Darknet Diaries/2024/2024-01-15 - 141 - The Pizza Problem.mp3`. That language, its sanitizer and its collision rules are Phase 5 work, and building half of it now would mean building it twice.

The alternative — waiting — would mean no downloads in Phase 4.

## Decision

Phase 4 writes to identifier-derived paths:

```text
<media dir>/<podcast-id>/<episode-id>.<ext>
<media dir>/<podcast-id>/.uguisu-tmp/<job-id>.part
```

- `<media dir>` is `UGUISU_MEDIA_DIR`, defaulting to `<data dir>/media` (`/media/podcasts` in the Docker image, ADR 0061). It is created lazily, so read-only commands never touch it.
- Podcast, episode and job identifiers are ULIDs: 26 characters of `[0-9A-HJKMNP-TV-Z]`, which are safe on every filesystem Uguisu targets.
- `<ext>` comes from a whitelist of media MIME types, else a lowercase alphanumeric extension of at most five characters taken from the URL, else `bin`.
- The `.part` lives in `.uguisu-tmp` **inside the podcast directory**, so finalization is always an intra-filesystem rename.
- `part_path` and `target_path` are stored on the job, relative to the media directory, with POSIX separators.

## Consequences

- Nothing from a feed reaches a path component, so path traversal, reserved Windows names, bidi characters, Unicode collisions and length limits are impossible in this phase rather than defended against. The sanitizer of ADR 0009 still has to do all of that when templates arrive.
- Two episodes cannot collide, so no collision suffix policy is needed yet.
- The archive is not human-readable yet: users see identifier names in the media directory. This is documented as a Phase 4 limitation, and the media directory is not advertised as browsable. Since [ADR 0022](0022-template-grammar-and-path-safety.md) a download lands on the template's readable path; identifier names remain only as its fallback and for files nobody relocated.
- Moving a file to its template path is mechanical — the stored relative path is the source, the rename is atomic, and the episode row keeps the link — and happens only on an explicit `archive relocate` ([ADR 0022](0022-template-grammar-and-path-safety.md)); nothing migrates on its own.
- Storing paths relative to the media directory means the archive can be moved or mounted elsewhere without a database rewrite.

## Alternatives considered

- **Implement ADR 0009 now:** the sanitization profiles, collision handling and golden tests are a phase of their own; doing it under download pressure would produce a layout nobody wants to migrate away from later.
- **A flat `<media dir>/<episode-id>.<ext>`:** one directory with hundreds of thousands of entries is slow on several filesystems, and the podcast directory is the natural unit for the temporary directory and for later per-podcast operations.
- **A content-addressed store (`<hash>.mp3`) with symlinks:** deduplicates shared enclosures, but the hash is known only after the download, symlinks are awkward on Windows, and it collides with the readable layout that is the product's promise.
- **Keeping the temporary file in a global temporary directory:** would make finalization a cross-device copy on many setups, losing atomicity.
