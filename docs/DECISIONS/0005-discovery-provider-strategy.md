# ADR 0005 — Discovery provider strategy

**Status:** accepted, amended 2026-09-17 (Phase 2) and 2026-10-03 (Phase 10) · **Date:** 2026-09-17

## Context

See `docs/research/DISCOVERY_ECOSYSTEM.md`. No single directory is complete, none offers explainable fuzzy ranking, some forbid caching, and Uguisu must never make existing subscriptions depend on a provider.

## Decision

- Providers implement one trait (`DiscoveryProvider`) and are registered with a trust weight, a credential requirement and rate limits; results are normalized to `PodcastCandidate` before leaving the provider module.
- **v1 providers:** Apple iTunes Search (default on, keyless), URL/website resolver (default on), Podcast Index (on when the user supplies their own key/secret), gpodder.net directory (opt-in).
- **After v1:** fyyd (see the amendments below).
- **Excluded from v1:** Podchaser, Listen Notes, Taddy (paid tiers and caching restrictions), Spotify and YouTube (no feeds).
- Uguisu never ships shared credentials. Keys are never stored: both Podcast Index keys live only in the environment ([ADR 0028](0028-persisted-settings-and-precedence.md), `SECURITY.md` §3.7).
- Fuzzy ranking, deduplication and explanations are Uguisu's, not the providers'.
- Providers are wrapped with rate limiter, retry/backoff, circuit breaker and a TTL cache (memory + `discovery_cache` table); failures are reported per provider.

## Consequences

- Works out of the box (Apple + resolver); improves with a free Podcast Index key.
- Provider outages degrade search, never archival.
- Every provider needs recorded fixtures because CI cannot call the network; a fixture-recording tool is part of Phase 2.

## Alternatives considered

- **Podcast Index only:** best data, but requires every user to register a key before the first search.
- **Apple only (Podgrab's approach):** fine for mainstream shows, weak for regional and independent podcasts, no GUIDs or health flags.
- **Running our own index:** out of scope; the resolver plus multiple directories covers the gap.

## Amendment 2026-09-17 (Phase 2)

- **fyyd is deferred.** Its primary documentation was unreachable from the Phase 2 environment; the provider is not implemented and the abstraction stays ready (`docs/research/DISCOVERY_ECOSYSTEM.md` §2.3).
- **Podcast Index cache rule.** Cache entries expire at `min(configured TTL, response Cache-Control max-age)` (ToS §5); `ProviderResponse.cache_max_age` carries the hint and the registry applies it.
- **Attribution.** `ProviderInfo.attribution` is carried through `SearchResponse.attribution`, the CLI footer and the web UI's Discover view (ToS §7).
- **Fail-fast provider retries.** Provider HTTP calls retry once with a short backoff and treat a `Retry-After` longer than two seconds as an immediate `RateLimited`; the registry's circuit breaker then backs off using that value.
- **Verification status (Part A).** Providers were implemented strictly against primary sources (Apple documentation, the Podcast Index OpenAPI specification and Terms of Service, the mygpo directory documentation) but could not be called live; fixtures were labelled `spec-example`.

## Amendment 2026-09-17 (Phase 2, Part B — live verification)

- **Apple and gpodder.net are live-verified**; their recorded responses (`tests/fixtures/discovery/live`) are replayed in tests next to, not instead of, the spec-derived fixtures (`tests/fixtures/discovery/spec`). The two provenances stay separate so that a re-recording exposes schema drift.
- **Podcast Index is verified only up to authentication** (401 shapes recorded; plain-text bodies under `application/json`). Authenticated responses remain spec-validated until a user key is available. Its `no-cache` answers led to a rule: a `Cache-Control` with `no-cache`/`no-store` yields a zero TTL and is never cached (ToS §5).
- **fyyd stays deferred.** The API answers and its live schema is recorded for reference, but the primary documentation is still unreachable, so no provider is built on it.
- **No provider is fuzzy.** Apple and gpodder.net return nothing for a misspelled title; Uguisu's typo-tolerant ranking therefore improves ordering, not recall. Provider-side tolerance exists only as Podcast Index `similar=true` (unverified). Query relaxation is deferred to a later phase.
- **Rate limits.** Apple did not throttle 45 requests in 20 s; the documented 20/min stays as Uguisu's own limit. gpodder.net publishes no subscriber counts (all 0), so its popularity signal is currently inert.

## Amendment 2026-10-03 (Phase 10)

- **fyyd moves after v1.** Its primary documentation was never reachable, and a provider built on one observed response would be the only one verified neither against a specification nor by a set of recordings. The live schema stays recorded (`tests/fixtures/discovery/live/fyyd`), `ProviderId::FYYD` and the resolver's classification of `fyyd.de` pages stay, and a fyyd page keeps answering `provider_unavailable`. v1 ships Apple, the resolver, Podcast Index and gpodder.net.
- **Misspelling recall by query relaxation.** When every asked provider answers a term query successfully with nothing, the engine asks the same providers one more round with at most two looser queries — the folded words without the last and without the first, or a single word of four or more characters without its last character — and ranks what comes back against the query as typed, so the existing Jaro-Winkler signal puts `Darknet Diaries` first for `darknet diariez`. `SearchResponse.relaxed` names each looser query with its per-provider outcomes; `providers` and `outcome` keep describing the query as typed, so the four outcomes keep their meaning. No relaxation after a failure, a timeout or an open breaker (none is evidence of a typo), after any result, or for a URL or an empty query. The round goes through the same cache, throttle, circuit breaker and hard deadline: with Apple's 20/min limit and no burst, its two relaxed calls wait about 3 s and 6 s, so a relaxed search can take most of the hard deadline, and a call still waiting at it is reported `timed_out`. Rejected: Podcast Index `similar=true` (unverified, needs the user's key, and Apple and gpodder.net stay strict); a local spelling dictionary (a dependency, and podcast titles are mostly names); relaxing every query (doubles the provider load for searches that already found something).
