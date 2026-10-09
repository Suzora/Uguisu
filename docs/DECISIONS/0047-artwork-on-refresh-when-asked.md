# ADR 0047 — Artwork on refresh, when asked

**Status:** accepted, amends [ADR 0026](0026-tagging-and-artwork.md) · **Date:** 2026-10-01

## Context

ADR 0026 made fetching podcast artwork an explicit command, off by default, because installing a release must not start making requests. The configuration has carried `UGUISU_ARCHIVE_ARTWORK_FETCH` since then: `CONFIGURATION.md` describes it as "fetch podcast artwork when a feed is refreshed", but nothing read it, so turning it on did nothing.

## Decision

With `UGUISU_ARCHIVE_ARTWORK_FETCH` on, a refresh that stored a changed feed fetches the podcast's artwork if either:

- the refresh changed the podcast's artwork URL, or
- no artwork is stored for the podcast yet.

In every other case — the same URL with an image stored, a 304, an unchanged body — the refresh asks for nothing.

The fetch is the one `archive artwork fetch` runs: `Profile::Artwork`, the byte cap, content-addressed storage, a conditional request, and its own `podcast.artwork.*` events. It runs after the refresh has committed, as the archive policy does (ADR 0023). A failure is logged and recorded by its event, and never turns the refresh into a failure: the feed is stored either way.

The default stays **off**, for ADR 0026's reason.

## Consequences

- Someone who wants artwork for every podcast turns on one setting instead of running a command per podcast.
- A refresh that fetches artwork holds its scheduler slot for the image request as well.
- A publisher whose cover URL never changes is asked for the image once, not on every changed feed.
- An image replaced at the same URL is not noticed by refreshes. `archive artwork fetch` (and `--force`) still asks.

## Alternatives considered

- **Fetch on every refresh with a changed body.** Rejected: each would send a request and write an event, for an image that rarely changes.
- **Remove the setting.** Rejected by the owner: the setting was documented as working, and nothing else offers artwork without a command per podcast.
