# ADR 0012 — Metadata tagging via lofty with format capabilities and policies

**Status:** accepted, amended 2026-10-03 (Phase 10), amended by [ADR 0026](0026-tagging-and-artwork.md) · **Date:** 2026-09-17

## Context

Uguisu must write clean tags to MP3 (ID3v2), M4A/MP4 (iTunes atoms), Ogg Vorbis, Opus and FLAC files, embed artwork and chapters where possible, never destroy existing tags unless configured, and keep source vs normalized vs embedded metadata distinct.

## Decision

- `uguisu-metadata` uses `lofty` for reading and writing all formats; no ffmpeg/external tools in v1.
- A **capability table** per container/tag format declares which normalized fields (title, artist, album, album artist, genre, date, track, disc, comment, description, podcast flags, season, episode, artwork, chapters, URL) can be written and how (ID3v2.4 frames incl. `CHAP`/`CTOC` for chapters; MP4 `©nam`, `©ART`, `pcst`, `tvsn`, `tves`, `chpl`; Vorbis/Opus comments with `METADATA_BLOCK_PICTURE`). Unsupported fields are recorded as "not embeddable" in the sidecar instead of silently dropped.
- **Policies** (global): `rss_wins`, `existing_wins`, `fill_missing`, `overwrite`, `custom` (field-level mapping); ADR 0026 built `fill_missing` and `sync`. The mode is `UGUISU_ARCHIVE_TAG_MODE` or a write's `--mode`. There is **no per-podcast tag mode** (decided in Phase 10): tagging is never automatic, so a per-podcast mode would only change the default of a write somebody asks for, and that request already names its mode.
- The original tag set is captured into the sidecar before the first write so any write is reversible. Built in Phase 10 as a record, not a restore: the managed fields and a description of the cover (`ARCHIVE_ENGINE.md` §20); writing them back is left to a person.
- Tag writes happen after the atomic rename, on the final file, with a `.bak` only when the policy is `overwrite` and the file already had tags.

## Consequences

- Memory-safe parsing of untrusted media; consistent field mapping across formats.
- `lofty` limitations (e.g., chapter frames edge cases) become Uguisu limitations; tracked in the risk register (R5). Fuzzing the tag reader is not built yet.

## Alternatives considered

- **Shelling out to ffmpeg/`id3v2`/`AtomicParsley`:** heavier images, inconsistent behavior, harder to sandbox.
- **Format-specific crates (`id3`, `mp4ameta`, `metaflac`):** more crates to align; `lofty` already unifies them.
