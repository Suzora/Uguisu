# ADR 0040 — One cursor contract for every page

**Status:** accepted, amended by [ADR 0057](0057-library-paged-by-the-server.md) · **Date:** 2026-09-22

## Context

Phase 9 adds cursors to the podcast list and the archive list, joining the two that already existed for episodes and download jobs. Those two disagreed with each other in three ways, and each was defensible alone: `next_after` was set whenever a page was full, so an exact-multiple last page advertised a page that did not exist; an unknown `?after=` was a 404 on the download queue and a 400 elsewhere; and a cursor naming a row outside the request's own filter was silently accepted, so `?podcast=A&after=<id in B>` paged through the wrong parent.

Four pagers with three disagreements become five pagers with four, unless the contract is written down once.

## Decision

**`limit + 1`, then truncate.** The engine asks for one row more than the page and drops it, so `next_after` exists **iff** a further row does. Storage keeps `limit` meaning what it says; only the engine and the service probe.

**An unknown cursor is 400 `invalid`.** A vanished `?after=` is a bad request, not a missing collection. `DownloadService::list` stops answering 404.

**A cursor outside the request's filter is 400 `invalid`.** The cursor row is resolved and checked against `?podcast=` and `?state=` before it is used, so a cursor cannot cross into another parent.

**`limit` is validated, then clamped.** `0` is 400 rather than silently meaning 1; above the maximum clamps. The default is 50, the maximum 500, and the clamp lives in `uguisu_core::page` rather than in each engine.

**`next_after` keeps its name and stays top-level.** Wrapping pages in `page: { … }` would touch three CLI renderers, their tests and the web for nothing.

## Consequences

- All five pagers — podcasts, episodes, download jobs, archive files and the archive's `missing`/`invalid` views — answer the same shape, and `crates/uguisu-server/tests/pagination.rs` covers `limit=0`, `limit=abc`, an unknown cursor and a cross-parent cursor on each.
- A client can stop when `next_after` is absent, without a trailing empty page.
- Keyset paging needs an index that resolves the tie-break: `sort=title` pages on `(sort_title, id)` with a new index, while `sort=added` pages on `id DESC` alone, because ULIDs are time-ordered and need neither an index nor a column.
- `?q=` was kept off the podcast list as an unindexed scan duplicating the FTS-backed `GET /api/v1/search` (ADR 0029); ADR 0057 adds it as a title filter.

## Alternatives considered

- **Offset paging.** Cheap to write and wrong under concurrent inserts, which a refresh does constantly.
- **An opaque encoded cursor.** It hides the sort key from the operator and the logs, and buys nothing on a single-node API where the key is already a public id.
- **Returning `next_after` whenever the page is full.** That is the false positive this ADR removes.
- **Leaving the download queue's 404.** Two meanings for one client mistake.
