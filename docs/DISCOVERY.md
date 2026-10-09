# Discovery Architecture (as built)

Discovery answers one question: *"Which podcast did the user mean, and what is its feed URL?"* It lives in `uguisu-discovery` and has no dependency on storage, downloads or the archive. Its output is a verified feed plus provenance; from there the rest of Uguisu treats the podcast exactly like a pasted RSS URL.

Research: [`research/DISCOVERY_ECOSYSTEM.md`](research/DISCOVERY_ECOSYSTEM.md) · decisions: [ADR 0005](DECISIONS/0005-discovery-provider-strategy.md), [ADR 0011](DECISIONS/0011-http-client-policy.md) · CLI: [`CLI.md`](CLI.md).

## 1. Pipeline

```text
input ──► NormalizedQuery ──► is URL? ──yes──► Resolver ──► ResolvedFeed / ResolveFailure
              │ term
              ▼
   ProviderRegistry fan-out (parallel, per-provider throttle / circuit breaker / timeout / cache)
              │  Vec<PodcastCandidate> per provider, as they arrive
              ▼
   dedup::group ──► merge::merge (one canonical candidate per group, field provenance)
              ▼
   rank::rank (ten weighted signals, RankingExplanation per result)
              ▼
   SearchResponse snapshot ──► (streamed on every arrival; first at the soft deadline; final at completion or hard deadline)
              │  user selects
              ▼
   Resolver::resolve_candidate ──► ResolvedFeed  (handed to the engine on "Add", Phase 3)
```

Searching never subscribes.

## 2. Module map

| Module | Responsibility |
|---|---|
| `query` | `NormalizedQuery`: NFKC + case folding + punctuation/whitespace normalization (`fold_text`), ASCII transliteration (`ascii_fold`), tokens, URL detection (`https://…`, `feed://`, bare domains) |
| `candidate` | `PodcastCandidate` (provider-independent), `ProviderIdentity`, `Popularity`, `FeedHealthHints`, per-field `provenance` |
| `provider` | `DiscoveryProvider` trait, `ProviderInfo` (capabilities, attribution, default throttle, trust), `ProviderContext`, `ProviderError` taxonomy, `ProviderResponse` with cache hints |
| `providers::{apple, podcastindex, gpoddernet}` | Built-in providers, each mapping its own response format and nothing else |
| `registry` | `ProviderRegistry`: throttle (governor + semaphore), circuit breaker with exponential cool-down, per-call timeout, health statistics, cache integration, `ProviderStatus` |
| `cache` | moka cache keyed `(provider, kind, normalized key)` with per-entry TTL = min(configured, provider `Cache-Control max-age`), in front of an optional `CacheStore` (ADR 0030) |
| `fuzzy` | token-set / token-sort ratio, token coverage, Jaro-Winkler, combined `title_similarity` |
| `dedup` | union-find grouping on strong and medium keys; `AmbiguityNote`s for similar-but-unmerged pairs |
| `merge` | deterministic canonical candidate per group |
| `rank` | explainable scoring (`RankingExplanation`), deterministic ordering |
| `search` | `SearchEngine`: fan-out, deadlines, streaming, `SearchOutcome`, `ProviderOutcome` |
| `resolve` | `Resolver`: URL classification, provider lookups for directory pages, fetch + probe, HTML autodiscovery, platform patterns, well-known paths, validation, canonicalization, budget, provenance |
| `assemble` | Builds registry + engine + resolver from `DiscoveryConfig`; the engine passes its `discovery_cache` store, the CLI's embedded `search podcast` and `resolve` pass none |
| `testing` | Fixture format and wiremock loader (feature `testing`) |

## 3. Providers

| Provider | Default | Credentials | Throttle (from docs) | Trust | Attribution | Status 2026-09-17 (after Part B) |
|---|---|---|---|---|---|---|
| Apple (iTunes Search) | on | none | 20/min, concurrency 2 (45 req/20 s were not throttled live) | 0.8 | "Search results from Apple Podcasts (iTunes Search API)" | implemented, **live-verified** |
| Podcast Index | on when key+secret set | user's own key/secret | 60/min, concurrency 2 (limits unpublished) | 0.9 | required by ToS §7: "Podcast data from Podcast Index (podcastindex.org)" | implemented; **live-verified for the 401 path only** (no key available) |
| gpodder.net | off (opt-in) | none | 60/min, concurrency 2 | 0.6 | "Directory data from gpodder.net" | implemented, **live-verified** (subscriber counts are 0 live) |
| fyyd | — | — | — | — | — | after v1 (schema observed live, primary docs unreachable; ADR 0005) |
| Website/URL resolver | on | none | request budget 8 / 30 s | — | — | implemented, **live-verified** against 20 real inputs |

Live findings and their consequences are logged in [`research/DISCOVERY_ECOSYSTEM.md`](research/DISCOVERY_ECOSYSTEM.md) §6b. Two matter for users: **no provider is fuzzy** — Apple and gpodder.net return nothing for `darknet diariez`, so Uguisu recalls a typo by relaxing a query that found nothing (§7) and lets its typo-tolerant ranking order what the looser query returns (Podcast Index `similar=true` is the one provider-side option and is unverified) — and **gpodder.net reports 0 subscribers**, so the popularity signal is currently inert.

Each provider call goes through the registry: **cache** → **circuit breaker** (opens after 3 consecutive transient failures, cool-down 30 s doubling to 10 min, 1 h after credential failures, one half-open probe) → **throttle** → **timeout** (`provider_timeout`, default 6 s). Provider HTTP calls use a fail-fast retry policy (one retry; a `Retry-After` above two seconds surfaces as `RateLimited` so the breaker backs off instead of a search waiting).

## 4. Deduplication

| Key | Strength | Merge? |
|---|---|---|
| normalized feed URL (scheme dropped, `www.` and default port removed, tracking params stripped, trailing slash removed) | strong | yes (`feed_url`) |
| iTunes id (Apple `collectionId` = Podcast Index `itunesId`) | strong | yes (`itunes_id`) |
| `podcast:guid` | strong | yes (`podcast_guid`) |
| same website host + title similarity ≥ 0.90 | medium | yes (`website_and_title`) |
| title similarity ≥ 0.95 and author similarity ≥ 0.90 | medium | yes (`title_and_author`) |
| title similarity ≥ 0.85 without any of the above | weak | **no** — recorded as `AmbiguityNote { other_title, similarity, reason }` |

Grouping is union-find; output is deterministic and membership does not depend on provider arrival order (property-tested).

## 5. Canonical merge

Members are ordered by provider trust, then provider id and reference. Per field: `feed_url` prefers Podcast Index › Apple › gpodder.net; `artwork` prefers Apple › Podcast Index › gpodder.net; `description` is the longest; `categories` are the case-insensitive union; identities and popularity signals are all kept; health hints take the first value in trust order; every chosen field records its source in `provenance`. Merging is order-independent (property-tested).

## 6. Ranking

Score = Σ weight × value, every value in 0..1, defaults unchanged from the Phase 1 design:

| Signal | Weight | Value |
|---|---|---|
| `exact_title` | 3.0 | folded query equals folded title; spacing variants ("dark net" vs "darknet") count when the user typed more than one word |
| `token_set` | 2.5 | token-set ratio (a query that is a subset of the title scores 1.0) |
| `jaro_winkler` | 1.5 | max of Jaro-Winkler on the strings, on the whitespace-stripped strings, and 0.95 × per-token coverage |
| `author` | 1.0 | improvement of the token-set ratio when the author is appended to the title (rewards "rhysider darknet", neutral for pure title queries) |
| `website` | 0.5 | a query token of ≥ 4 characters occurs in the website host |
| `popularity` | 1.0 | best normalized provider popularity (gpodder.net subscribers, log-scaled) |
| `feed_quality` | 1.0 | 0 when the directory marks the feed dead; otherwise feed URL 0.5 + HTTPS 0.1 + has episodes 0.15 + updated within two years 0.15 + not locked 0.1 |
| `provider_trust` | 0.5 | best trust among contributing providers |
| `artwork` | 0.2 | artwork present |
| `agreement` | 0.8 | (providers − 1) / 2, capped at 1 |

Rationale for the two refinements made during Phase 2 (weights untouched): the spacing-variant rule fixed "dark net diaries" ranking a show called "Dark Net" above "Darknet Diaries" (test `brief_query_table_ranks_darknet_diaries_first`), and the author signal is defined as a *gain* so that an author query helps without adding constant noise to title-only queries (test `author_signal_rewards_author_queries_only`). Ties break on title, then provider reference, so ordering is deterministic. `RankedCandidate` carries `rank`, `score`, `confidence` (score / max score), `explanation.signals[]` (name, weight, value, contribution, note) and `ambiguities[]`.

## 7. Search engine

- Inputs: `SearchRequest { query, providers?, limit?, country?, no_cache }`.
- Every enabled (and requested) provider is queried concurrently with one child `CancellationToken`.
- **Soft deadline** (default 2 s): the first snapshot is emitted even if providers are pending. **Hard deadline** (default 8 s): pending providers are marked `timed_out` and cancelled.
- Snapshots are recomputed (dedup → merge → rank) on every arrival; `search()` returns the final one, `search_stream()` yields every snapshot (`complete: false … true`).
- Outcomes: `results`, `no_results` (every asked provider answered, nothing matched), `all_providers_failed` (a provider skipped by its open circuit breaker counts as failed), `no_providers_enabled`. Per provider: `ok | skipped | failed | timed_out | cancelled`, candidate count, latency, `from_cache`, error kind.
- **Relaxation** ([ADR 0005](DECISIONS/0005-discovery-provider-strategy.md), Phase 10 amendment): when every asked provider answered a term query with nothing, one more round asks the same providers for at most two looser queries and ranks the answers against the query as typed. `relaxed[]` names each looser query with its own per-provider outcomes; `providers[]` and `outcome` describe the query as typed. A failure, a timeout, an open breaker, any result, a URL or an empty query means no relaxation. The round shares the cache, the throttles and the hard deadline.

  | Query (folded words) | Relaxed queries |
  |---|---|
  | `darknet diariez` | `darknet`, `diariez` |
  | `the darknet diariez` | `the darknet`, `darknet diariez` |
  | `darknett` | `darknet` |
  | `abc` | none (a single word needs four characters) |

- Attribution strings of providers that contributed results, in either round, are returned for the UI (Podcast Index requires it).
- URLs are not searched: `search()` returns `no_results` with `query.kind = url` so callers resolve instead (the CLI and the UI do this automatically).

## 8. Feed resolution

`Resolver::resolve(input)` / `resolve_candidate(candidate)`:

1. **Classify**: not a URL → `not_a_url`; Apple Podcasts page → provider lookup by iTunes id (Podcast Index first when enabled, then Apple); Podcast Index page → lookup by feed id; Spotify / YouTube → `no_feed_available`; fyyd page → `provider_unavailable` (after v1); anything else → fetch.
2. **Fetch** through the `Feed` client (SSRF policy, ≤ 5 redirect hops re-validated, 50 MB cap) and **probe** the body with `uguisu_feed::probe`.
3. **HTML** → autodiscovery: `<link rel="alternate">` with RSS/Atom/XML types (relative hrefs and `<base href>` honoured; podcast-titled RSS first, Atom next, comment feeds last) → platform patterns derivable from the page URL (Libsyn, Buzzsprout, Transistor, Podbean, Captivate, RSS.com, Podigee, Acast, Spreaker, Audioboom, Megaphone) → well-known paths (`/feed/podcast/`, `/feed`, `/rss`, `/feed.xml`, `/rss.xml`, `/podcast.rss`, `/podcast/feed`) only when nothing else was found. Each candidate is fetched and probed; the first feed with media items wins.
4. **Validate**: a feed must have ≥ 1 item with media (`enclosure`, Atom enclosure link, RSS 1.0 `enc:enclosure` or `podcast:alternateEnclosure`), else `feed_invalid`.
5. **Canonicalize**: `atom:link rel="self"` on the same host is fetched once and, when it serves the same feed, recorded as `canonical_url`; `itunes:new-feed-url` is recorded as `moved_to` with a warning; an `http` feed is upgraded to `https` when the HTTPS variant serves the same feed.
6. **Budget**: at most 8 requests and 30 s per resolution → `budget_exceeded`.

Errors (`ResolveError`, tagged `kind`): `not_a_url`, `not_a_feed`, `no_feed_link_found { tried[] }`, `feed_invalid`, `http_status`, `network`, `blocked_by_policy`, `provider_unavailable`, `no_feed_available { platform }`, `budget_exceeded`, `cancelled` — each with a user-facing `suggestion()`. Every step is recorded in `provenance[]` (`classify`, `provider_lookup`, `fetch`, `sniff`, `autodiscovery`, `platform_pattern`, `well_known_path`, `validate`, `canonicalize`, `https_upgrade`).

`ResolvedFeed`: `feed_url`, `canonical_url`, `moved_to`, `website`, `title`, `author`, `description`, `artwork`, `podcast_guid`, `language`, `item_count`, `items_with_media`, `newest_item`, `locked`, `feed_kind`, `provenance[]`, `warnings[]`, `verified_at`.

## 9. Caching, limits and health

- Cache entries live `min(configured TTL, provider max-age)`: search 15 min, lookup 24 h by default; a provider's `no-cache` or `no-store` stores nothing; `no_cache` bypasses reads (writes still happen). Memory is in front; behind it, wherever the engine runs discovery, the `discovery_cache` table keeps entries across restarts, expired on read and swept by housekeeping ([ADR 0030](DECISIONS/0030-discovery-persistence.md)). The CLI's embedded `search podcast` and `resolve` open no database and cache in memory only.
- Throttle: token bucket per provider from `ProviderInfo.throttle` plus a concurrency semaphore; cancellation-aware.
- Health per provider: circuit state, consecutive failures, last success/failure, EWMA latency, totals — exposed by `GET /api/v1/discovery/providers`, which the web UI's Discover view shows as its provider strip. The CLI's provider lines show each search's per-provider outcome, not health.

## 10. Configuration

Every key, its default and its accepted values: [`CONFIGURATION.md`](CONFIGURATION.md), sections "Discovery" and "Networking". The discovery stack reads them once, when it is built.

## 11. Surfaces

```text
uguisu search podcast "<query>" [--provider a,b] [--limit n] [--country CC] [--explain] [--resolve] [--no-cache] [--json]
uguisu resolve <url> [--json]

GET  /api/v1/discovery/search?q=&providers=&limit=&country=&no_cache=&stream=   # JSON, or SSE snapshots with stream=true
GET  /api/v1/discovery/providers                                                  # status, health, attribution, cache counters
GET|POST /api/v1/discovery/resolve  (?input= | {"input": …})                     # ResolvedFeed, or 400/403/422/500/502/504 + failure body
GET  /api/v1/discovery/records?limit=                                             # recorded resolutions, newest first (ADR 0030)
GET  /api/v1/discovery/records/{id}                                               # one recorded resolution
```

JSON shapes are versioned by `schema` (currently 1) on `SearchResponse`; `ResolvedFeed` and failure bodies are documented in [`CLI.md`](CLI.md).

## 12. Logging

`tracing` events: `search started`, `search relaxed`, `provider request started`, `provider request completed`, `provider request failed`, `provider results received`, `provider skipped: circuit open`, `provider circuit opened`, `provider search served from cache`, `search completed`, `feed resolution started`, `feed resolution step`, `feed resolution succeeded`, `feed resolution failed`, `resolver refused every address`. Fields: `provider`, `query`, `latency_ms`, `error`, `kind`, `step`, `url`, `feed_url`, `requests`. Credentials never appear in logs (`Secret` redacts them; auth headers are not logged).

## 13. Tests and benchmarks

- Unit tests in every module (normalization, similarity tables from the brief, dedup keys, merge determinism, ranking explanations, cache TTL, circuit breaker, providers with spec-derived fixtures, resolver with wiremock pages and feeds, SSRF classification and redirect blocking in `uguisu-http`).
- Integration: `tests/e2e.rs` (three providers → merge → rank → resolve; outages; cache; streaming), `tests/properties.rs` (proptest), CLI binary tests, server router tests.
- Benchmarks: `crates/uguisu-discovery/benches/discovery.rs`, `crates/uguisu-http/benches/policy.rs`; results in [`benchmarks/`](benchmarks/).
- Live replay: `tests/live_fixtures.rs` serves the recordings from `tests/fixtures/discovery/live` through wiremock and asserts the real field mappings, the real autodiscovery markup (relative and absolute `<link>`s, oembed noise, three announced feeds, no links plus catch-all HTML), the recorded redirect chain and the self-referencing `itunes:new-feed-url`.
- Fixtures: `tests/fixtures/discovery/spec` (`origin: spec-example`, from documentation) and `tests/fixtures/discovery/live` (`origin: recorded`, from the live services on 2026-09-17, credentials stripped) are kept apart; `tests/fixtures/websites` and `tests/fixtures/feeds/probe` are hand-made. Tests never use the network.

---

*Status: Phase 2 as built and live-verified (2026-09-17). Open: Podcast Index authenticated responses (needs a user key); fyyd is after v1 (ADR 0005); see `research/DISCOVERY_ECOSYSTEM.md` §7.*
