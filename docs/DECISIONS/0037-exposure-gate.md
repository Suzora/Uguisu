# ADR 0037 — A network bind without a credential is a startup failure

**Status:** accepted, supersedes in part [ADR 0013](0013-auth-and-network-binding.md), amended by [ADR 0058](0058-refusing-cross-site-changes.md) and [ADR 0061](0061-the-docker-image.md) · **Date:** 2026-09-22

## Context

ADR 0013 made loopback the default and left it there. Nothing enforced it: `uguisu serve --bind 0.0.0.0:8484` started silently, with the same log line as a loopback bind, and until Phase 9 there was no authentication at all — so anything that could reach the port could read the whole archive, change settings and start downloads. A default is not a control if the unsafe alternative is one flag away and says nothing.

## Decision

**Refuse, before binding.** `uguisu_server::expose::check(bind, credential_set, allow_insecure)` runs before `TcpListener::bind`, and before the download workers and the scheduler start: a server that is not going to serve should not have touched the queue either. Loopback is decided on the canonical address, so `::ffff:127.0.0.1` counts as loopback and `127.0.0.53` does too.

**Exit code 12, a new one.** Adding a code is additive, so the exit-code contract is not broken. `Usage(2)` would conflate "you typed it wrong" with "refused for your safety", and `Blocked(6)` belongs to the SSRF policy.

```
uguisu serve: refusing to bind 0.0.0.0:8484 without authentication
  set a credential first: uguisu auth set-password
  or bind to loopback: --bind 127.0.0.1:8484
  to override deliberately: UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1
```

**The override is a deployment knob, not a setting.** `UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE` and `UGUISU_AUTH_COOKIE_SECURE` are `--bind`'s kind of thing (ADR 0031), not `SETTINGS` rows: a database row that permits insecure exposure could be set through the API it protects. They appear in `docs/CONFIGURATION.md` § "Process and CLI" and in neither `GET /api/v1/settings` nor `config list`, and the five unstorable keys stay five *(six since ADR 0028's 2026-10-04 amendment)*.

**When the override is used, the startup line says so.** One `WARN` naming the address, `auth = "disabled"` and the override — so the log for a wildcard bind with no password is not identical to the log for loopback. It is emitted where the listener is created, once, and nowhere else.

## Consequences

- The unsafe configuration is still reachable, in one deliberate step, and leaves a trace in the log. A control nobody can override is a control people work around.
- Phase 11's packaging inherits this: a container that binds `0.0.0.0` inside its namespace must either set a password during setup or pass the override, and that is a documented decision rather than an accident.
- `uguisu serve --bind 0.0.0.0:8484` in the brief's own examples now exits 12 without a credential; `docs/CLI.md` spells out the three ways forward.
- Nothing about TLS changes: Uguisu still does not terminate it, and still trusts no `X-Forwarded-*` header (`docs/DEPLOYMENT.md`).

## Alternatives considered

- **Warn and continue.** A warning in a log nobody reads is how Podgrab's open-by-default surface happened.
- **Refuse with no override.** Someone behind a firewall on a trusted network has a legitimate reason, and an unoverridable refusal is worked around by patching the binary.
- **Deriving the bind from whether a credential exists.** Silently moving the listener is worse than refusing: a deployment that asked for a public port and got loopback looks broken.
- **Trusting `X-Forwarded-For` to decide.** The header a proxy sets is the header an attacker sets when there is no proxy.
