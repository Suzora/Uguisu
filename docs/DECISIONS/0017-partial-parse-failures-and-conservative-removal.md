# ADR 0017 — Partial parse failures and conservative removal detection

**Status:** accepted · **Date:** 2026-09-17 · amends [ADR 0008](0008-feed-parser.md)

## Context

Feeds fail in two ways: a single broken item (a mismatched tag in one show-notes block) and a broken document (a truncated download, a syntax error near the top, nesting bombs). A parser that rejects the whole feed for one bad item loses every update; a parser that silently skips items makes Uguisu believe episodes vanished. Removal detection ("this episode is no longer in the feed") has to know which of these happened.

## Decision

- **Isolable errors** — Uguisu limit violations (field/text caps, too many enclosures or extensions) and quick-xml *ill-formed* errors inside an item — are handled per item: the field is truncated with a warning, or the item becomes a `MalformedItem { index, reason, partial_title, partial_guid }`, the reader skips to the item's end and continues. At most `max_malformed_items` (100) are isolated.
- **Non-isolable errors** — syntax errors outside an item, `max_depth`, `max_items` — stop the parser; the items completed so far are kept and the feed is marked `truncated`. A document with no completed item and unclosed elements is `malformed_xml`.
- Both facts are typed on `ParsedFeed` (`truncated`, `malformed_items`/`is_partial()`), stored per fetch (`feed_fetches.partial`, `truncated`), reported and shown as exit code 9 (partial success) by the CLI.
- **Removal detection is conservative:** only a *complete* fetch (fetched, not truncated, no malformed item) may increment `missing_streak`; an episode is marked `removed_from_feed_at` at `removal_streak` consecutive complete fetches without it (default 2), never deleted, and the mark is cleared when it reappears. When more than `mass_removal_guard_percent` (50 %) of the present episodes are missing at once, the fetch changes no streak and reports `removal_suppressed`. 304 and fingerprint hits never touch streaks. Malformed items with a GUID are stored as placeholders so their episodes are not both "new" and "missing" across refreshes.

## Consequences

- One broken item costs one episode's update, not the feed; a truncated feed costs one refresh's removal signal, not the archive's history.
- A feed that genuinely shrinks by more than half is noticed only after the guard is raised (`UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT`) or by a human; the report says so explicitly.
- `max_items` truncation of a very large feed keeps the newest items only if the host lists newest first (the usual case); removal detection stays off for such feeds until the limit is raised.

## Alternatives considered

- **Reject the whole document on any error:** loses updates for the ~1–2 % of feeds with one broken item in the corpus.
- **Delete episodes missing from the feed:** violates the archive promise; feeds routinely drop old items.
- **Removal only when the feed says so (`podcast:…` markers):** no such marker exists in practice.
