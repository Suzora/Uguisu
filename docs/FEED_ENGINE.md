# Feed Engine

How Uguisu turns a feed URL into stored podcasts and episodes, keeps them in sync, and tells the rest of the system what changed — **without downloading anything**. Downloads belong to [`DOWNLOAD_ENGINE.md`](DOWNLOAD_ENGINE.md): the feed engine emits `episode.discovered`, and after the refresh commits the archive policy, off by default, decides whether to queue it (ADR 0010, [ADR 0023](DECISIONS/0023-archive-policy.md)).

Code: `crates/uguisu-feed` (parse, normalize, identity), `crates/uguisu-engine` (`refresh.rs`, `sync.rs`, `migration.rs`, `coordinator.rs`, `library.rs`, `inspect.rs`), `crates/uguisu-storage` (migration `0001_phase3.sql`, repositories). Decisions: ADR 0006, 0008, 0010, 0014, 0015, 0016, 0017.

## 1. Pipeline

```text
refresh_podcast(id, {force})
  coordinator: join an in-flight run for this podcast, or spawn one          (ADR 0016)
  load podcast + current source; mark source `fetching`; event refresh.started
  ┌─ under FeedConfig.refresh_timeout, with a CancellationToken ───────────────────────────┐
  │ GET feed_url via uguisu-http Profile::Feed (SSRF policy, redirects re-validated,      │
  │     body cap = FeedLimits.max_bytes, retries with backoff, If-None-Match /            │
  │     If-Modified-Since unless --force)                                                  │
  │   transport error ──────────────────────────────► FetchErrorKind (table §2)            │
  │   304 ──────────────────────────────────────────► not_modified (http304)               │
  │   4xx/5xx ──────────────────────────────────────► failed (status kind)                 │
  │   body not XML / empty ─────────────────────────► invalid_content_type / invalid_podcast_feed │
  │   sha256(body) == stored fingerprint && !force ─► not_modified (fingerprint), no parse │
  │   parse (limits) ───────────────────────────────► malformed_xml / unsupported_feed / too_deep / too_large │
  │   no items, or no item with an enclosure ───────► invalid_podcast_feed                 │
  │   itunes:new-feed-url differs ──────────────────► verification fetch of the announced feed (§8) │
  └────────────────────────────────────────────────────────────────────────────────────────┘
  BEGIN IMMEDIATE
    load identity index of stored episodes (one query)
    plan (pure, sync.rs): normalize channel + items → identities → classify each item
         unchanged | updated | added | candidate duplicate | malformed placeholder      (§5, ADR 0014)
         removal detection on complete fetches only                                    (§6, ADR 0017)
    redirect check: a redirected body must be this podcast, else failed                (§8)
    write podcast row (metadata hash), upserts, mark_seen / mark_missing / mark_removed,
          episode_changes rows, source validators + fingerprint, feed_fetches (+trim),
          feed URL migration (source switch), events rows
  COMMIT → publish events on the bus → RefreshReport
failure at any point before COMMIT: only source state, back-off, fetch log and
  podcast.feed.refresh.failed are written; podcast metadata and episode rows are untouched
```

`add_podcast(input)` = resolve (Phase 2) → insert podcast + current source (`never_fetched`) → `podcast.added` → first refresh. Adding is idempotent on the feed or canonical URL; a second feed with a `podcast:guid` already in the library is a `conflict` (that is a migration, §8, not a new show).

`import_opml(text)` ([ADR 0049](DECISIONS/0049-opml-import-and-export.md)) plans every feed outline of an OPML document against every source the library has had, without a request, and with `apply` runs the same resolution for each new feed, at most `UGUISU_FEED_REFRESH_CONCURRENCY` at a time. It inserts the podcast, its source (`provider = "opml"`) and the optional policy in one transaction, emits `podcast.added`, and stops there: there is no first refresh, because the scheduler fetches `never_fetched` podcasts first. An outline whose URL led to a page rather than a feed is reported, not added. `export_opml` writes every podcast's current feed URL back out.

## 2. Error classification

Every failed fetch or parse gets one stable kind (`FetchErrorKind`, `uguisu-core`), stored on the source (`last_error_kind`, `last_error_detail`), in the fetch log, in the failed event and in the report. The CLI maps kinds to exit codes 6/7/8 (`docs/CLI.md`).

| Kind | Raised by | Transient | Exit |
|---|---|---|---|
| `network_error` | connect/transport failure, too many redirects, bad `Location`, body read error | yes | 8 |
| `timeout` | request timeout, or the whole refresh exceeding `refresh_timeout` | yes | 8 |
| `dns_error` | host does not resolve | yes | 8 |
| `tls_error` | TLS/certificate failure (detected from the transport error text) | no | 8 |
| `http_client_error` | any 4xx not listed below | no | 8 |
| `http_server_error` | 5xx (after the client's retries) | yes | 8 |
| `rate_limited` | 429 (after honouring `Retry-After` within the retry cap) | yes | 8 |
| `unauthorized` / `forbidden` / `not_found` | 401 / 403 / 404 and 410 | no | 8 |
| `malformed_xml` | quick-xml syntax error outside an item, or no item survived | no | 7 |
| `unsupported_feed` | unknown root element or unsupported encoding | no | 7 |
| `too_large` | body over `max_bytes` (the HTTP client stops reading at the cap) | no | 7 |
| `too_deep` | nesting over `max_depth` | no | 7 |
| `invalid_content_type` | body is not XML (HTML error page, JSON, binary) | no | 7 |
| `invalid_podcast_feed` | empty body, no items, no item with an enclosure, or a redirect to another show | no | 7 |
| `blocked_by_policy` | SSRF policy refused the URL or a redirect hop | no | 6 |
| `cancelled` | the caller's token fired | yes | 1 |

`consecutive_failures` counts failures since the last success; at `FeedConfig.error_after_failures` (5) the podcast's status becomes `error` (the scheduler still retries it, at a slower cadence), and the next success flips it back to `active`. `next_refresh_at` backs off exponentially (interval × 2^(failures−1), the factor capped at 64 and the delay at 24 h).

## 3. Parsing (`uguisu-feed::parse`)

- **Formats:** RSS 2.0 (`<rss>`), Atom (`<feed>`), RSS 1.0/RDF (`<rdf:RDF>`). Namespaces are resolved by URI, not prefix: iTunes, Podcasting 2.0, Atom, `content:encoded`, Media RSS, Dublin Core, RSS 1.0 `enc:`. Unknown namespaced elements on items are kept in `raw_extensions` (capped).
- **Channel:** title, subtitle, description/summary, link, language, author, publisher, owner, copyright, image, nested `itunes:category`, explicit, type, `itunes:new-feed-url`, `podcast:guid`, `podcast:locked`, `podcast:medium`, `podcast:funding`, `atom:link rel=self`, updated, generator.
- **Item:** guid (+`isPermaLink`), title (+`itunes:title`), subtitle, description, `content:encoded`, summary, link, `pubDate`/`dc:date`/Atom dates, duration, season, episode, type, explicit, image, author, enclosures, `podcast:chapters`, `podcast:transcript`, `podcast:person`, `podcast:location`, `podcast:soundbite`, `podcast:value`, `podcast:license`, `podcast:txt`.
- **Enclosures**, in priority order: `<enclosure>`, Atom `link rel="enclosure"`, `enc:enclosure`, `media:content` (fallback when nothing else declares media), `podcast:alternateEnclosure` (with `podcast:source` and `podcast:integrity`). The first primary-capable one becomes the primary; nothing is disqualified for an odd MIME type or a bad length — that is recorded as a warning and the enclosure kept.
- **Encodings:** UTF-8, UTF-16 (BOM), Latin-1 and Windows-1252 declared feeds are decoded with lossy replacement and a warning; entity expansion is limited to the predefined XML entities and numeric references (no DTD, no external entities, `docs/SECURITY.md` §3.3).
- **Limits** (`FeedLimits`): `max_bytes` 50 MiB, `max_depth` 64, `max_items` 50 000, `max_text_bytes` 64 KiB per text node, `max_field_bytes` 4 KiB per scalar field, `max_enclosures_per_item` 16, `max_raw_extensions_per_item` 64, `max_malformed_items` 100. Exceeding a per-field limit truncates the value and warns.
- **Partial failures (ADR 0017):** a mismatched end tag inside an item isolates that item as a `MalformedItem { index, reason, partial_title, partial_guid }` and parsing continues; a syntax error outside an item, `max_depth` or `max_items` stop the parser with the items completed so far and `truncated = true`. Both facts are typed (`ParsedFeed.truncated`, `is_partial()`), reach the report and switch removal detection off for that fetch.

`uguisu feed inspect <url>` runs exactly this parse plus normalization and identity on a URL without opening the database.

## 4. Normalization (`uguisu-feed::normalize`)

- **Dates:** RFC 2822 (with and without seconds, with named zones), RFC 3339/ISO 8601, date-only, `dc:date`. Quality is stored with the value: `exact`, `assumed_utc` (no zone), `future` (more than a day ahead), `ancient` (before 1990), `invalid` (kept raw, value `NULL`). Invalid dates never disqualify an item; the episode sorts by `first_seen_at` instead.
- **Durations:** `H:MM:SS`, `MM:SS`, plain seconds, fractional seconds, `1h 2m 3s`; implausible values (over 30 days) and unparseable strings keep the raw text and yield `NULL` plus a warning.
- **Text:** titles and fields are NFC-normalized, whitespace-collapsed and capped; HTML descriptions are kept verbatim in `description_html` and converted to plain text (`description_text`, tags stripped, entities decoded, `<script>`/`<style>` dropped, 20 000 characters).
- **Missing title:** the enclosure's file name or `Untitled episode`, with a warning; the fingerprint then uses the enclosure only. A title of markup only (`<![CDATA[<br>]]>`) renders to no text and takes the item's `itunes:title` when it has one, otherwise the same fallback; its fingerprint keeps the empty text it was always computed from, so an identity stored before the fallback does not move.
- **Podcast metadata** is normalized the same way (sort title with leading articles stripped, categories flattened, `podcast:guid` trimmed).

## 5. Identity and change detection

**Identity cascade (ADR 0006, per feed):** `guid:<normalized guid>` when the GUID is non-empty and unique within the feed → `url:<normalized enclosure url>` when unique (scheme and host case-insensitive, default ports and fragments dropped, `utm_*`/tracking parameters removed, known prefix trackers unwrapped: `chtbl.com/track/<id>/…`, `pdst.fm/e/…`, `dts.podtrac.com/redirect.mp3/…`, `mgln.ai/…`, `op3.dev/e/…`, and others) → `fp:<sha256(normalized title | publish day | enclosure length)>`. The chosen source and every signal are stored (`identity_source`, `guid_key`, `enclosure_key`, `fingerprint_key`, `identity_reason`) so the choice is explainable. Two byte-identical items in one feed collapse onto one episode with a warning.

**Comparable hash:** `content_hash` = sha256 over title, subtitle, description HTML, link, publication instant (UTC seconds), duration, season, episode number, type, explicit, artwork URL, author, every enclosure (URL, MIME, length, primary flag) and the extras document. An item whose hash equals the stored one is **unchanged** and only marked seen; otherwise it is **updated**, the row is upserted and every differing field gets an `episode_changes` row with old and new value (1 KiB each, ADR 0015) plus `episode.updated { fields }`.

**GUID changes (ADR 0014):** an item whose identity key matches nothing stored is matched by its secondary signals (GUID, enclosure URL, fingerprint — in that order). If the stored and incoming identities do not contradict each other (one side has no GUID, e.g. a feed that gained or lost GUIDs, or the same enclosure under a new host) the stored episode is kept, its signal columns are refreshed, `identity_signals` is logged and the report warns. If both carry different GUIDs, or both identities are enclosure-based on different URLs and only the fingerprint agrees, the item becomes a **new episode marked as a candidate duplicate**: `duplicate_of_episode_id`, `duplicate_reasons` (`same_enclosure_url`, `same_fingerprint`, `same_published_minute`, `same_title`, `same_media_length`), `archive_state = skipped`, `skip_reason = duplicate of <id>`, event `episode.identity_ambiguous`. Nothing is merged or deleted automatically: the policy skips candidates, and a person resolves each as `same` (merged into the original) or `separate` with `episode duplicates|resolve`, `POST /api/v1/episodes/{id}/resolve` or the episode list (ADR 0051). A later refresh never changes the link or the reasons.

**Malformed items** are counted and, when they carry a GUID, stored as placeholders (`malformed = 1`, `archive_state = skipped`) so the episode is not "new" forever once the host fixes it.

## 6. Removal detection (ADR 0017)

Feeds drop old items, hosts misbehave, parsers stop early. Uguisu therefore never deletes and only *detects*:

- Only a **complete** fetch (outcome `fetched`, not truncated, no malformed item) may touch `missing_streak`. 304 and fingerprint hits change nothing.
- A stored episode absent from a complete fetch gets `missing_streak + 1`; at `FeedConfig.removal_streak` (2) it is marked `removed_from_feed_at` and `episode.removal_detected { missing_streak }` fires. Seen again, the streak resets and the mark is cleared; the episode id and its archive state never change.
- **Mass-change guard:** when more than `mass_removal_guard_percent` (50 %) of the present episodes are missing at once, streaks are left alone, the report carries `removal_suppressed` and the warning `removal_suppressed_mass_change`. A feed that shrinks to its last 100 items after an archive of 1 000 therefore never flags 900 removals by accident. If that is what the feed did, raise `UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT` above the share that went missing (100 never suppresses); refreshing again does not help, because every later fetch is suppressed the same way.
- Episodes already marked removed are not counted again.

## 7. Podcast metadata

Channel fields are hashed (`metadata_hash`); when the hash changes the podcast row is updated, the changed fields are listed in the report and `podcast.metadata.updated { fields }` fires. A channel that lost its `podcast:guid` keeps the stored one (the GUID is identity, not a field). `feed_kind` is recorded from the parse (`rss2` / `atom` / `rss1`).

## 8. Feed URL migration

Triggers: a permanent redirect chain (every hop 301/308) ending at another URL, or an `itunes:new-feed-url` that differs from the current feed URL, its canonical URL and the final URL.

Verification (same show?): equal `podcast:guid` when both sides have one — a different GUID is a hard no; otherwise the same normalized title **and** at least half of the comparable episodes present (identity keys, measured on the smaller of the two lists so a host that lists fewer items still verifies). An announced `new-feed-url` is fetched and parsed for that check (no items are synced from it); a redirect target is the body just fetched.

Verified → in the same transaction: a new `podcast_sources` row becomes current (for redirects with the validators and fingerprint of this fetch; for `new-feed-url` without any, so the next refresh parses the new feed in full), the old row keeps `replaced_by_source_id` and `replacement_reason` (`redirect` / `new-feed-url`), `feed.url.changed` fires, the report says `changed`. Episodes keep their ids. Not verified → `feed.url.change_detected { announced, via, reason }`, a warning, the URL stored as the podcast's **announced source** with why (`<kind>: <detail>`), and the next refresh tries again. Any redirect — permanent or temporary — that lands on a different show fails the refresh with `invalid_podcast_feed` before a single row is written, so only `new-feed-url` leaves an announced source.

The announced source is a `podcast_sources` row neither current nor replaced, one per podcast at most, kept in step with the feed: a fetched feed that announces another URL replaces it, one that announces nothing or a verified move removes it, a not-modified or failed refresh leaves it ([ADR 0052](DECISIONS/0052-moving-a-feed-by-hand.md)). A person acts on it — or on a feed that moved without saying so — with `podcast move-feed`, `POST /api/v1/podcasts/{id}/move-feed` or the podcast page: the URL is fetched and checked the same way, moved to when the check passes and, when it fails, only with `force`. The old source is kept with `replacement_reason = manual`, `feed.url.changed` fires with `via: manual`, and a refresh of the new feed follows. Moving onto another podcast's feed, or onto a feed with another podcast's `podcast:guid`, is a conflict.

## 9. HTTP semantics

- Conditional requests send the stored `ETag` as `If-None-Match` and `Last-Modified` as `If-Modified-Since`; both come back verbatim from the last successful fetch (a 304 may refresh them). `--force` sends neither.
- A host that ignores validators is caught by the body fingerprint (`content_fingerprint`, sha256): identical bytes are `not_modified (fingerprint)` without parsing.
- `Cache-Control: max-age` longer than the podcast's interval postpones `next_refresh_at` accordingly, up to 24 h; `no-store`/`no-cache` neither shorten the interval nor disable validators (they only stop Uguisu from reusing a *body*, which it never does). The Podcast Index caching rule from Phase 2 (`no-cache` answers are never cached) applies to provider responses, not to feeds.
- Redirects are followed by `uguisu-http` with per-hop policy checks (max 5); the final URL and the hop count reach the report, and whether every hop was permanent decides a migration (§8).
- The whole refresh — fetch, verification fetch, parse — runs under `refresh_timeout` (60 s) with a cancellation token; the persistence step runs outside it.

## 10. Transactions, idempotency and failure safety

One `BEGIN IMMEDIATE` transaction per refresh on the single writer connection (ADR 0002): podcast row, episode upserts (`ON CONFLICT (podcast_id, identity_key)`), enclosures keyed by `(episode_id, url)` (ids survive refreshes; vanished ones are deleted), extras, marks, change log, source state, fetch log (trimmed to `retain_fetches`) and event rows. Events are published on the bus **after** commit (ADR 0016), so a subscriber never sees an event whose state did not make it to disk.

`refresh; refresh` is a no-op (fingerprint), `refresh --force; refresh --force` reports every episode unchanged and writes no change rows. Every failure path writes only the source's fetch state, the podcast's `last_error`/back-off, the fetch log and the failed event; podcast metadata and episodes are exactly as before (tested for every error kind).

## 11. Events

Persisted in `events` and published on the bus; wire shape `{ schema: 1, id, occurred_at, podcast_id?, episode_id?, kind, ...payload }` (`uguisu-core::events`). Consumers: `GET /api/v1/events` (SSE, or a stored list with `after`/`limit`), logs, later webhooks.

| Kind | Payload | When |
|---|---|---|
| `podcast.added` | `title, feed_url, source_id` | podcast stored |
| `podcast.removed` | `title, feed_url, episodes, files` | the podcast's records removed, every file kept (ADR 0055) |
| `podcast.feed.refresh.started` | `source_id, feed_url, conditional` | fetch begins |
| `podcast.feed.refresh.completed` | `source_id, fetch_id, episodes{counts}, podcast_changed, truncated` | after commit of a `fetched` refresh |
| `podcast.feed.refresh.failed` | `source_id, fetch_id, error_kind, detail, http_status, consecutive_failures` | any failure |
| `podcast.feed.not_modified` | `source_id, fetch_id, reason: http304 \| fingerprint` | nothing changed |
| `podcast.metadata.updated` | `fields[]` | channel metadata changed |
| `episode.discovered` | `title, identity_key, enclosure_url, published_at` | new episode (never for malformed placeholders) |
| `episode.updated` | `fields[]` | stored episode changed |
| `episode.removal_detected` | `missing_streak` | streak reached |
| `episode.identity_ambiguous` | `duplicate_of, reasons[]` | candidate duplicate stored |
| `episode.duplicate_resolved` | `candidate, original, resolution` | a person resolved a candidate (ADR 0051); `episode_id` is the episode that remains |
| `feed.url.change_detected` | `source_id, announced, via, reason` | announcement not verified; stored as the announced source |
| `feed.url.changed` | `from_source_id, to_source_id, from, to, via` | source switched; `via` is `manual` for a person's move (ADR 0052) |

## 12. Concurrency and the refresh interface

- **One process per data directory:** `uguisu.lock` (`File::try_lock`) is held by `uguisu serve` and by every embedded stateful CLI command; a second holder gets `locked` (exit 1, hint to use `--server`) naming the pid the holder wrote to `uguisu.pid`. `feed inspect`, `search podcast` and `resolve` need no lock.
- **Coalescing:** concurrent `refresh_podcast` calls for one podcast share one spawned run and all receive the same report; the run finishes even if every caller stops waiting; the entry is cleared after commit and publish. A request that joins an in-flight run inherits its `force` setting.
- **`refresh_all`:** every `active`/`error` podcast through the same path, bounded by `refresh_concurrency` (8), cancellable; results in title order with per-podcast report or error.
- **Interface:** `Engine::{add_podcast, refresh_podcast, refresh_all, inspect_url, list_podcasts, podcast, episodes, duplicates, resolve_duplicate, move_feed, source, sources, fetch_log}`; `uguisu podcast add|list|show|refresh`, `uguisu feed inspect|refresh|status`; `POST /api/v1/podcasts`, `POST /api/v1/podcasts/{id}/refresh?force=`, `POST /api/v1/podcasts/refresh`, `GET /api/v1/feeds/{source_id}/status`, `GET /api/v1/feeds/inspect?url=` (`docs/CLI.md`, `crates/uguisu-server/src/library.rs`). Uguisu adds the loop that starts refreshes on its own: it calls `refresh_podcast` for the podcasts the database says are due, through the same coalescer, and runs only inside `uguisu serve` and the desktop shell ([`SERVICE.md`](SERVICE.md), [ADR 0027](DECISIONS/0027-service-and-refresh-scheduler.md)).

## 13. Refresh report

`RefreshReport` (`schema: 1`): `podcast_id, source_id, fetch_id, outcome (fetched | not_modified{reason} | failed{kind, detail}), http {status, final_url, redirects, etag, etag_changed, last_modified, bytes, conditional}, podcast_changed_fields[], episodes {seen, added, updated, unchanged, malformed, removed_detected, ambiguous}, feed_url (unchanged | change_detected{announced, reason} | changed{from, to, via}), warnings[], truncated, removal_suppressed?, duration_ms`. The same counters are stored per fetch in `feed_fetches`.

## 14. Configuration, limits, logging

Environment variables (`docs/CONFIGURATION.md`): `UGUISU_DATA_DIR`, `UGUISU_FEED_MAX_BYTES`, `UGUISU_FEED_MAX_ITEMS`, `UGUISU_FEED_REFRESH_TIMEOUT_MS`, `UGUISU_FEED_REFRESH_CONCURRENCY`, `UGUISU_FEED_RETAIN_FETCHES`, `UGUISU_FEED_REMOVAL_STREAK`, `UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT`, `UGUISU_FEED_SCHEDULER`, `UGUISU_FEED_REFRESH_INTERVAL_SECS`, plus the HTTP settings shared with discovery. Defaults are in `FeedConfig`/`FeedLimits` (`uguisu-core::config`).

Logging (`tracing`): `refresh started` (podcast, source, url, conditional, force), `refresh finished` (outcome, counters, duration), `refresh failed` (kind, detail, failures), `feed url changed` / `feed url change detected but not verified`, `verifying announced feed url`, `joining in-flight refresh`, `event subscriber lagged`. Bodies are never logged.

## 15. Known limitations (Phase 3)

- ~~No scheduler loop yet~~ — built; `uguisu serve` and the desktop shell refresh due podcasts by themselves, and every write a refresh makes to `next_refresh_at` spreads the podcast across its interval ([`SERVICE.md`](SERVICE.md) §2).
- Removal detection is conservative by design; a feed that legitimately drops more than half of its items in one go is noticed only once `UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT` is raised above that share.
- ~~Candidate duplicates cannot be resolved~~ — built in Phase 10: `episode resolve` and the episode list (ADR 0051).
- The verification fetch for `itunes:new-feed-url` runs inside the refresh budget; a slow announced host can time the refresh out (reported as `timeout`, retried next time).
- Media RSS is used only as an enclosure fallback; `podcast:liveItem` and `podcast:images` are kept raw, not modelled.
- Imports use one `INSERT` per episode; 10 000 items take ≈ 3 s (`docs/benchmarks/2026-09-17-phase3.md`). Multi-row inserts are a later optimization.
