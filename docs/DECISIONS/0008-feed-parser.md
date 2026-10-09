# ADR 0008 — Own feed parser on quick-xml

**Status:** accepted, amended 2026-09-17 (Phase 3), amended by [ADR 0017](0017-partial-parse-failures-and-conservative-removal.md) · **Date:** 2026-09-17

## Context

Uguisu needs RSS 2.0 + Atom + the iTunes namespace + a wide Podcasting 2.0 subset (guid, transcript, chapters, alternateEnclosure, source, integrity, person, location, season, episode, trailer, license, funding, value, txt, images/image, locked, medium, liveItem awareness), preservation of the raw source values for "source vs normalized" metadata, tolerant date/duration parsing, streaming over 50 MB feeds, and hardened XML handling (no DTD/entities).

`feed-rs` (2.x) is a mature general feed parser with good RSS/Atom/media/iTunes coverage but no Podcasting 2.0 model, no access to raw per-item extension elements beyond a generic map, and normalization choices Uguisu would have to undo.

## Decision

Implement `uguisu-feed`'s parser directly on `quick-xml` (streaming reader, entity expansion off, depth and size limits), producing `ParsedFeed { channel, items, warnings }` with:
- typed fields for RSS/Atom/iTunes/Podcasting 2.0 elements Uguisu uses,
- a `raw_extensions` map per item for elements Uguisu does not model yet,
- a `warnings` list (malformed dates, duplicate GUIDs, missing enclosures, bad durations) that flows into `feed_fetches.parse_warnings`.

A fixture corpus of intentionally messy feeds (brief §50) drives the parser tests; `feed-rs` output on the same corpus is used during Phase 3 as a differential oracle for the common fields.

## Consequences

- Full control over namespaces and error tolerance; one dependency (`quick-xml`) instead of a parser plus a wrapper.
- More code to own; mitigated by the differential tests and the fixture corpus.

## Alternatives considered

- **`feed-rs` + post-processing of extensions:** less code initially; but raw-source preservation and P2.0 modelling would still require walking the XML separately.
- **`serde`-based XML deserialization:** brittle for namespaces, unordered elements and lenient parsing.

## Amendment 2026-09-17 (Phase 3)

As built: `uguisu_feed::parse(bytes, &FeedLimits) -> ParsedFeed { kind, encoding, channel, items, malformed_items, truncated, warnings, stats }` on quick-xml 0.42 with the crate's own element stack (`check_end_names = false`) so a mismatched end tag inside an item isolates that item instead of aborting the document (ADR 0017). Namespaces are resolved by URI with declarations scoped per element, so feeds that declare the iTunes or Podcasting 2.0 namespace under unusual prefixes parse identically. RSS 2.0, Atom and RSS 1.0/RDF share one walker. Warnings are typed lists on the feed and on each item and end up in `feed_fetches.warnings` and the refresh report. The Phase 2 `probe` (identity and podcast-likeness for the resolver, 12 behavioural tests, live fixtures) stays a separate, smaller walker; folding both into one is deferred because the resolver's tests and recorded fixtures depend on the probe's exact tolerances. The differential test against `feed-rs` 2 runs on the corpus and on the recorded live feeds for item count, titles, GUIDs, publication instants and first enclosure URL, with the documented tolerances (feed-rs synthesizes ids for items without a GUID and drops dates it cannot parse).
