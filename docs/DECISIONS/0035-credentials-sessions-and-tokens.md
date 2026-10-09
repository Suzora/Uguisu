# ADR 0035 — One credential, server-side sessions, hashed tokens

**Status:** accepted, supersedes in part [ADR 0013](0013-auth-and-network-binding.md), amended by [ADR 0042](0042-launch-credential-exchange.md) and [ADR 0062](0062-trusted-proxies.md) · **Date:** 2026-09-22

## Context

Until Phase 9 Uguisu had no authentication at all: `docs/SECURITY.md` said so plainly, and the only control was a loopback default that nothing enforced. ADR 0013 chartered the model for Phase 7, which was re-scoped; it also described a `users` table "from the first migration with a role column" that never existed — migrations 0001–0005 contain no user, session or token table.

Two audiences need to authenticate, and they cannot use the same mechanism. A browser plays archived audio with `<audio src>`, shows artwork with `<img src>` and reads events with `EventSource`; none of those can attach an `Authorization` header, and a credential in a URL would end up in history, logs and referrers. Automation — the CLI, a cron job — has no place to keep a password and should not be given one.

## Decision

**One operator, one credential row.** `auth_credential` has a single row (`CHECK (id = 1)`), a username and an argon2id PHC string. Deliberately not a `users` table with a role column: Uguisu has one operator, and a schema shaped for a feature nobody asked for is a schema that lies. Multi-user, if it ever arrives, is a forward migration like every other change.

**argon2id, measured and pinned.** 64 MiB, 2 passes, 1 lane: 101.6 ms per verification on the reference machine, which is the ≈100 ms `docs/SECURITY.md` §3.5 asks for. Hashing runs on `spawn_blocking` behind a two-permit semaphore, so at most 128 MiB is committed to hashing at once and a flood cannot stall the runtime's other work. The parameters live in the stored PHC string, so raising them later re-hashes on the next successful login rather than invalidating anything.

**The browser gets a cookie.** `uguisu_session` is a 256-bit secret, `HttpOnly`, `SameSite=Lax`, `Path=/`, with `Secure` when the deployment says TLS is in front. The database holds only its SHA-256 digest, so a stolen database cannot be replayed as a session. The CSRF token beside it is stored in the clear on purpose: it is useless without the cookie, and hashing it would cost a digest per mutation and buy nothing.

**Sessions expire two ways, and neither is a setting.** Idle 7 days, absolute 30 days — constants, because nobody asked for the knobs. `last_seen_at` and the idle deadline refresh at most once an hour: the writer pool has size 1, and a write per request would serialise the whole API behind the session table. Pruning joins the existing maintenance transaction and reports `sessions_pruned`.

**Automation gets a token.** 256 bits, shown once, stored as a digest, with two scopes — `read` and `write` — because the API has exactly two authenticated levels (ADR 0036). ADR 0013's third scope, `admin`, is withdrawn: no route asks a question it would answer. A revoked token's row is kept so `auth token list` can say it was revoked.

**A failure says one thing.** An unknown username and a wrong password answer the same 401 with the same message, and an unknown username is still verified against a dummy hash so the timing does not answer the question either.

**The limiter lives in memory and trusts no header.** Ten failures per peer address per five minutes plus a global hundred; success clears the bucket; over the limit is 429 with `Retry-After`. The map evicts its oldest window past 1 024 peers, so the limiter cannot become the exhaustion it prevents. Not in the database: a row per failed guess would put an unauthenticated write in front of the single writer. No `X-Forwarded-For` parsing, because there is no trusted-proxy knob in this phase and a spoofable header is a worse identity than the socket peer.

## Consequences

- A login costs about 100 ms and that is the point; the limiter caps it at ten hashes per peer per five minutes.
- Behind a reverse proxy every request shares one limiter bucket, and the global cap is what covers a distributed attempt. Recorded in `docs/DEPLOYMENT.md`.
- A forgotten password is recoverable only with filesystem access: `uguisu auth set-password` run embedded does not ask for the current one, because anyone who can run it already has the database and every archived file.
- The first API token must be minted with the session cookie (through the API) or embedded with the server stopped; the CLI cannot open a session from a password, so `--server auth token create` needs a token it already has.
- Authentication is not archive history, so it adds no `EventKind`. Failures are logged by category — never a username, never a value.

## Alternatives considered

- **Stateless JWT sessions.** Revocation and the CSRF pairing are both simpler with a row, and there is one node.
- **A password for automation too.** It would put the operator's password in a cron file, and every use would cost a 100 ms hash.
- **scrypt or bcrypt.** argon2id is what ADR 0013 chose and what the RFC 9106 parameters are written for; bcrypt's 72-byte input limit is a trap.
- **A database-backed limiter.** One unauthenticated path writing to the single-writer pool is exactly the amplification the limiter exists to prevent.
