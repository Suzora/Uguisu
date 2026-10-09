# ADR 0058 — A change sent from another site is refused

**Status:** accepted, amended 2026-10-09, amends [ADR 0037](0037-exposure-gate.md) · **Date:** 2026-10-04

## Context

With no password set, which is how a fresh loopback install starts, every request is anonymous and the server checks nothing else. Cross-site JSON posts were already stopped by the failing preflight (`Body<T>` requires `application/json`, and there is no CORS). But many mutations take no body at all, such as retry-failed, pause-all, `db vacuum`, `db backup` and refresh-all. Others take an optional one (`MaybeBody`), and axum accepts a request without a content type there as "no body". Any web page the operator visits could therefore fire those with a plain `<form method=post>` or a `no-cors` fetch, with no preflight and no credential. SECURITY.md named "CSRF from a malicious site against the local server" as a threat, but its control, the CSRF token, covers cookie sessions only.

## Decision

**A request that is not `GET`, `HEAD` or `OPTIONS` and carries an `Origin` header whose authority is not this server's own is refused** with 403 `forbidden`, in the authentication middleware, before authentication is resolved. "Own" means the `Host` header, or the URI's authority when there is none. Both sides drop a default port (`:80`, `:443`), and only the authority is compared, because behind a TLS proxy the browser's scheme is not the one the server sees. `Origin: null`, as sent by a sandboxed or `file:` page, is refused as well.

**A request without `Origin` passes.** The CLI, curl and scripts send none. A browser sends one on every request whose method is not `GET` or `HEAD`, form posts and `no-cors` fetches included (`null` when the referrer policy hides the page), so a change without `Origin` did not come from a web page.

It applies whether or not authentication is on. With a password it is a second layer behind the CSRF token, and it also stops a cross-site login.

## Consequences

- A reverse proxy must pass the browser's `Host` header on (`proxy_set_header Host $http_host` in nginx, the default in Caddy and Traefik), or every change made through it is refused. `docs/DEPLOYMENT.md` says so.
- The desktop shell and the web UI are same-origin by construction and are unaffected, as is the Vite dev proxy (`changeOrigin: false`).
- **DNS rebinding is not covered.** A rebinding page's `Origin` equals the `Host` it makes the browser send. Only a `Host` allowlist would stop that, and it would break the deliberate insecure-exposure override on a LAN and any proxy setup with an unlisted name. It is recorded as a residual risk in `docs/SECURITY.md` §3.6. The defence is the one ADR 0037 already prefers: set a password.

## Alternatives considered

- **A `Host` allowlist as well.** It would also close rebinding, but it needs a configured list of names for every LAN and proxy deployment, and getting it wrong refuses the operator's own browser. Deferred to the threat review, not rejected. *(Amended 2026-10-09: not built; see the amendment.)*
- **Requiring a custom header on every mutation, even without a session.** A page cannot send one without a preflight, so it would work in browsers. But every curl script and automation that changes something today would have to start sending it, which is a contract change for clients that were never at risk.
- **`Sec-Fetch-Site` instead of `Origin`.** For this check it says nothing `Origin` does not, and a request without it would need the same pass-through rule.

## Amendment 2026-10-09

DNS rebinding with authentication off is an accepted residual; no `Host` allowlist is built, for the reasons above. The defence stays the password, and `serve` makes the residual visible: while no password is set, it logs a `WARN` at startup on a loopback address too, saying that a website can reach the server through DNS rebinding. `docs/SECURITY.md` §3.6 names what such a page can do.
