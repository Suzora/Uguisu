# ADR 0013 — Authentication and network binding model

**Status:** superseded in part by [0035](0035-credentials-sessions-and-tokens.md), [0036](0036-three-access-levels.md) and [0037](0037-exposure-gate.md), amended by [0061](0061-the-docker-image.md) and [0062](0062-trusted-proxies.md) · **Date:** 2026-09-17

## Context

Self-hosted, usually single-user, sometimes exposed through a reverse proxy; the desktop app runs a local server; CLI/automation needs tokens. Multi-user is a future capability.

## Decision

- **Identity:** one admin account in v1 (username + argon2id password hash), created on first run. The `users` table exists from the first migration with a role column, so multi-user can be added without a data migration.
- **Browser sessions:** server-side sessions in SQLite, `HttpOnly` cookies, `SameSite=Lax`, CSRF token required on state-changing requests, idle and absolute expiry, revocation.
- **API tokens:** random, hashed at rest, scoped (`read`/`write`/`admin`), shown once, revocable; used by the CLI and automation via `Authorization: Bearer`.
- **Binding:** native installs default to `127.0.0.1:8484`; Docker defaults to `0.0.0.0:8484` inside the container. Reverse-proxy headers are trusted only from configured proxy addresses.
- **Desktop mode:** ephemeral loopback port, per-launch token injected into the WebView; the password login is bypassed for the local shell but the same token model applies to the CLI.
- **TLS:** not terminated by Uguisu in v1; documented reverse-proxy setup.

## Consequences

- Secure defaults without configuration; scriptable with tokens.
- No OIDC/OAuth in v1 (PodFetch offers it); can be added as an alternative login method later without changing the session model.

## Alternatives considered

- **No auth by default (Podgrab):** unacceptable even on a LAN given the SSRF and file-writing surface.
- **JWT stateless sessions:** revocation and CSRF handling are simpler with server-side sessions for a single-node app.

## Correction (2026-09-22)

This ADR was written before any of it was built, and three statements above were wrong or have been decided differently. What is built is ADRs 0035–0037; what stands here is the shape (argon2id, server-side sessions, `HttpOnly` cookies, CSRF on mutations, hashed bearer tokens, no TLS termination, no OIDC).

- **There was never a `users` table.** "The `users` table exists from the first migration with a role column" describes a migration that does not exist: 0001–0005 contain no user, session or token table. Migration 0006 introduces `auth_credential` as a **single row**, deliberately not a table shaped for multi-user (ADR 0035).
- **The `admin` scope is withdrawn.** Tokens have two scopes, `read` and `write`, because the API has exactly two authenticated levels (ADR 0036). No route asks a question a third scope would answer.
- **Binding is enforced, not defaulted.** A non-loopback bind with no credential is a startup failure with exit code 12, overridable by one deployment knob (ADR 0037). Docker's `0.0.0.0` default inherits that rule; Phase 11 owns the packaging.
- **No reverse-proxy header is trusted.** "Reverse-proxy headers are trusted only from configured proxy addresses" is not built: there is no trusted-proxy setting in this phase, so `X-Forwarded-*` is ignored entirely. `docs/DEPLOYMENT.md` says what that means for the login limiter.
- **Desktop mode is built differently** ([ADR 0041](0041-desktop-shell-and-embedded-server.md), [ADR 0042](0042-launch-credential-exchange.md)): the shell always requires authentication, and its per-launch token is exchanged once for an ordinary session rather than injected into the WebView.
