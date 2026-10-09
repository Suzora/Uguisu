# ADR 0042 — A per-launch token, exchanged once for an ordinary session

**Status:** accepted, amends [ADR 0035](0035-credentials-sessions-and-tokens.md) · **Date:** 2026-09-24

## Context

The embedded server must not be an anonymous API. Any process on the machine can reach a loopback port, and a fresh install has no password. Standalone `uguisu serve` answers anonymously on loopback until a password is set (ADR 0037). That is acceptable for someone who started a server on purpose, but not for an application that starts one silently.

The WebView still has to become authenticated without anyone typing a password. A page cannot set an `HttpOnly` cookie, and a secret in the URL would sit in history, logs and the referrer.

## Decision

**The shell always runs the server with authentication required**, whether or not a password exists. It mints an ordinary `write` token (ADR 0035) named `desktop launch`, valid for one hour. The token lives only in the shell's memory: never an argument, an environment variable, a URL or a log field.

**One route, one IPC command:**

```
WebView boot ─ GET /api/v1/auth/session ─ authenticated? ─ done
                         │ no
      invoke("launch_credential")  →  the secret, once; a second call gets nothing
      POST /api/v1/auth/exchange   Authorization: Bearer <secret>
                         ←  201, Set-Cookie: HttpOnly; SameSite=Lax + csrf_token
```

`POST /api/v1/auth/exchange` needs no classifier change. A non-GET API path is already `Mutate`, so default-deny already refuses anonymous and read-scoped callers, and a bearer principal is already exempt from CSRF. The handler adds exactly two conditions: the principal must be a token, and the peer must be loopback, read from the socket, never a header. `Engine::open_session` is `login` without the password check, not a second way to mint a session. The route refuses on every server, including one with authentication off, because only a token can use it.

**The frontend surface is one module.** `web/src/lib/api/desktop.ts` is called from the session effect `App.svelte` already has. `client.ts` gains a `bearer` option that only `exchange()` uses. A general headers map is deliberately not added, so no ordinary caller can attach an `Authorization` header. Outside the shell all of it is inert.

**At shutdown the token is revoked**, and the plaintext is dropped first. A credential that outlives its launch is dead rather than merely unused.

## Consequences

- A desktop install is never an anonymous API, and the page never holds a reusable secret.
- The exchanged session is ordinary: it expires, it can be signed out, and it still needs the CSRF header to mutate.
- Setting a password later from the UI still works: the bootstrapped session may mutate.
- `uguisu auth token list` shows one `desktop launch` row per launch, revoked once the launch ends.

## Alternatives considered

- **Leave the embedded server anonymous on loopback.** Every local process, and every page able to reach loopback, would get the whole API.
- **A token in the start URL or a query string.** History, logs and referrers keep it.
- **Inject the cookie from the shell.** Tauri 2.11.6 exposes no API to write one into the WebView.
- **A reusable launch token.** The session cookie carries every request after the first exchange. A page that loses its session anyway, for instance after signing out, gets no second credential from `launch_credential` and falls back to the login view, as it would in a browser. A token the page could ask for again would sit in reach for the whole launch.
