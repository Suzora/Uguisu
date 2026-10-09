# ADR 0036 — Three access levels, classified in one place

**Status:** accepted, supersedes in part [ADR 0013](0013-auth-and-network-binding.md) · **Date:** 2026-09-22

## Context

The API has 83 operations across seven modules. Deciding "does this need a credential" per handler means 83 decisions, 83 chances to forget one, and a 84th route next phase that nobody classifies. Roles would mean a matrix, and nobody asked for one: there is a single operator.

## Decision

**Three levels, and no roles.** `Public`, `Read`, `Mutate`. A `read` token at a `Mutate` route is 403; there is no third thing a scope could mean.

**One classifier, default deny.** `access_of(method, path, credential_set)` decides, in order:

1. A path outside `/api/` is `Public` — the SPA shell and its assets, which carry no data and must load so the login page can be shown.
2. `GET /api/v1/health`, `GET /api/v1/auth/session` and `POST /api/v1/auth/login` are `Public`.
3. `POST /api/v1/auth/password` is `Public` **only while no credential exists**: first-run bootstrap, and the exposure gate (ADR 0037) guarantees that state implies loopback. Once one exists it is `Mutate` and `current_password` is required.
4. `GET` and `HEAD` are `Read`.
5. Everything else is `Mutate`.

**The middleware wraps the fallback.** Because the layer is applied to the whole router, an unauthenticated probe of `/api/v1/anything` answers 401, not 404, so a route table cannot be enumerated. That is the no-bypass property and it is a test.

**CSRF applies to cookies only.** A `Mutate` request authenticated by the cookie must echo the session's token in `X-Uguisu-CSRF`, compared with `Secret::ct_eq`; otherwise 403 `csrf_required`. A bearer principal is exempt: no browser attaches `Authorization` by itself, so there is nothing for another origin to forge.

**Bytes and streams are ordinary reads.** Archived media, artwork images and both modes of `GET /api/v1/events` are `Read`, authenticated by cookie or bearer. `bytes.rs` is untouched — the middleware runs before the handler, so the whole `Range`/304/416 path and its tests keep their behaviour.

**No credential in a query parameter, ever.** There is no such fallback to tempt anyone, and a test asserts the classifier reads none.

**Authentication off is one early return.** With no credential set, the middleware inserts `Principal::Anonymous` and returns; the same tests pin both sides of that switch.

## Consequences

- Adding a route needs no authorization decision: it is authenticated unless it is added to the public list, which is five lines long and in one file.
- The OpenAPI document declares `security: []` exactly on the public operations, and a test holds the document against the classifier — so a route cannot claim to be open unless the middleware agrees.
- A `read` token cannot pause the queue, change a setting or start a download, which is the whole point of having two scopes.
- `HEAD` is a read, so a future `HEAD` route is authenticated without anyone thinking about it.

## Alternatives considered

- **Per-handler extractors.** 60-odd signatures to change, and route 61 can still be forgotten. A classifier cannot be forgotten, because unlisted means authenticated.
- **Roles.** A matrix for one operator. ADR 0013's `admin` scope is withdrawn rather than implemented.
- **CSRF for bearer requests too.** A header a browser never sends cannot be forged by another origin; requiring one would only break `curl`.
- **A token in a query parameter for `<audio src>`.** That is a credential in a URL — in history, in logs, in referrers. The cookie exists precisely so this is not needed.
