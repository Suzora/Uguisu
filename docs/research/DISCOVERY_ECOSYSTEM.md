# Discovery Ecosystem Analysis — Podcast Directories, APIs and Feed Resolution

*Desk research, September 2026. The research environment could not call any of these APIs live; endpoint shapes, limits and terms are taken from public documentation and prior knowledge and are marked **[verify in Phase 2]** where a live check is required before implementation. Terms of service change; re-read them before shipping a provider.*

## 1. The landscape in one paragraph

Podcasts have no central registry. The closest things are (a) Apple's directory, which nearly every publisher submits to and which exposes the feed URL through a keyless search API, (b) **Podcast Index**, an open, community-run index of ~4M feeds with a free API and the `podcast:guid` identity system, (c) national/community directories such as **fyyd** (German-speaking Europe) and **gpodder.net**, and (d) commercial data vendors (Podchaser, Listen Notes, Taddy) that add analytics and transcripts behind paid keys. Spotify and YouTube host podcasts but do not expose RSS. Publisher websites almost always link the feed via `<link rel="alternate" type="application/rss+xml">` or use a hosting platform with a predictable feed URL pattern.

**Consequence for Uguisu:** discovery is an aggregation problem, not a single-API integration. Every provider is replaceable; RSS remains the only source of truth.

## 2. Provider profiles

### 2.1 Apple Podcasts — iTunes Search API

| Aspect | Finding |
|---|---|
| API | `GET https://itunes.apple.com/search?term=<q>&media=podcast&entity=podcast&limit=<n>&country=<cc>&lang=<lang>`; `GET https://itunes.apple.com/lookup?id=<collectionId>` |
| Auth | None |
| Search capability | Term matching with some tolerance; no true fuzzy search, no ranking explanation, max 200 results, `country` strongly affects results |
| Feed URL | **Yes** — `feedUrl` in every result (occasionally missing for Apple-hosted/subscription-only shows) |
| Metadata | `collectionId`, `collectionName`, `artistName`, `artworkUrl600`, `genres`/`genreIds`, `trackCount`, `releaseDate`, `country`, `contentAdvisoryRating`, `collectionViewUrl` |
| Popularity | Not in search results; separate chart endpoints exist (RSS/JSON "top podcasts" per country) **[verify in Phase 2]** |
| Rate limits | Documented as "approximately 20 calls per minute" per IP; 403/429 when exceeded |
| Reliability | Very high availability; response format stable for a decade; "subject to change" per docs |
| Licensing | Public API published for affiliates; personal/self-hosted search use is common practice across the ecosystem (Podgrab, PodFetch, Audiobookshelf, gPodder all use it). No key means no ToS acceptance flow, but also no guarantees |
| Dependency risk | Low-medium: Apple could add a key requirement; the fall-back is Podcast Index |
| Uguisu role | **Default provider, enabled out of the box.** Best coverage for mainstream shows; `collectionId` is a stable cross-reference (Podcast Index carries `itunesId`) |

### 2.2 Podcast Index

| Aspect | Finding |
|---|---|
| API | `https://api.podcastindex.org/api/1.0/` — `search/byterm?q=`, `search/bytitle?q=`, `search/byperson?q=`, `podcasts/byfeedurl`, `podcasts/byitunesid`, `podcasts/byguid`, `podcasts/trending`, `recent/feeds`, `episodes/byfeedid`. Optional `fulltext`, `similar`, `max`, `clean` parameters **[verify in Phase 2]** |
| Auth | Free API key + secret from api.podcastindex.org. Headers: `User-Agent`, `X-Auth-Key`, `X-Auth-Date` (unix seconds), `Authorization` = SHA-1 hex of `key + secret + date` |
| Search capability | Term search across title/author/owner; `similar=true` widens matching; results carry no relevance scores. Fuzzy ranking must be done in Uguisu |
| Feed URL | **Yes** — `url` plus `originalUrl`, and `link` (website) |
| Metadata | `id`, `podcastGuid` (Podcasting 2.0 GUID), `itunesId`, `title`, `author`, `ownerName`, `description`, `image`/`artwork`, `language`, `categories`, `episodeCount`, `lastUpdateTime`, `newestItemPubdate`, `medium`, `dead`, `locked`, `explicit`, `funding`, `value` |
| Popularity | `trending` endpoint; no per-result popularity in search |
| Rate limits | Not published; ToS states limits are enforced at Podcast Index's discretion. Be conservative (≤ 1 req/s, cache aggressively) |
| Reliability | Good; nonprofit-run; occasional slow responses; index freshness depends on aggregators |
| Licensing | ToS at api.podcastindex.org; API and data intentionally open (MIT-licensed docs/tools). Users must register their own key — Uguisu must never ship a shared key |
| Dependency risk | Medium: community project; the open data model and `podcastGuid` make it strategically important |
| Uguisu role | **Optional provider, enabled when the user enters a key.** Adds feed health flags (`dead`, `locked`), GUIDs and the best cross-reference data |

### 2.3 fyyd

| Aspect | Finding |
|---|---|
| API | `https://api.fyyd.de/0.2/` — `search/podcast?title=&term=&url=`, `podcast?podcast_id=`, `podcast/episodes`, `feature/podcast/hot`, `categories`. Docs at fyyd.de/api-doc **[verify in Phase 2]** |
| Auth | None for read endpoints; OAuth2 for write/user features |
| Search capability | Title/term search; strong for German-language and European podcasts underrepresented elsewhere |
| Feed URL | **Yes** — `xmlURL`; also `htmlURL` (website), `imgURL` |
| Metadata | `title`, `author`, `description`, `language`, `categories`, `episode_count`, `lastpub`, `rank`/`subscribers` (fyyd-internal popularity) **[verify]** |
| Rate limits | Not published; single-maintainer project — be gentle |
| Licensing | Free public API; no formal ToS located **[verify]** |
| Dependency risk | High (one maintainer), low impact (optional provider) |
| Uguisu role | **Optional provider, off by default; recommended for DACH users.** |

### 2.4 gpodder.net directory

| Aspect | Finding |
|---|---|
| API | `GET https://gpodder.net/search.json?q=<q>`; `toplist.json`; `podcast.json?url=` — public directory API, no auth |
| Feed URL | Yes (`url`) plus `title`, `description`, `logo_url`, `subscribers`, `website` |
| Quality | Smaller and older index; subscriber counts are a genuine popularity signal for its user base |
| Dependency risk | Medium (community-run, occasional outages) |
| Uguisu role | **Optional, low-priority provider** — cheap to implement; mainly a tie-breaker and a second source for niche feeds **[verify availability in Phase 2]** |

### 2.5 Commercial providers — evaluated, excluded from v1

| Provider | Why excluded for v1 | Possible later use |
|---|---|---|
| **Podchaser** (GraphQL, OAuth client credentials, commercial tiers) | Paid beyond a small dev tier; ToS restricts caching/redistribution; analytics focus irrelevant to archival | Optional enrichment provider if users bring keys |
| **Listen Notes** (REST, API key, free tier very limited, ToS restricts storing results) | Storage/caching restrictions conflict with Uguisu's discovery cache; per-call pricing | Same |
| **Taddy** (GraphQL, API key, ~100 req/hour free) | Small free quota; transcript/webhook features are outside discovery scope | Transcript source post-v1 |
| **Spotify Web API** | Search returns shows without RSS URLs; OAuth app registration; ToS forbids using data to feed other services | Never for feeds; at most "resolve a Spotify show URL by title" |
| **YouTube** | No audio RSS; downloading is out of Uguisu's scope (PinePods covers it) | — |

Excluding these keeps the provider layer free of keys Uguisu would have to ship, and free of terms that forbid caching.

## 3. How podcast websites expose feeds

Ordered by how often each mechanism succeeds in practice:

1. **HTML autodiscovery:** `<link rel="alternate" type="application/rss+xml" href="…">` (also `application/atom+xml`, `application/feed+json`). Multiple links are common (per-category feeds, comments feeds on WordPress) — rank by `title` containing "podcast", by enclosure presence after fetching, and by media type.
2. **Hosting-platform URL patterns** (deterministic, no crawling):
   - Spotify for Podcasters / Anchor: `https://anchor.fm/s/<id>/podcast/rss`
   - Libsyn: `https://<show>.libsyn.com/rss`
   - Buzzsprout: `https://feeds.buzzsprout.com/<id>.rss`
   - Transistor: `https://feeds.transistor.fm/<slug>`
   - Simplecast: `https://feeds.simplecast.com/<id>`
   - Megaphone: `https://feeds.megaphone.fm/<slug>`
   - Acast: `https://feeds.acast.com/public/shows/<slug>`
   - Podbean: `https://feed.podbean.com/<slug>/feed.xml`
   - Spreaker: `https://www.spreaker.com/show/<id>/episodes/feed`
   - Captivate: `https://feeds.captivate.fm/<slug>/`
   - RSS.com: `https://media.rss.com/<slug>/feed.xml`
   - Audioboom: `https://audioboom.com/channels/<id>.rss`
   - SoundCloud: `https://feeds.soundcloud.com/users/soundcloud:users:<id>/sounds.rss`
   - Podigee: `https://<slug>.podigee.io/feed/mp3`
   - Ausha, Podcaster.de, Letscast, Julep and others follow similar `feed/…` conventions **[collect exact patterns in Phase 2 fixtures]**
3. **Directory page URLs pasted by users:**
   - `https://podcasts.apple.com/<cc>/podcast/<slug>/id<collectionId>` → Apple lookup by id → `feedUrl`
   - `https://podcastindex.org/podcast/<id>` → Podcast Index `podcasts/byfeedid`
   - `https://open.spotify.com/show/<id>` → no feed; fall back to title search with a clear message
   - `https://fyyd.de/podcast/<slug>/<id>` → fyyd `podcast?podcast_id=`
4. **Well-known paths on the site itself:** `/feed/podcast/` (PowerPress/WordPress), `/feed`, `/rss`, `/rss.xml`, `/feed.xml`, `/podcast.rss`, `/podcast/feed`. Try only after (1)–(3) fail, with a strict request budget.
5. **Redirect following and HTTPS upgrade**, always re-validated by the SSRF policy.
6. **Feed self-reference** once fetched: `<atom:link rel="self">` gives the canonical URL; `<itunes:new-feed-url>` announces a move.

Each resolution attempt records which mechanism produced the feed; that becomes the podcast's source provenance.

## 4. Fuzzy search possibilities

No provider offers explainable fuzzy ranking. Podcast Index's `similar` and Apple's tolerant term matching widen recall; precision and ordering must come from Uguisu:

- **Query normalization:** Unicode NFKC, case folding, punctuation stripping, diacritics folding (keeping the original for display), stop-word-light tokenization (keep "the" for exact match, ignore it for fuzzy).
- **Recall expansion:** query each enabled provider with the raw query; optionally a second pass with the top tokens only when the first pass returns fewer than N results.
- **Ranking signals** (all visible in a `RankingExplanation`): exact title, normalized title, token-set ratio, Damerau-Levenshtein / Jaro-Winkler on title, author match, website host match, provider-supplied popularity (normalized per provider), feed quality (feed URL present, HTTPS, episode count, recent update, not `dead`), provider confidence weight, artwork present.
- Libraries: `strsim` (Jaro-Winkler, Damerau-Levenshtein) and `rapidfuzz` (token ratios) — both pure Rust; benchmarked in Phase 2 before committing.

## 5. Recommendation and status

| Tier | Provider | Default | Key needed | Reason | Status (2026-09-17, after Part B) |
|---|---|---|---|---|---|
| 1 | Apple iTunes Search | on | no | Coverage, feed URLs, zero setup | **implemented and live-verified** (recorded fixtures, §6b) |
| 1 | Website / URL resolver | on | no | Handles pasted RSS, website and directory URLs; no third party | **implemented and live-verified** against 20 real websites, feeds and directory pages (§6b) |
| 2 | Podcast Index | off until key entered | yes (free) | Open data, `podcastGuid`, `dead`/`locked`, cross-references | **implemented; live-verified only for the unauthenticated path** (401 shapes recorded). Authenticated search/lookup responses are still unverified: no key was available in the Part B environment |
| 3 | fyyd | — | no | DACH coverage | **deferred**: the API answers (schema observed live, §6b) but its primary documentation is still unreachable; not implemented |
| 3 | gpodder.net | off | no | Cheap second opinion, subscriber counts | **implemented and live-verified**; note that live subscriber counts are 0 for every result (§6b) |
| — | Podchaser / Listen Notes / Taddy / Spotify | not implemented | — | Licensing, caching restrictions, no feeds | excluded |

"Implemented" means built and tested against documentation and spec-derived fixtures; "live-verified" means the same code was run against the real service and the recorded responses now drive tests. The two are kept apart on purpose.

Design consequences adopted in [`DISCOVERY.md`](../DISCOVERY.md): provider trait with health/rate-limit state, parallel querying with incremental results, cross-provider dedup keyed on normalized feed URL → iTunes id → Podcast Index GUID → (title, author) similarity, per-provider TTL cache, and a resolver that classifies any pasted input.

## 6a. Verification log — 2026-09-17 (Phase 2, Part A, desk research)

Primary sources were fetched and read directly; live API calls were not possible from the Part A environment (egress blocked). "Live" items remain open for Part B.

| Provider | Primary source read | Confirmed | Corrected vs. Phase 1 | Still needs a live check |
|---|---|---|---|---|
| Apple iTunes Search | developer.apple.com archive, *iTunes Search API → Constructing Searches* and *Understanding Search Results* (last updated 2017-09-19) | `GET https://itunes.apple.com/search` with `term` (required), `country` (ISO 3166-1 alpha-2, default US), `media=podcast`, `entity=podcast` or `podcastAuthor`, `limit` 1–200 (default 50), `explicit=Yes|No`, `version`, `callback`; `lookup?id=`; "limited to approximately 20 calls per minute (subject to change)"; large sites are asked to cache. Result keys `wrapperType`, `kind`, `artistName`, `collectionName`, `trackName`, `artworkUrl60/100`, `*ViewUrl`, `*Explicitness`, `country`, `primaryGenreName`. | `lang` accepts **only `en_us` and `ja_jp`** (Phase 1 implied arbitrary languages). `feedUrl`, `artworkUrl600`, `genres`, `genreIds`, `trackCount`, `releaseDate`, `contentAdvisoryRating` are not in the 2017 table but are returned for podcasts in practice → fixtures label them "documented by observation, verify live". | 429/403 behaviour above 20/min; exact podcast result keys; `text/javascript` content type. |
| Podcast Index | `Podcastindex-org/docs-api` → `docs/pi_api.yaml` (OpenAPI 3) and `Podcastindex-org/legal/TermsOfService.md` | Base `https://api.podcastindex.org/api/1.0`; headers `User-Agent` (required), `X-Auth-Key`, `X-Auth-Date` (UTC unix seconds, 3-minute window), `Authorization` = lowercase hex SHA-1 of `key + secret + date`; `search/byterm?q&max&similar&fulltext&clean&aponly&val`, `search/bytitle`, `search/byperson` (episodes), `podcasts/byfeedurl?url`, `byitunesid?id`, `byguid?guid`, `byfeedid?id`; response envelope `{status, feeds[], count, query, description}` (400/401 → `{status, description}`); `feed_search` fields: `id, podcastGuid, title, url, originalUrl, link, description, author, ownerName, image, artwork, lastUpdateTime, lastCrawlTime, lastParseTime, lastGoodHttpStatusTime, lastHttpStatus, contentType, itunesId, generator, language, explicit, type, medium, dead, episodeCount, crawlErrors, parseErrors, categories{}, locked, imageUrlHash, newestItemPubdate`. ToS: limits set and enforced at PI's discretion (§4 "API Limitations"); **cached copies may not be kept longer than the cache header permits (§5)**; **attribution required (§7)**; every API client uses its own key. | Phase 1 listed `similar` as "widens matching"; the spec says it *includes similar matches and prioritizes title matches for byterm*. Rate limit: no number published → Uguisu defaults to 1 request/s, concurrency 2. Cache TTL must honour `Cache-Control`. | Presence and values of `Cache-Control` on live responses; latency; whether `search/bytitle` adds recall. |
| gpodder.net | `gpodder/mygpo` → `doc/api/reference/directory.rst` | `GET /search.json?q=&scale_logo=` (no auth); result fields `url, title, description, subscribers, logo_url, scaled_logo_url, website, mygpo_link`; `GET /api/2/data/podcast.json?url=` (lookup by feed URL, 404 if unknown); `toplist/{n}.json`. No rate limit documented. | None. | Service availability and latency (community-run); actual field completeness. |
| fyyd | **not reachable** (fyyd.de, codeberg.org, hexdocs mirrors all blocked); only secondary sources (`api.fyyd.de/0.2/search/podcast?title=…`, `xmlURL`, no auth for reads) | — | — | **Deferred.** Not implemented in Phase 2; the provider abstraction stays ready. Revisit when `fyyd.de/api-doc` can be read or responses are recorded. |

Consequences applied in the implementation: Apple `lang` is only sent for `en_us`/`ja_jp`; Podcast Index cache entries expire at `min(configured TTL, Cache-Control max-age)` and the provider carries an attribution string for the UI; fyyd is listed as deferred in the provider matrix.

## 6b. Live verification log — 2026-09-17 (Phase 2, Part B)

Network access was available; every provider and a set of real websites and feed hosts were exercised with the release build of `uguisu` and with `record-fixtures`. Recordings live in `tests/fixtures/discovery/live/` (`origin: recorded`, credentials stripped) and are replayed by `crates/uguisu-discovery/tests/live_fixtures.rs`. Spec-derived fixtures stay under `tests/fixtures/discovery/spec/`.

### Apple iTunes Search — verified

| Aspect | Observed live |
|---|---|
| Endpoints | `GET /search?term&country&media=podcast&entity=podcast&limit` and `GET /lookup?id&entity=podcast` answer as documented; unknown ids give `200` with `resultCount: 0` (no 404). |
| Content type | `text/javascript; charset=utf-8` (JSON body). The client accepts it. |
| Fields | Podcast results carry `wrapperType: track`, `kind: podcast`, `collectionId`/`trackId`, `artistName`, `collectionName`, `feedUrl`, `artworkUrl30/60/100/600`, `genres` (always includes `"Podcasts"`), `genreIds`, `trackCount`, `releaseDate`, `collectionExplicitness`/`trackExplicitness`, `contentAdvisoryRating`, `primaryGenreName`, `country` (storefront, ISO 3166-1 alpha-3 such as `USA`/`DEU`), `currency`, prices. All "documented by observation" fields from Part A are confirmed. |
| Cache | `Cache-Control: max-age=86400` on searches, shorter on lookups (`18681` observed). Uguisu caps the TTL at its configured 900 s search / 86 400 s lookup TTLs. |
| Rate limit | A burst of 45 sequential searches in 20 s (≈ 135/min) was answered `200` throughout; no `403`/`429` and no rate-limit headers. The documented "approximately 20 calls per minute" is not enforced strictly (or not from this egress). Uguisu keeps the 20/min throttle as documented courtesy. |
| Search semantics | **Not fuzzy.** `darknet diariez` → 0 results; `darknet` → 25 results including the show; `Jack Rhysider` (author) → 5 results. Misspelling tolerance must therefore come from another provider or the user. |
| Correction applied | `country` was previously copied into `language` when no storefront was configured; it is a storefront code, not a language, and is no longer mapped. |

### gpodder.net — verified

| Aspect | Observed live |
|---|---|
| Endpoints | `GET /search.json?q&scale_logo=256` and `GET /api/2/data/podcast.json?url=` answer as documented; unknown feed URLs give a **`404` HTML page** (not JSON), which the provider maps to "not found". |
| Fields | Exactly the documented set: `url, title, author, description, subscribers, logo_url, scaled_logo_url, website, mygpo_link`. `scaled_logo_url` and `mygpo_link` are plain `http://` URLs. |
| Data quality | `subscribers` is `0` for every result seen (Darknet Diaries, Gemischtes Hack, author queries), so the popularity signal from gpodder.net is currently always 0. Several feed variants of one show are listed as separate entries (ad-free feed, old feed paths, current feed); dedup merges them via title/author and website. |
| Cache | `Cache-Control: max-age=3600`, `Vary: Accept-Language`. |
| Latency | 0.7–1.7 s per request from the test environment (slower than Apple). |
| Search semantics | Not fuzzy either (`darknet diariez` → `[]`). |

### Podcast Index — partially verified

| Aspect | Observed live |
|---|---|
| Unauthenticated / invalid credentials | `401` with `Content-Type: application/json` but a **plain-text body** ("Authorization header value either not set or blank…", "The X-Auth-Key header contains an invalid API key…", "X-Auth-Date header value either not set, blank or corrupt…"), `Cache-Control: no-cache, must-revalidate`, served by Cloudflare. The provider maps the status before parsing, so the non-JSON body is harmless. |
| Authenticated responses | **Not verified**: no API key was available in the Part B environment. The mapping stays validated against the OpenAPI examples only. When a key is present, run `record-fixtures provider podcastindex --case search_darknet --query "Darknet Diaries"` (and the lookup cases) and diff against `spec/podcastindex`. |
| Cache rule | Because the service answers with `no-cache` on the paths seen, `Response::cache_max_age` now returns zero for `no-cache`/`no-store`, and the registry does not cache such answers at all (ToS §5). |

### fyyd — deferred, schema observed

`GET https://api.fyyd.de/0.2/search/podcast?title=…&count=…` answers `200` with `{status: 1, msg, meta{paging, API_INFO{API_VERSION: "0.2"}, duration}, data[]}`; each podcast has `id, title, slug, xmlURL, htmlURL, imgURL, layoutImageURL, thumbImageURL, smallImageURL, microImageURL, language, generator, categories[ids], lastpub, rank, url_fyyd, description, subtitle, episode_count, status, status_since, author, paymentURL, iflags, color, tcolor`; headers `Cache-Control: no-cache, no-store, must-revalidate`. The primary documentation (the fyyd API repository on GitHub) is still not reachable from this environment, so the provider remains deferred; the observation is stored as `tests/fixtures/discovery/live/fyyd/search_darknet.json` for the day it is implemented.

### Resolver — verified against real sites

| Input | Result |
|---|---|
| `darknetdiaries.com` (bare domain) and `https://darknetdiaries.com/` | `<link rel="alternate" type="application/rss+xml">` → `feeds.megaphone.fm/darknetdiaries` → `301` → `https://podcast.darknetdiaries.com/` (1 MB feed, validated) |
| `https://feeds.megaphone.fm/darknetdiaries` | redirect followed, feed validated |
| `https://podcasts.apple.com/us/podcast/darknet-diaries/id1296350485` | Apple lookup → feed validated |
| `https://chaosradio.de/` | three announced feeds (mp3/m4a/opus); the first validated one is returned after 2 requests |
| `https://atp.fm/` | relative `href="/rss"` resolved |
| `https://twit.tv/shows/security-now` | absolute link → `feeds.twit.tv/sn.xml` |
| `https://www.buzzsprout.com/1` | oembed link ignored, RSS link used → `rss.buzzsprout.com/1.rss` (via `feeds.buzzsprout.com` redirect) |
| `https://lexfridman.com/podcast/` | no feed link; well-known path `/feed/podcast/` hit (WordPress/PowerPress) |
| Direct feeds: Libsyn, Buzzsprout, Transistor, Anchor, Simplecast, Podigee, FeedBurner, ATP CDN, chaosradio | all validated as podcast feeds |
| `https://www.dancarlin.com/hardcore-history-series/` | correctly **fails** with `no_feed_link_found`: the page announces only the blog and comment feeds (no enclosures); the podcast feed is not linked |
| `https://www.gemischteshack.de/` | correctly **fails**: SPA without feed links whose catch-all route answers HTML for every well-known path; budget of 8 requests respected |
| `open.spotify.com/show/…` | `no_feed_available` (Spotify), no request made |
| `podcastindex.org/podcast/920666` | `provider_unavailable` while Podcast Index has no key |
| `fyyd.de/podcast/…` | `provider_unavailable` (fyyd deferred) |
| `https://httpbin.org/redirect/6` | `network/too_many_redirects` (limit 5) |

Correction applied: several real feeds (TWiT, Simplecast, Transistor, Megaphone) put their own URL into `itunes:new-feed-url`; the resolver reported that as `moved_to`. It now reports a move only when the announced URL differs.

### SSRF policy — verified live

All of the following are refused with exit code 6 before any connection is made, including the DNS-level cases that only the resolver hook can catch: `http://127.0.0.1/`, `http://169.254.169.254/latest/meta-data/`, `http://10.0.0.1/`, `http://192.168.1.1/`, `http://2130706433/`, `http://0x7f.0.0.1/`, `http://0177.0.0.1/`, `http://127.0.0.1.nip.io/` (public name → 127.0.0.1), `http://localtest.me/` (→ ::1), `https://httpbin.org/redirect-to?url=http://127.0.0.1/` and `…url=http://169.254.169.254/` (public host redirecting to private), `http://example.com:22/` (port not allowed). `http://[::1]/` and `ftp://…` are rejected earlier as "not a URL" (exit 2) by the query classifier, which only accepts `http(s)` and `feed` URLs with a host name.

## 7. Open items after Part B

- **Podcast Index with a real key**: record `search_byterm`, `byfeedurl`, `byitunesid`, `byguid` and check `Cache-Control` on `200` answers; confirm `similar`/`fulltext` semantics.
- **fyyd**: after v1 (ADR 0005, 2026-10-03). The live schema is recorded and the abstraction is ready for the day the primary documentation can be read.
- **Misspelling recall**: closed in Phase 10 by query relaxation (ADR 0005, 2026-10-03; `DISCOVERY.md` §7). Podcast Index `similar=true` is still unverified and unused.
- **gpodder.net popularity**: subscriber counts are 0 live; the signal stays wired but contributes nothing until the directory publishes counts again.
