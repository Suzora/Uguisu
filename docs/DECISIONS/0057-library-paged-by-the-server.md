# ADR 0057 — The library list is filtered, sorted and paged by the server

**Status:** accepted, amends [ADR 0040](0040-one-cursor-contract.md), amended 2026-10-10 · **Date:** 2026-10-03

## Context

ADR 0040 kept `?q=` off `GET /api/v1/podcasts`, reasoning that a substring filter would be an unindexed scan duplicating the FTS-backed search (ADR 0029). The web library then read every page and filtered and sorted in the browser, including two sorts — last refresh and episode count — that the server did not offer. A large library paid for the whole list on every visit, and the UI had two paging models.

## Decision

**`q=` filters by title.** A case-insensitive substring of the title or the sort title, at most 200 characters, `400 invalid` above that. It scans `podcasts`: no index answers a substring, and the table holds one row per show — hundreds, not the forty thousand episodes FTS exists for. It is not a search: it does not rank, tokenise or look at episodes, so it does not duplicate `GET /api/v1/search`.

**`sort=refreshed` and `sort=episodes`.** `refreshed` orders by `last_refresh_at` (the last refresh, whatever its outcome) newest first, never refreshed last; `episodes` by the stored episode count, highest first. Both break ties by id and keep ADR 0040's cursor: the cursor is a podcast id whose sort key is read when the page is.

**No new index.** Both sorts read the filtered rows and sort them. A migration for a list of hundreds of rows costs more than the sort. *(Amended 2026-10-10: `sort=episodes` reads `podcasts.episode_count`, indexed with id and kept by an insert and a delete trigger on `episodes` (migration 0009). Counting each podcast's episodes for every page took 78 ms at 10 000 podcasts ([Phase 11 benchmark](../benchmarks/2026-10-10-phase11.md)).)*

**A cursor outside the title filter is `400 invalid`,** as one outside the status filter already is.

## Consequences

- The web library fetches one page at a time with `Load more`, like the archive view.
- A sort key that changes while a client walks the list — a refresh finishing, an episode arriving — can move that podcast past the cursor or back before it. Title and added order cannot change that way; these two can, and the list is a view, not a snapshot.

## Alternatives considered

- **Filtering in the browser.** That is what this replaces: it needs the whole library first.
- **Indexes on `last_refresh_at` and a stored episode count.** A migration and a trigger-maintained counter for a list that is small by construction. *(The stored count was added on 2026-10-10, above; `last_refresh_at` still has no index, and `sort=refreshed` took 5.6 ms at 10 000 podcasts.)*
- **Sending library filtering through `GET /api/v1/search`.** Search answers episodes and podcasts ranked by relevance; the library needs podcasts in a chosen order with counters.
