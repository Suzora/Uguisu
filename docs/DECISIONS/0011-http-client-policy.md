# ADR 0011 — Single HTTP client crate with SSRF policy

**Status:** accepted, amended 2026-09-17 (Phase 2), 2026-09-18 (Phase 4) · **Date:** 2026-09-17

## Context

Discovery, feed fetching and media downloads all make outbound requests from URLs supplied by users, feeds and providers. SSRF protection, rate limiting, conditional requests and size caps must be consistent and impossible to bypass by accident.

## Decision

- `uguisu-http` is the only crate allowed to depend on `reqwest`; CI enforces this with `cargo deny` bans once the crates have dependencies.
- It exposes purpose-specific clients (`for_feeds()`, `for_media()`, `for_discovery()`, `for_html()`) built from one policy: rustls, compression, configurable User-Agent, timeouts, per-purpose size caps, per-host concurrency and rate limits (`governor`), retry with backoff and `Retry-After`.
- SSRF: scheme allow-list, pre-connect DNS resolution with public-address checks (IPv4 and IPv6, including mapped forms), manual redirect handling with re-validation per hop (max 5), optional admin allow-list for private hosts.
- Helpers for conditional GET (ETag/Last-Modified) and validated `Range` resumption.

## Consequences

- One place to audit and fuzz; policy changes apply everywhere.
- Slightly more code than using `reqwest` defaults (manual redirects), justified by the threat model.

## Alternatives considered

- **`reqwest` directly in each crate with a shared builder function:** easy to bypass; policy drift over time.
- **`hyper` directly:** more control, much more code; `reqwest` with manual redirects is sufficient.

## Amendment 2026-09-17 (Phase 2, as built)

SSRF enforcement has two layers: (1) `NetworkPolicy::check_url` plus explicit pre-resolution and address classification of every resolved address; (2) `SafeResolver`, a `reqwest::dns::Resolve` hook that returns only policy-approved addresses, so the socket can only connect to what was validated (DNS rebinding cannot bypass layer 1). Redirects use `redirect::Policy::none()` and a manual loop with per-hop re-validation. A `Profile::Trusted` client (private addresses allowed) exists solely for the user-configured `--server` address in the CLI. `cargo deny` bans `reqwest` outside `uguisu-http`. Retry handling stays explicit in `uguisu-http` (not `reqwest`'s built-in retry) so tests are deterministic; profiles choose the policy (default: 3 attempts; providers: fail fast).

## Amendment 2026-09-17 (Phase 2, Part B — live validation)

Both layers were exercised against real targets with the release build: literal loopback, RFC 1918, link-local (cloud metadata address), decimal/hex/octal IPv4 forms, public names resolving to loopback (`127.0.0.1.nip.io`, `localtest.me` → `::1`), redirects from a public host to `127.0.0.1` and `169.254.169.254`, a disallowed port and a six-hop redirect chain. Every case was refused without a connection (exit code 6) or stopped at the redirect limit; a real three-host redirect chain (`darknetdiaries.com` → `feeds.megaphone.fm` → `podcast.darknetdiaries.com`) passed with each hop re-validated. `Response::cache_max_age` was extended to return zero for `no-cache`/`no-store` so callers never cache what the origin forbids.

## Amendment 2026-09-18 (Phase 4, streaming)

`get_stream()` returns the response head plus a `BodyStream` whose `chunk()` applies the idle timeout, the cancellation token and the byte cap, so a media body is never buffered in memory (a 1 GiB download costs ≈ 20 MiB of resident memory). Retries inside `get_stream` stop once headers arrive: a body that fails mid-transfer belongs to the download engine's persistent schedule, not to an in-request retry that would restart the transfer from zero.

`request_timeout` moved from the client builder to the request, because a total deadline is meaningless for a body that legitimately takes an hour; for streaming it bounds only the headers. `Profile::Media` builds a client with decompression disabled and sends `Accept-Encoding: identity`, so `Content-Length` describes the bytes that land on disk. `GetOptions` gained `range_start`, `if_range` and `idle_timeout`; `Range`/`If-Range` are injected per hop, so they survive redirects, while `Authorization`/`Cookie` are still dropped when the host changes.

New building blocks: `HostKey` (`scheme://normalized-host:port`) as the identity for per-host limits, `HostThrottles` with a **synchronous** `try_acquire` plus `saturated()` so the download scheduler can exclude busy hosts from its claim query instead of blocking on a permit, and `headers.rs` with parsed accessors (`etag`, `last_modified`, `content_length`, `accept_ranges_bytes`, `content_range`) shared by buffered and streaming responses. A malformed header is a typed `MalformedHeader` error, not a silent default.
