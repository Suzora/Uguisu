# ADR 0052 — Moving a feed by hand

**Status:** accepted · **Date:** 2026-10-03

## Context

A podcast moves to a new feed URL on its own only when the same-show check passes ([`FEED_ENGINE.md`](../FEED_ENGINE.md) §8): an equal `podcast:guid`, or the same title and half of the stored episodes present. When an `itunes:new-feed-url` fails it — the new host renamed the show, rewrote every GUID, dropped `podcast:guid`, or the announced URL does not load — Uguisu warns, emits `feed.url.change_detected` and checks again on the next refresh, for as long as the feed announces it. Nothing stores the announcement: it exists in the event log and in the warnings of a fetch-log row, and both are pruned. Nothing lets a person act on it. A feed that moves without saying so (the old host answers 404) has no path at all but adding the new URL as a second podcast, which starts its history from nothing.

A redirect never leaves an unverified announcement: one that lands on a different show fails the refresh before anything is written.

## Decision

### An unverified announcement is a source

It is stored as a `podcast_sources` row that is neither current nor replaced: `is_current = 0` and no `replaced_by_source_id`. `discovered_at` is when the feed first announced it. Its fetch columns hold the last check: `failed`, with `last_error_kind` and `last_error_detail` saying why it is not verified — a fetch error's kind, or `invalid_podcast_feed` and the same-show check's reason — and `consecutive_failures` counting the refreshes that checked it.

A podcast has at most one: the URL its feed announces now.

| A refresh that | does to the announced source |
|---|---|
| fetched the feed, which announces a URL that fails the check | records it: updates the row for that URL, or replaces a row for another URL |
| fetched the feed, and the move verified | removes it; the current source is replaced as before |
| fetched the feed, which announces nothing | removes it |
| found the feed not modified, or failed | leaves it |

`podcast show` and every podcast body of the API carry it as `announced`; the web UI shows it on the podcast page and marks the podcast in the library. A podcast's source history is where its feed came from and leaves it out. The URLs that map an OPML outline or a Podgrab feed to a podcast include it: an OPML exported by an app that followed the announcement then names the podcast that announced it, rather than adding the show a second time.

### A person moves a podcast to a feed

`uguisu podcast move-feed <podcast-id> <url> [--dry-run] [--force]`, `POST /api/v1/podcasts/{id}/move-feed` `{url, dry_run, force}`, and "Move to this feed" beside the announcement on the podcast page. The URL may be the announced one or any other.

1. The URL is fetched and parsed as an announced feed is, under the same network policy and limits, and the same-show check runs. A URL that does not serve a podcast feed is an error, and nothing moves.
2. A conflict, and nothing moves: the URL is another podcast's current feed, or the feed carries a `podcast:guid` another podcast has. Joining two podcasts is not a move.
3. A verified feed is moved to. An unverified one is moved to only with `force`; without it the result says why and `moved` is false, and the CLI exits 2, as for every confirmation it asks for. `dry_run` never moves.
4. The move is one transaction: the announced source is removed, the current one is replaced with `replacement_reason = manual`, and `feed.url.changed` fires with `via: manual`. The new source has no validators, so the refresh that follows parses the new feed in full. Episodes keep their ids; that refresh's identity rules ([ADR 0014](0014-identity-fallback-and-guid-changes.md)) decide which items are stored episodes, and items whose GUIDs were rewritten become candidate duplicates ([ADR 0051](0051-candidate-duplicates-and-orphans.md)).
5. The URL the podcast already uses is nothing to do: `moved` is false, and nothing is fetched.

## Consequences

- `podcast_sources` rows are current, replaced or announced. A new reader of the table says which it wants.
- A forced move to a different show is possible and recorded: the old source stays in the history, the event says `manual`, and nothing is deleted. Moving back is another move.
- An announcement nobody acts on stays visible for as long as the feed makes it.
- The reason a check failed now reads `<kind>: <detail>` in the warning and in `feed.url.change_detected`, for a failed check as for a failed fetch.

## Alternatives considered

- **Following an unverified announcement automatically.** Rejected: the check exists because a feed can announce a different show, from a reused or hijacked host. Only a person who has read the reason can overrule it.
- **Four columns on the current source** (`announced_url`, `_via`, `_reason`, `_at`). Rejected: a migration for a second copy of the fetch bookkeeping that a source row already keeps for its own URL.
- **Dismissing an announcement.** Rejected: the feed does make it; hiding that changes nothing the feed says, and the row goes away when the feed stops.
- **Joining two podcasts** whose feeds turn out to be one show. Not a move; out of scope.
