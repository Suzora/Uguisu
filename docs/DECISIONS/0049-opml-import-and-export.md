# ADR 0049 — OPML import and export

**Status:** accepted · **Date:** 2026-10-02

## Context

Every podcast app can export its subscriptions as OPML, and `PRODUCT.md` promises that Uguisu imports them (v1 #8) and exports its own. No document decided what an import does with folders, how the file reaches a server, whether it is checked before anything is added, what a feed that has moved or redirects counts as, or what an export contains. An import adds tens or hundreds of podcasts at once, so each of those choices is multiplied.

## Decision

**The document travels as text.** The CLI and the web UI read the file as UTF-8 and send it as a JSON string, `{opml, apply?, policy?}`, to `POST /api/v1/podcasts/opml`. The parser takes text and ignores a declared encoding, which would otherwise decode the text a second time; a file in another encoding loses accented title characters, never a URL. The document is capped at 1 MiB (axum's 2 MB request limit leaves room for JSON escaping), 64 levels and 10 000 feeds, and it is parsed as feeds are: no DTD, no custom entities.

**Outlines are read flat.** Every `<outline>` with an `xmlUrl` counts, at any depth; folders and categories are ignored, because Uguisu has nothing to put them in.

**A dry run is the default and sends no request.** Each outline is planned from the document and the library alone:

| Action | When |
|---|---|
| `invalid` | Not an `http`/`https` URL with a host, or longer than 4096 bytes; `feed://` and `itpc://` are reported, not rewritten |
| `duplicate` | The same feed appeared earlier in the file |
| `already_present` | A podcast has this feed as its current or a former source |
| `add` | Anything else |

"The same feed" means equal `feed_key`s (`uguisu_discovery::dedup`): scheme, `www.`, default port, trailing slash and tracking parameters do not make a different feed.

**Applying verifies every feed** the way `podcast add` does: resolution through the SSRF policy, at most `UGUISU_FEED_REFRESH_CONCURRENCY` at a time. Only a feed the outline's URL served itself is added — decided from the resolution's provenance, so a redirect or the HTTPS upgrade is not a reason to stop. An outline that led to a website or a directory page is `needs_review`, with the feed it found in the detail, to be added with `podcast add` by someone who looked at it. After resolution the feed's `feed_key` is checked again, so two spellings of one feed add one podcast. A second podcast with the same `podcast:guid` is a `conflict`; any other error is `failed` with its message, and stops nothing else.

**A policy given with the import is stored with each new podcast, in the same transaction.** Otherwise the scheduler could run the first refresh under the global policy, and the given one would never see the backlog. A podcast already in the library keeps its policy.

**An import does not refresh.** The scheduler fetches never-fetched podcasts first; without `serve` or the desktop app running, they stay empty until `podcast refresh --all`. Each new source records `provider = "opml"`.

**Each podcast is its own transaction.** An import cut short by a timeout keeps what it added, and running it again continues: those feeds are now `already_present`.

**The export lists every podcast**, whatever its status, with its current feed URL and website, flat and sorted by title. It carries no `dateCreated`, so the same library exports the same bytes. A private feed's URL is exported as stored, token included.

**Exit codes stay as they are.** A dry run and an applied import exit 0, with failures in the report and its counts, as `archive import` does; 9 stays the refresh's. A document that is not OPML exits 2, a file that cannot be read 1.

## Consequences

- Moving from another app is: export there, a dry run here, then `--apply` (or "Add N podcasts" in the web UI).
- The plan is free; applying costs one resolution per new feed, so an import of a few hundred feeds takes minutes. The CLI waits up to an hour over `--server`; a reverse proxy's timeout can cut it short, and re-running continues.
- Nothing is downloaded by an import unless the policy given with it, or the global one, says so.
- A file exported by Uguisu and imported into the same library adds nothing; imported into an empty one, it recreates the same feeds.

## Alternatives considered

- **A multipart upload or a raw XML body.** Rejected: neither the API's extractors nor the web client or its test stub take one, and the only gain is honouring declared encodings that exporters do not use.
- **A path on the server**, as `archive import` takes. Rejected: a file someone has on their laptop would first have to be copied to the server.
- **An asynchronous job with progress events.** Rejected for now: the per-podcast `podcast.added` events already show progress, and a job table is a lot of machinery for something run once.
- **Refreshing each podcast as it is added**, as `podcast add` does. Rejected: it doubles the time of a request that is already long, and the scheduler does the same work spread out.
- **Comparing the resolved URL with the outline's**, as `podcast add`'s confirmation does. Rejected: feeds redirect and move to HTTPS all the time, and every one of those would need review.
