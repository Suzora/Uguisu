# ADR 0026 — Writing tags, and fetching artwork

**Status:** accepted, amended by [0047](0047-artwork-on-refresh-when-asked.md) (artwork on refresh, when asked) · **Date:** 2026-09-19 · amends [ADR 0012](0012-metadata-tagging.md) · relates to [ADR 0011](0011-http-client-policy.md), [ADR 0021](0021-archive-file-and-verification.md)

## Context

ADR 0012 chartered metadata tagging via `lofty` with a per-format capability table, five policies and a pre-write snapshot. Phase 6 implements it, and implementing it changed three things: what the policies are, which fields can be written, and what has to happen so that a crash mid-write cannot leave a file Uguisu will later call corrupt.

Artwork is here too because it is the one thing tagging needs that has to be fetched from a stranger.

## Decision

### Two modes, not five

| Mode | Rule |
|---|---|
| `fill_missing` *(default)* | write a managed field only where the file has none |
| `sync` | make the managed fields match Uguisu; a field Uguisu has no value for is left alone |

`overwrite` and `custom` stay unbuilt rather than half-built. `existing_wins` is `fill_missing`, and `rss_wins` is `sync`; the CLI accepts the shorter names.

Neither mode removes anything, and neither touches a tag Uguisu does not manage — **by construction rather than by policy**. A write starts from a clone of the tag already in the file and changes only the managed keys, so a publisher's lyrics, ReplayGain values and cover art survive because nothing ever looked at them. ADR 0012's "no policy removes a pre-existing tag" is therefore a property of the mechanism, not a rule someone has to remember.

### Uguisu never modifies the only copy of a file

Copy the artifact into `.uguisu/tmp/`, tag the copy, re-parse it, hash it, then rename it over the original. Until that rename the artifact is untouched; after it the record moves with the bytes in the same transaction, and the sidecar and manifest follow. `lofty`'s `save_to_path` would open the real artifact, so `clippy.toml` forbids it and `disallowed_methods` is denied workspace-wide — the guarantee is enforced rather than remembered.

A file that does not already match its record is refused outright. Tagging it would replace a detectable problem with an undetectable one.

Two copies exist for the length of the operation, so a space check refuses up front rather than filling the disk halfway through.

### A marker is committed before a byte moves

Between the atomic replace and the record update, "Uguisu retagged this" and "somebody else changed it" are indistinguishable from the bytes alone. So `tag_state = pending` goes down first, in its own committed transaction. A crash cannot forge it, and recovery is bounded by a partial index that is empty whenever nothing is in flight:

- bytes still match the record → the write never landed; clear the marker.
- bytes differ → this is Uguisu's own work; adopt them with reason `retag_recovered`.

That second case is the **only** place in Uguisu where a hash is re-read to fit a file rather than the other way round. It is deliberately narrow — only a row that said `pending` before the first byte moved — and it is recorded as a resumed write, never as a verification.

### `hash_value` follows the bytes; `source_*` does not

`archive_files.hash_value` keeps meaning "the bytes on disk now", so every Phase-5 verification path is untouched and `verify.rs` needed no change at all — which is the test of the design. New `source_hash_value`, `source_hash_algo` and `source_size_bytes` record what was received. Without that split, a file Uguisu itself retagged would be indistinguishable from one someone altered behind its back, and `verify --full` would report a false `invalid` on Uguisu's own work.

The provenance is immutable for the life of **one download**: a re-download of the same episode replaces it, because it then describes different bytes. `upsert` therefore resets `tag_state` and `sidecar_written_at` too — fresh bytes carry no tag write and no sidecar that describes them.

### A field belongs in the table only if it round-trips

`sync` decides whether to write by comparing the value it wants against the value the file reports. A field that comes back different is rewritten on **every run**, and every rewrite moves the archive's hash for no change at all. So the managed set is exactly the fields that survive a write-then-read unchanged, and a test writes every one of them to an MP3 and a FLAC, reads it back, and writes again asserting the second write does nothing.

Four left the set as a result:

- **ID3v2's `PCST` podcast flag.** `lofty` models it as a flag rather than as text; writing it makes the whole frame set fail to encode, taking every other field with it. A cosmetic flag is not worth a field that breaks MP3 tagging.
- **The show name** — written, not readable back.
- **The year** — ID3v2 stores it in the same frame as the recording date, so the two overwrite each other. `RecordingDate` carries it, as a date rather than a full timestamp, because a trailing `Z` does not survive either.
- **The feed and enclosure URLs** — written into URL frames `lofty`'s own reader does not return as text.

The podcast description and episode GUID are unreadable specifically in Vorbis comments, so the capability table declares them unsupported there. All of it stays in the sidecar, which has no such limits, and is reported as `not_embeddable` rather than silently dropped.

### WAV and AIFF are refused by policy, not by capability

`lofty` will write `RiffInfo` and `AiffText`. Those chunks hold none of the podcast fields — no season, no episode GUID, no cover — and no podcast client reads them. Writing a handful of the thirteen managed fields and calling it done would be worse than saying so. An unsupported container is a per-file **result**, never a failed batch.

### Chapters are deferred, with a reason

ADR 0012 lists chapters (`podcast:chapters` → ID3 `CHAP`/`CTOC`) and per-episode artwork. Phase 6 implements neither, and the reason is worth stating precisely rather than waving at difficulty.

`lofty` 0.25 does expose `ChapterFrame` and `TableOfContentsFrame` — but only through the **ID3v2-specific** API, not through the generic `Tag` that this crate writes through, and its MP4 module has no chapter support at all. Implementing chapters therefore means a second, format-specific writing path that works for MP3 and silently does nothing for M4A: exactly the kind of half-feature the capability table exists to prevent.

The other half is upstream of the writer. `podcast:chapters` is a **reference to a JSON document at a URL**; `uguisu-feed` parses and stores the reference, and nothing fetches it. Writing chapters into files would mean a new untrusted network fetch, a parser for the Podcasting 2.0 chapters schema, and a place to keep the result — three decisions no Phase-6 requirement asks for. The reference stays in the feed data where it already is.

### Artwork goes through the ordinary HTTP stack

A new `Profile::Artwork` in `uguisu-http`: the SSRF policy, per-hop re-validation on redirects, and a 16 MiB cap applied before a byte is read. There is no second URL validator, because a second one is a second thing to get wrong.

The **bytes** decide the format, then the declared media type, and never the extension. Only JPEG, PNG and WebP are stored. SVG is refused even when honestly declared: it is a document format that can carry script and external references, and embedding one into an audio file would hand a player a program rather than a picture. `application/octet-stream` counts as silence rather than as a claim, because a great many CDNs send it for ordinary images; a type that names something else entirely is a contradiction and is refused.

Storage is content-addressed, so a replacement can never destroy the image before it, and a partial unique index — not a check that could race two fetches — keeps exactly one current row per podcast. Fetching is explicit and off by default: installing a release must not start making requests.

`ARCHITECTURE.md` pre-approved the `image` crate for this crate. It is **not** taken up: Phase 6 stores and embeds artwork as it arrived and never decodes, resizes or re-encodes, so a decoder would be attack surface bought for nothing (`SECURITY.md` §3.10). Magic-byte recognition is thirty lines beside the pattern `uguisu-download`'s sniffer already uses.

## Consequences

- A player shows a readable title, show and date for every archived episode, and the file a user copies out of the archive carries them.
- A tag write can be interrupted at any point without leaving a file Uguisu will later call corrupt.
- Four podcast fields exist only in the sidecar. A tool reading Uguisu's files rather than its tags gets everything; a music player gets what its format can hold.
- ADR 0012's five-policy vocabulary is reduced to two, and its chapter support deferred.
