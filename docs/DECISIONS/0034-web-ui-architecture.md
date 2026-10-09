# ADR 0034 — The web UI's layers, and a typed adapter before a generated client

**Status:** accepted, amends [ADR 0004](0004-web-ui-and-desktop-stack.md), amended by [ADR 0039](0039-openapi-from-the-types.md) · **Date:** 2026-09-21

## Context

ADR 0004 said the SPA would use "an API client generated from the OpenAPI document". There is no OpenAPI document: it was chartered for Phase 7 and moved out with that phase's re-scope, and Phase 8 is explicitly not the phase that adds it. Meanwhile Phase 8 needs to call about forty routes from nine views.

The Phase-7 scaffold showed what happens without a boundary: `status.ts` and `api.ts` each declared their own `ServiceStatus`, each had its own `fetch` with its own error handling, and a component read `response.ok` directly. Two shapes for one Rust struct is one shape too many.

## Decision

**Four layers, in one direction.**

```
lib/api/client.ts     transport: one fetch, one deadline, one error taxonomy
lib/api/types.ts      the shapes the Rust API serialises
lib/api/index.ts      one function per operation
views/, components/   rendering and page-local state
```

A component never calls `fetch`, never builds a URL and never reads a status code. `index.ts` is deliberately flat and mechanical — one exported function per route, no clever grouping — because that is the shape a generated client has, so the day one arrives it replaces that file and the views do not move.

**Failures are a closed set, not a message.** `ApiFailure.failure` is one of `network`, `timeout`, `cancelled`, `http`, `unavailable`, `malformed`. A view renders them differently because they mean different things: `unavailable` is a server without a library, `network` is a server that is not there, `malformed` is a server that answered something else. Nothing is swallowed: every non-success path throws, so no caller can mistake an error for data.

**The URL is the state that matters.** `router.ts` maps a path to a route and back as pure functions; query parameters carry the filters, the sort, the search text and the selected state. A refresh, a shared link and Back all work because there is nothing else to restore. Typing in a filter replaces the history entry; following a link pushes one.

**Global state is three things.** The event stream (ADR 0033), the current location, and what the player is playing — each because it must outlive a view. Everything else is page-local. There is no store mirroring the library, no client-side cache of episodes, no normalised entity map.

**Every mutation ends in an authoritative re-read.** Action → request → success or failure → re-read or event-driven refresh. Nothing is optimistically mutated, so the UI cannot show a state the server refused; a `409` shows what the server said and reloads the real state.

**The backend decides; the browser renders.** State names, ranking, policy evaluation, settings precedence and validation all stay in Rust. Two places bend this, and both are recorded rather than hidden: the podcast list is filtered and sorted in the browser because `GET /api/v1/podcasts` takes no parameters and has no cursor, and the queue joins job rows against the library because job rows carry ids and not titles. Neither computes anything the backend decides; both are listed in `docs/WEB_UI.md` as the places a backend change would simplify.

**Untrusted text is text.** Podcast titles, episode titles and show notes come from feeds. They are rendered as text — `description_text`, never `description_html`, and no `{@html}` anywhere in the tree. Artwork prefers Uguisu's own bytes (ADR 0032) over the publisher's URL.

**Zero runtime dependencies.** Svelte 5 runes, the History API, `fetch`, `EventSource` and `<audio controls>` cover everything the UI needs; the built bundle is about 134 kB, 42 kB compressed. A router, a query library and a design system were each considered and each would have been more code than the thing it replaced. Testing adds jsdom and `@testing-library/svelte` as development dependencies only.

## Consequences

- A future generated client replaces `lib/api/index.ts` and `types.ts`; `client.ts` keeps the error taxonomy the views branch on, and no view changes. **This happened in Phase 9 and cost less than predicted** (ADR 0039): the document is derived from the Rust types, `types.ts` became 89 lines of re-exports under the same names, `index.ts` did not move because it was already shaped like a generated client, and `client.ts` kept the taxonomy and gained `unauthorized`, `forbidden` and `retryAfterSeconds`. Five call sites changed, all because a generated optional field is `T | null | undefined` where the hand-written one said `T | null`.
- Adding a route is one function, and the compiler finds every caller of a shape that changed.
- The browser holds no copy of the database, so a stale view is a re-read away rather than a cache-invalidation problem.
- Filtering a very large library in the browser is the known ceiling. It is bounded by the response the API already sends whole, so the fix is a paginated, filterable `GET /api/v1/podcasts` — a backend change, deliberately not smuggled into Phase 8. **Phase 9 made that change** (ADR 0040): the route takes `status`, `sort`, `after` and `limit`, and the queue's job rows now carry their own titles, so both places this ADR listed as "where a backend change would simplify" are gone.

## Alternatives considered

- **Adding OpenAPI now and generating the client.** It is a Phase-8 non-goal, and generating from a document that does not exist yet would have meant writing the document first — a larger and separate piece of work.
- **A router library (svelte-routing, tinro).** Nine routes, one of them parameterised. The typed `parsePath`/`pathFor` pair is smaller than the dependency and exhaustively checked by the compiler.
- **TanStack Query or similar.** Its value is caching and invalidation across a large surface; here the event stream already drives refreshes and there is no cache to invalidate.
- **A shared store per entity.** That is a client-side mirror of SQLite, which is the thing this ADR exists to avoid.
