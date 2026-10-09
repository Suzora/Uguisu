# The web UI

Uguisu's single-page application: Svelte 5 with runes, TypeScript, Vite, and no runtime dependency beyond the browser. It talks to the same `/api/v1` the CLI talks to, holds no rules of its own, and is not required — every action it offers has a CLI equivalent (`docs/CLI.md`).

Design decisions live in [ADR 0031](DECISIONS/0031-serving-the-web-ui.md) (serving), [ADR 0032](DECISIONS/0032-media-and-artwork-over-http.md) (media and artwork), [ADR 0033](DECISIONS/0033-sse-reconnection.md) (the event stream) and [ADR 0034](DECISIONS/0034-web-ui-architecture.md) (layers and the typed adapter). This document says what exists and how to work on it.

## Running it

```
pnpm --dir web install          # once
pnpm --dir web build            # writes web/dist
cargo run -p uguisu-cli -- serve
```

`uguisu serve` serves `web/dist` by default, relative to the working directory, and logs the directory at `info`. `--web <dir>` or `UGUISU_WEB_DIR` points it elsewhere; a directory that is not there means the API runs alone and the UI answers `404`.

For UI work, run Vite instead and leave the API where it is:

```
cargo run -p uguisu-cli -- serve     # terminal 1, the API on 127.0.0.1:8484
pnpm --dir web dev                   # terminal 2, the UI on 127.0.0.1:5173
```

The dev server proxies `/api` to `127.0.0.1:8484`, so the UI is same-origin in both modes and needs no CORS. `pnpm --dir web check` type-checks, `pnpm --dir web test` runs the suite.

## Pages

| Path | What it is for |
|---|---|
| `/` | what the library, the queue, the scheduler and the index are doing now, plus recent activity |
| `/podcasts` | the library, filtered by title and status and sorted by the server, a page at a time (ADR 0057); exports it as OPML |
| `/podcasts/{id}` | one podcast: metadata, schedule, archive policy, actions — archiving it and removing it from the library among them (ADR 0055) — and its episodes; a candidate duplicate is merged or kept there (ADR 0051) |
| `/episodes/{id}` | one episode: its dates and show notes, its archive record and its download job, and the player when it is archived |
| `/discover` | search the directories or resolve a URL, then add; or plan an OPML file and add its new feeds (ADR 0049) |
| `/search` | local FTS5 search over podcasts and episodes |
| `/downloads` | the queue, with live progress, per-job and global controls |
| `/archive` | what is on disk, its verification state, and verification |
| `/archive/import` | plan an import of another tool's archive from a folder on the server, then copy what it can place ([ADR 0059](DECISIONS/0059-archive-import-in-the-web-ui.md)) |
| `/archive/repair` | the files a check found missing: put their exact bytes back from a folder on the server, or download them again ([ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)) |
| `/settings` | persisted settings with their origin, and what the environment pins |
| `/service` | the daemon: scheduler, workers, index, and the controls for them |

Filters, sort, search text and the selected state are in the query string, so a refresh restores the view and a link shares it. Back and Forward work.

## Layers

```
lib/api/generated/schema.d.ts   generated from docs/api/openapi.json — never edited
lib/api/types.ts                the same names, re-exported from the schema
lib/api/client.ts               one fetch, one deadline, one error taxonomy, the CSRF token
lib/api/index.ts                one function per API operation
lib/router.ts                   path ↔ route, as pure functions
lib/*.svelte.ts                 the three pieces of global state, and one request's lifecycle (resource)
lib/i18n/                       the interface's text: en/ is the catalogue, one file per area
views/, lib/components/         rendering and page-local state
```

A component never calls `fetch`, never builds a URL and never reads a status code, and never writes interface text of its own.

## Text and language

Every piece of text the interface shows comes from the catalogue in `lib/i18n/en/` ([ADR 0053](DECISIONS/0053-message-catalogue.md)): a view writes `{m.library.empty}`, and text that carries a value or a count is a function, with plural forms chosen by `Intl.PluralRules`. Dates, times and numbers follow the catalogue's locale, not the browser's. A state, kind or reason the API sends is shown through `label()`, which uses the catalogue's word for it when there is one.

What the server and the feeds say is shown as sent: error messages, verification and check reasons, titles and show notes. Only English ships. Another language is a copy of `en/` that satisfies its type — the compiler names every missing entry — and a choice at start-up, which is built with the second locale. `src/lib/i18n/literal-text.test.ts` fails when a Svelte file writes text into its markup, a read attribute, a component's text prop, or prose into a script.

**The shapes are generated** (ADR 0039). `pnpm api:types` regenerates `schema.d.ts` from the OpenAPI document; `pnpm api:types:check` — and `scripts/check.py api-types`, in the default set and in CI — fails when the committed file is not what the document produces. `types.ts` re-exports them under the names the views already used, so a wire change is a compile error and never a silent one.

**Errors are a closed set.** `ApiFailure.failure` is one of:

| `failure` | What happened | What the UI says |
|---|---|---|
| `network` | the request never reached a server | is `uguisu serve` running? |
| `timeout` | no answer inside the deadline | the request was given up on |
| `cancelled` | a newer request superseded this one | nothing; it is not an error |
| `http` | the server rejected it (`4xx`/`5xx`) | the server's own message, with its `kind` |
| `unauthorized` | `401`: no credential, or one that expired | nothing — the shell shows the login form instead |
| `forbidden` | `403`: a `read` token at a mutation, or a missing CSRF header | the server's own message |
| `unavailable` | `503`, e.g. a server with no library | the server's own message |
| `malformed` | a success that was not the expected JSON | the response was not usable |

`status` and `kind` carry the HTTP status and the server's stable error kind, so a view can single out a conflict (`409`, a pinned setting or a refused transition) from a rejection. `retryAfterSeconds` carries a `429`'s wait, which the login form shows.

**A 401 is the shell's business, not a view's.** `client.ts` takes one handler, registered by `App.svelte`; a session that expires mid-visit puts every view behind the login form rather than each view rendering its own refusal. The `/auth/` routes are exempt, because a wrong password is the form's own message.

**State that outlives a view** is exactly three things: the event stream, the current location, and what the player is playing. There is no store mirroring the library and no client-side cache of episodes — the server is authoritative, and a mutation is followed by a re-read.

## The event stream

One `EventSource` per tab, opened by the shell, shared by every view (ADR 0033). The header shows `connecting`, `live` or `reconnecting`.

A view subscribes to the kinds it cares about and registers `onResync`. **Every successful connect runs the resync callbacks**, because `download.progress` is transient: it is never stored, so an event missed while the stream was down is missed for good and nothing in the data shows the gap. Re-reading the API is the only correct response. Reconnection backs off from 500 ms to a 30 s ceiling.

Events are an overlay. A progress message updates the bytes on a row the API supplied; a state change triggers a re-read. Nothing is reconstructed from a sequence of events.

## Media and artwork

Audio comes from `GET /api/v1/archive/{episode_id}/media`, which answers `Range` so `<audio>` can seek. Artwork comes from `GET /api/v1/podcasts/{id}/artwork/image` when Uguisu has fetched it, and falls back to the feed's URL when it has not. The player is the browser's own `<audio controls>`: play, pause, seek, volume, keyboard and mobile handling are already there and already accessible.

Both endpoints take an id, never a path. See ADR 0032 for what they refuse and why.

## Authentication

The shell asks `GET /api/v1/auth/session` on boot. When the server needs a credential and this request does not carry one, it renders the login form in place of the navigation, every view and the event stream — an authenticated read is not attempted before there is something to authenticate with.

- **The password is in the form and nowhere else.** No URL, no `localStorage`, no query parameter. What comes back is a cookie the browser holds (`HttpOnly`, so script cannot read it) and a CSRF token the client keeps in a module variable.
- **`x-uguisu-csrf` rides every non-GET request**, because a cookie alone must not be able to change anything (ADR 0036). There is no automatic retry on a 403: the view shows the server's message. Only a 401 sends the shell back to the session and the login form.
- **Cookies need no `credentials:` option.** Same-origin `fetch`, `EventSource`, `<audio src>` and `<img src>` all send them by default, which is the whole reason the browser's credential is a cookie.
- **Settings has a password panel**, not a forced wizard: authentication is optional on loopback, so setting one is an action the operator takes rather than a gate they pass. The two `UGUISU_AUTH_*` knobs are deployment settings and never appear in the settings list.
- **Signing out** revokes the session server-side and clears the cookie.

## Security

The UI adds no authorization of its own — the server decides, and the UI renders what it is told (`docs/SECURITY.md` §3.5).

What the UI is responsible for:

- **Feed text is text.** Titles, subtitles and show notes are rendered from `description_text`, never `description_html`. There is no `{@html}` anywhere in the tree, so a podcast cannot inject markup or script into the page.
- **No filesystem path is constructed in the browser.** Media and artwork are requested by id; a `relative_path` is shown as information, never used to build a URL. The only paths the browser sends are folders the operator types on the import and repair pages, carried unchanged in a POST body and never in a URL; there is no endpoint that lists the server's directories ([ADR 0059](DECISIONS/0059-archive-import-in-the-web-ui.md), [ADR 0060](DECISIONS/0060-repairing-a-missing-file.md)).
- **Artwork prefers Uguisu's own copy**, so opening the UI does not announce the library to every publisher's CDN. The feed URL is a fallback for a podcast whose artwork has not been fetched.
- **`nosniff` and canonical content types** on everything the server serves, so an archived HTML error page cannot execute as same-origin script (ADR 0032).
- **A CSP whose every executable directive is `'self'`**, with no `unsafe-inline` — the built page has no inline script or style, so it needs none. `img-src` is the one open directive: a podcast whose artwork Uguisu has not fetched falls back to the URL its feed published, and an image cannot execute.

## Tests

```
pnpm --dir web test        # jsdom + @testing-library/svelte
python3 scripts/e2e.py     # the built UI and the shipped binary, over HTTP
```

The frontend suite covers the API layer's error taxonomy, the router, the formatters, the event stream's lifecycle (one connection, backoff, resync, no duplicate delivery), and each view's loading, empty, error and refused-mutation paths against a stubbed API. `src/tests/accessibility.test.ts` renders all twelve pages and asserts one `h1` each, no skipped heading level, a label on every control, an accessible name on every button, captions and column scopes on tables, no clickable `div`, and the page-change announcement.

`scripts/e2e.py` starts a local publisher and the real binary and walks the whole journey three times over: without a credential on loopback (the SPA is served, a deep link survives a refresh, a feed is resolved and added, both episodes download and archive, the archived bytes come back with working range requests and a validator, the stored artwork is reachable, local search finds the episode, an environment-pinned setting is refused with a conflict, a stop signal — `SIGTERM`, or `CTRL_BREAK_EVENT` on Windows — exits cleanly); then behind a password, with a cookie, a CSRF header and tokens; then against a network bind with no credential, which refuses to start.

Neither is a browser test. There is no browser automation dependency in the repository, by design (§34 of the Phase-8 brief): a real-Chromium pass was run by hand during Phase 8 and is recorded in [`docs/benchmarks/2026-09-21-phase8-verification.md`](benchmarks/2026-09-21-phase8-verification.md), alongside the [measurements](benchmarks/2026-09-21-phase8.md).

## Known limitations

- **Speed and ETA only appear while a job is running in the process that is watching.** They are not on the job row; they arrive as `download.progress` events, so a job running in another process shows bytes but no rate.
- **Artwork costs one request per podcast** on the library page, after the list renders. A podcast without stored artwork simply keeps the feed's image.
- **One operator, no accounts.** There is a password and there are API tokens; there are no users, no roles and no registration. Once a password is set, Settings lists, creates and revokes tokens, as `uguisu auth token` does.
