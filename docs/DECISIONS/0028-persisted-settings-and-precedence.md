# ADR 0028 — Persisted settings, and what wins

**Status:** accepted, amended 2026-10-03 (Phase 10) and 2026-10-04 · **Date:** 2026-09-21 · supersedes the precedence stated in `CONFIGURATION.md` and `ARCHITECTURE.md` before Phase 7 · relates to [ADR 0013](0013-auth-and-network-binding.md)

## Context

Configuration lived in the environment, so a UI could never change anything and an operator had to restart a container to raise a concurrency. The Phase-1 sketch said `defaults → config.toml → environment → settings`, with the database on top.

## Decision

### Two layers, and the environment wins

`defaults < stored settings < environment < command-line flag`.

The sketch had it the other way round. An operator who writes `UGUISU_FEED_REFRESH_CONCURRENCY=4` into a unit file or a compose file has to be able to rely on it; a value the API would silently override is worse than one it refuses to take. So a key that is set in the environment is **pinned**: the API and the CLI report it, and a write is refused with a conflict rather than stored and ignored. There is no `--force` — the remedy is to unset the variable.

### One parser, one vocabulary

The stored key *is* the environment variable's name, and the stored value is exactly the text that variable would hold — not JSON, as the sketch had it, because a second syntax is a second parser and a second way to be wrong. A write is validated by re-assembling the whole configuration, so the cross-field rules (`per_host ≤ global`, `backoff_max ≥ backoff_base`, a non-empty template) are checked over the merged view, which is the only view in which they mean anything, and a rejection quotes the message an environment variable would have produced.

`core::settings::SETTINGS` is the one list of keys: what shape a value has, whether it may be stored at all, and whether changing it does anything before a restart. It is metadata, not a second parser.

### Applied one key at a time, and quarantined rather than deleted

Stored settings are merged one key at a time, each step validating the whole configuration. That is what makes quarantine precise: a value that is only wrong *in company* names the key that broke it, where a single merge could only report that something, somewhere, is wrong.

**A stored value never prevents a start, and is never deleted to achieve that.** A value that no longer parses is quarantined: the row stays exactly as it was written, the engine opens without it, a warning names it and a `settings.rejected` event records it. Refusing to start would brick the daemon whose API is the only way to repair the row; deleting the row would destroy the only copy of what the user meant. Only an explicit `config set` or `config unset` clears one. An unknown key is likewise inert and reported, never auto-deleted.

### Six keys may never live in the database

`UGUISU_PODCASTINDEX_KEY` and `_SECRET` (secrets stay in the environment or a file), `UGUISU_DATA_DIR` and `UGUISU_MEDIA_DIR` (a row inside the database cannot say where the database is), and `UGUISU_HTTP_ALLOW_PRIVATE_HOSTS` — the SSRF allowlist, because one compromised session must not be able to widen the network policy (`SECURITY.md` §3.1).

*Amended 2026-10-04:* and `UGUISU_DISCOVERY_PODCASTINDEX_BASE_URL`. Podcast Index's key and secret are sent to whatever it names, so one stored row would have had a write-scope session send them to a host of its choosing. Found by the Phase 11 security review.

### Live versus restart is stated per key, not claimed for all

The engine holds the merged configuration behind an `RwLock<Arc<Config>>` and hands out a snapshot; a caller that read one value never finds the next one changed underneath it. But swapping the snapshot does not make everything live: the three HTTP clients, the discovery stack, the download queue's dependencies and the destination resolver capture their configuration at construction. Those keys are stored with `restart_required: true` rather than pretending.

### `config.toml` is not part of Phase 7

This phase defines exactly two layers below the command line. No file layer is read, written or half-wired. The documentation says so positively, because `CONFIGURATION.md` and `ARCHITECTURE.md` previously *announced* one.

A future file layer **inherits nothing from the superseded documentation**: its position in the order is an open decision for the phase that builds it, to be made against the pinning rule established here, not derived from a sketch that predates it. The precedence above is stated as a closed list of the layers that exist, so adding one is visibly an amendment rather than a detail.

**v1 has no file layer** (decided in Phase 10, 2026-10-03). An environment file — systemd's `EnvironmentFile=`, `docker --env-file`, a compose `env_file:` — is already a file-based configuration, and it has the pinning semantics above; stored settings cover changing a running daemon. A third layer would add a precedence question and a parser for no capability v1 lacks.

## Consequences

`uguisu config set` changes a running daemon for the live keys and says "restart required" for the rest. `config validate` fails when a stored value is being ignored, because a configuration the operator asked for and is not getting is exactly what that command exists to find.

The `settings` table is a durable record of intent, including intent Uguisu currently cannot honour.

## Alternatives considered

**Database wins over the environment.** The sketch's order. It makes a UI authoritative, and it makes a compose file a lie.

**Per-key validators.** Cheaper, and unable to express the rules that actually break a configuration.

**Delete what does not parse.** Tidy, and it throws away the only copy of what somebody meant at the exact moment they need to correct it.
