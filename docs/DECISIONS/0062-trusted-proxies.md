# ADR 0062 — Trusted proxies name the client, and nothing else

**Status:** accepted, amends [ADR 0013](0013-auth-and-network-binding.md) and [ADR 0035](0035-credentials-sessions-and-tokens.md) · **Date:** 2026-10-04

## Context

Uguisu reads no `X-Forwarded-*` header: anybody can send one, and a header a client writes is a worse identity than the socket it came from. Behind a reverse proxy that costs two things. Every request comes from the proxy, so the login limiter's per-peer bucket (ten failures in five minutes) is one bucket for everybody: one person guessing locks everybody out until the window ends, and only the global cap of a hundred bounds guessing. And every log line names the proxy. The Docker image (ADR 0061) makes a proxy in front the normal deployment, not the exception.

## Decision

**A process option lists the proxies.** `--trusted-proxy <ip|cidr>` (repeatable, or comma-separated in `UGUISU_TRUSTED_PROXIES`) names the addresses or networks whose forwarding is believed. Like `--bind` and the exposure override (ADR 0037) it is a deployment knob, never a stored setting: a database row that decides whose word counts could be written through the API it protects. `0.0.0.0/0` and `::/0` are refused at start, since they would let any client name itself; a host name is refused too, since resolving it would make trust depend on DNS.

**Only `X-Forwarded-For`, and only from a trusted peer.** When the socket's peer is not a trusted proxy, no header is read. When it is, the `X-Forwarded-For` hops are read from the right: each address a trusted proxy added is skipped, and the first one that is not trusted is the client. A client can prepend whatever it likes; it cannot change what the proxies appended. A hop that is not an address ends the walk at the proxy that forwarded it. `Forwarded`, `X-Real-IP` and the `-Proto` and `-Host` headers are not read.

**The client is used where an address identifies a person.** The login limiter's bucket and the `peer` field of the login log lines use it. The session exchange of the desktop shell (ADR 0042) keeps checking the socket's peer: "from this machine" is a fact about the connection, and no header may claim it.

**TLS stays explicit.** The session cookie gets `Secure` from `UGUISU_AUTH_COOKIE_SECURE` and from nothing else, and the `Origin` check (ADR 0058) compares with the `Host` header the proxy passes on.

## Consequences

- Behind a configured proxy, each client has its own login bucket, and logs name the client.
- A deployment that lists a proxy must make sure only that proxy can reach the port; a trusted network that other hosts share lets them set their own address. `DEPLOYMENT.md` says so, and the Docker documentation binds the published port to loopback.
- With nothing configured, behaviour is exactly as before.

## Alternatives considered

- **Trust any private address as a proxy.** Convenient in Docker, where the proxy is on a bridge network, but every other container on that network could then pick its bucket. Naming the network is one line.
- **Read `Forwarded` (RFC 7239) too.** Two headers that can disagree are two sources of truth; nginx and Caddy set `X-Forwarded-For` by default.
- **Take the leftmost hop.** That is the address the client claims, which is exactly what must not be believed.
- **Use the forwarded scheme to set `Secure` on the cookie.** A wrong guess makes sessions fail or travel in clear; an explicit switch does not guess.
