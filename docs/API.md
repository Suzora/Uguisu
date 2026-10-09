# The HTTP API

Everything Uguisu does is reachable over HTTP under `/api/v1`. The CLI uses the same API in `--server` mode (ADR 0003), and the web UI uses nothing else.

The machine-readable contract is [`api/openapi.json`](api/openapi.json): 85 paths, 96 operations, 209 schemas, generated from the Rust types and checked against the router in CI (ADR 0039). This page is the part a document cannot say — the rules that hold across every route.

| For | Read |
|---|---|
| Every route, parameter and body | [`api/openapi.json`](api/openapi.json) |
| Running it behind a proxy, and binding | [`DEPLOYMENT.md`](DEPLOYMENT.md) |
| The threat model and the controls | [`SECURITY.md`](SECURITY.md) §3 |
| The same operations from a terminal | [`CLI.md`](CLI.md) |

## Versioning

`v1` is in the path, and every success body carries `schema: 1`. Within `v1` a field may be **added**; nothing is removed or retyped, and no status or `error.kind` changes meaning. A change that cannot be made that way is `v2` on a new path prefix.

Three bodies carry their own `schema` instead of the API's, because they are versioned documents in their own right and are written to disk that way: a feed inspection, a refresh report and a sidecar.

## Authentication

Authentication is **off until a password is set** and required once one is (ADR 0035). `GET /api/v1/auth/session` answers the three questions a client needs before anything else, and needs no credential itself:

```json
{ "schema": 1, "auth_required": true, "credential_set": true, "authenticated": false }
```

Two credentials, for two kinds of client:

| | Cookie | Bearer token |
|---|---|---|
| Who | a browser | the CLI, automation |
| Obtained by | `POST /api/v1/auth/login` | `POST /api/v1/auth/tokens`, or `uguisu auth token create` |
| Sent as | `Cookie: uguisu_session=…` (automatic) | `Authorization: Bearer …` |
| Mutations need | `X-Uguisu-CSRF: <the session's token>` | nothing |
| Scopes | full | `read` or `write` |

A browser gets a cookie because `<audio src>`, `<img src>` and `EventSource` cannot attach a header, and **a credential is never accepted in a URL** — there is no query parameter for one, on any route.

### Three access levels

| Level | What | Refused with |
|---|---|---|
| **Public** | `GET /health`, `GET /auth/session`, `POST /auth/login`, and `POST /auth/password` while no credential exists | — |
| **Read** | every `GET` and `HEAD`, including media bytes, artwork bytes and both modes of `/events` | 401 `unauthenticated` |
| **Mutate** | everything else | 401, or 403 `forbidden` for a `read` token, or 403 `csrf_required` for a cookie with no CSRF header |

Whatever the class, a request that is not `GET`, `HEAD` or `OPTIONS` and whose `Origin` names another site is 403 `forbidden` before any of this, with or without a password ([ADR 0058](DECISIONS/0058-refusing-cross-site-changes.md)). Clients that send no `Origin`, such as the CLI and curl, are not affected.

Anything not on the public list is authenticated, **including a path that does not exist**: an anonymous request to an unknown `/api/` path answers 401, not 404, so a route table cannot be enumerated (ADR 0036). A 401 never carries `WWW-Authenticate`, on purpose: a browser that saw a challenge would offer its own login dialog and then send `Authorization` by itself, and the web UI authenticates with its session cookie alone.

### Rate limiting

Only `POST /api/v1/auth/login` is rate-limited: ten failures per peer address per five minutes, plus a global hundred, answered as 429 `too_many_requests` with `Retry-After` in seconds. A success clears the bucket. Every other route is authenticated, and the existing concurrency caps plus axum's 2 MB body limit bound what a credentialed client can do.

The bucket is the client: the socket peer, or, behind a proxy named with `--trusted-proxy`, the address its `X-Forwarded-For` names ([ADR 0062](DECISIONS/0062-trusted-proxies.md)). With no proxy named, every request behind a reverse proxy shares one bucket and the global cap is what covers a distributed attempt ([`DEPLOYMENT.md`](DEPLOYMENT.md)).

### CORS

There is none. No response carries an `Access-Control-*` header, and no `OPTIONS` request is answered as a preflight: on an API route it is refused in the standard envelope, 405 with authentication off and 401 `unauthenticated` once a password is set, since a preflight carries no credential. The SPA is same-origin by construction (ADR 0031) and `pnpm dev` uses Vite's proxy. A cross-origin client would have to be given a bearer token deliberately, which is what tokens are for.

## Failures

Every failure, including a rejected body and a wrong method, is one shape (ADR 0038):

```json
{ "schema": 1, "error": { "kind": "hash_mismatch", "message": "…" } }
```

`kind` is stable; branch on it rather than on the message. The message names what a person needs to act, and never an absolute path, a SQL statement, a stack trace or a credential.

| Kind | Status | Means |
|---|---|---|
| `invalid` | 400, or 422 for a JSON body that parses but does not fit the route's shape | the request is wrong: a bad id, an unknown enum value, `limit=0`, an unparseable cursor, a body that is not JSON |
| `unsupported_media_type` | 415 | a JSON body without `Content-Type: application/json` |
| `payload_too_large` | 413 | over axum's 2 MB body limit |
| `method_not_allowed` | 405 | the path exists, the method does not |
| `unauthenticated` | 401 | no credential, or one that no longer resolves |
| `forbidden` | 403 | a `read` token at a mutation, the wrong `current_password`, or a change sent from another site's page |
| `csrf_required` | 403 | a cookie mutation with no `X-Uguisu-CSRF` |
| `too_many_requests` | 429 | the login limiter; see `Retry-After` |
| `not_found` | 404 | no such row |
| `conflict` | 409 | the state does not allow it, or the environment pins the setting |
| `blocked_by_policy` | 403 | the SSRF policy refused the address |
| `unresolvable` | 422 | `POST /podcasts` found nothing usable at that input |
| `network`, `feed` | 502 | the publisher could not be fetched or parsed |
| `cancelled` | 408 | the operation was cancelled |
| `disk_full` | 507 | no room for the file |
| `locked` | 503 | another process holds the data directory |
| `engine_unavailable` | 503 | this server runs without a library |
| `storage`, `config`, `io`, `internal` | 500 | Uguisu's fault |
| the archive kinds | 404, 400, 409, 422 or 500 | one of the 21 archive kinds, e.g. `archive_not_found`, `hash_mismatch`, `path_collision`, `sidecar_missing`, `import_ambiguous` |
| `artwork_not_found` | 404 | the podcast has no stored artwork |

### The one deliberate divergence

`POST /api/v1/discovery/resolve` adds two keys inside the same envelope, because it is the only route whose failure is a story rather than a fact:

```json
{
  "schema": 1,
  "error": { "kind": "no_feed_link_found", "message": "…", "suggestion": "…", "detail": { … } },
  "provenance": [ { "step": "…" } ]
}
```

`POST /api/v1/podcasts` reports the same underlying failures as a single `unresolvable` (422). That is intentional: "tell me what this is" needs the steps it took, while "add this" needs one answer.

## Pagination

Seven routes page: `/podcasts`, `/podcasts/{id}/episodes`, `/episodes/duplicates`, `/downloads`, `/archive` and the `/archive/missing` and `/archive/invalid` views. All seven behave the same way (ADR 0040):

| | |
|---|---|
| Request | `?after=<last id of the previous page>&limit=<1..=500>` |
| Answer | the rows, plus `next_after` |
| `next_after` | present **iff** a further row exists — never a trailing empty page |
| `limit` | default 50, maximum 500; `0` or anything above 500 is 400 `invalid` |
| An unknown `after` | 400 `invalid` |
| An `after` outside the request's own filter | 400 `invalid` |

`GET /api/v1/podcasts` also takes (ADR 0057):

| Parameter | Meaning |
|---|---|
| `status=active\|paused\|error\|archived` | only podcasts in that status |
| `q=` | only podcasts whose title or sort title contains it, case-insensitively; at most 200 characters, blank means no filter |
| `sort=title` (default) | sort title, then id |
| `sort=added` | newest first |
| `sort=refreshed` | last refresh, whatever its outcome, newest first; never refreshed last; ties by id |
| `sort=episodes` | most episodes first; ties by id |

A cursor is a podcast id under every sort. Under `refreshed` and `episodes` a podcast whose key changes while a client pages — a refresh, a new episode — can move across the cursor.

## Events

`GET /api/v1/events` is two endpoints behind one path:

- **With `after` or `limit`** it answers a JSON list of stored events, oldest first: those after `after`, or with `limit` alone the newest `limit`.
- **Without either** it is a `text/event-stream` of live events, named by kind, with the event id in `id:`. Nothing reads `Last-Event-ID`: a reconnect starts from now, and `?after=<that id>` lists what it missed except `download.progress`, which is never stored. `?exclude=download.progress` drops the noisy kinds.

The stream is a **notification**, not a second source of truth (ADR 0033): an event says something happened, and the client re-reads what it needs. It is a `Read`, authenticated by cookie or bearer like any other.

## Bytes

`GET /api/v1/archive/{episode_id}/media` and `GET /api/v1/podcasts/{id}/artwork/image` stream from the media root (ADR 0032): `Accept-Ranges: bytes`, 206 with an exact `Content-Range`, 416 with `bytes */<size>` past the end, strong `ETag` from the stored hash and 304 on a match. `If-Range` is honoured. Both are `Read`; the path safety in front of them — no escape from the media root, no `.uguisu`, no sidecar served as audio — is unchanged and unchangeable by any row in the database.

`GET /api/v1/podcasts/opml` answers the library as an OPML file (`text/x-opml`, `Content-Disposition: attachment`), not a JSON envelope; importing one is `POST` to the same path with the document as a JSON string (ADR 0049).

## Settings

`GET /api/v1/settings` reports every key with the value in force and where it came from. A value the environment or the command line supplies wins over anything stored, and writing such a key answers 409 `conflict` rather than being silently ignored (ADR 0028). Secret values are redacted in `value`, in `stored`, and in the rejected and unused lists — a setting marked secret is never echoed, not even back to the operator who set it.

Keys are the `UGUISU_*` variable names.

Two settings are deliberately **not** in this list: `UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE` and `UGUISU_AUTH_COOKIE_SECURE` are deployment knobs, and a database row that permits insecure exposure could be set through the API it protects (ADR 0037).
