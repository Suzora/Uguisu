# ADR 0030 — Discovery that outlives the process

**Status:** accepted · **Date:** 2026-09-21 · relates to [ADR 0001](0001-workspace-layout.md), [ADR 0005](0005-discovery-provider-strategy.md)

## Context

The provider cache lived in memory, so every restart re-asked Apple and Podcast Index the same questions. And nothing recorded *why* a feed was chosen: a user who wondered how a podcast ended up pointing at that URL had only the podcast.

## Decision

### The cache gets a second tier, behind a port

`uguisu-discovery` must not depend on storage (ADR 0001), so it declares what it needs — a `CacheStore` with `get` and `put` — and the engine implements it over `discovery_cache`. The same shape as the download queue's `EventSink`.

Memory stays in front and writes through; a miss reads the store, checks `expires_at` and hydrates memory, so the next call is local again. In memory an entry keeps a monotonic `Instant`, which a row cannot, so the port speaks wall-clock times and the cache converts.

**Every store operation may fail silently.** A cache that cannot be read is a miss; one that cannot be written is a provider call next time. Nothing here may ever make a search fail.

### Two things had to stop being `Debug` renderings

A lookup's cache key was `format!("{reference:?}")`. That is fine while the cache dies with the process and a silent mass invalidation the moment it outlives one — a tidy-up of a derive would quietly empty the cache. `ProviderRef::cache_key` writes it out; `StepKind::as_str` does the same for recorded provenance.

`CachedValue` is adjacently tagged (`{"kind": …, "value": …}`), not internally tagged: an internal tag cannot carry a variant whose payload is a sequence, and a search result is a list. With an internal tag every write would have failed serialisation at run time — silently, because a cache write that fails is not an error. The test that restarts and reads the value back is what found it.

`SCHEMA_VERSION` is part of the key, so a change to the candidate model makes older rows unreachable rather than mis-read.

### Resolutions are recorded, as provenance and nothing else

`discovery_records` keeps what each resolution decided and why — input, provider, the feed it chose, the steps it took, the warnings — on `podcast add` and on an explicit `resolve`, **including the ones that failed**: "why did it pick that feed" and "why did it refuse mine" are the same question asked at different moments.

**Nothing reads a record back as a feed source.** A search result does not become a podcast; `search`, `resolve` and `podcast add` stay three separate things. `podcast_id` is filled in only when the user went on to add one, and `ON DELETE SET NULL` keeps the record when that podcast is removed: the provenance outlives what it produced.

Records are written outside the caller's transaction. Provenance must never be able to fail an operation that otherwise succeeded, and a missing record is a gap in a list, not a broken library.

### Expiry is enforced on read, and swept by housekeeping

A row past `expires_at` is a miss whether or not anything deleted it. The daily maintenance pass deletes them so that a cache of searches nobody repeats does not grow for ever.

## Consequences

A restarted daemon answers repeat searches from disk. `GET /api/v1/discovery/records` and `uguisu` can explain how any podcast in the library was resolved.

The database grows by cached provider answers, bounded by their TTLs and swept daily.

## Alternatives considered

**Persisting the moka cache directly.** Its entries carry monotonic instants that mean nothing after a restart.

**Storing records inside the podcast row.** Loses the failures, which are the half people actually ask about.

**No second tier.** Cheapest, and it re-asks a third party the same question every time a container restarts — which ADR 0005's provider terms ask us not to do.
