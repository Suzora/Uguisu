# ADR 0038 — One envelope on every failure, one kind vocabulary

**Status:** accepted · **Date:** 2026-09-22

## Context

Before Phase 9 the API failed in four different shapes. Twenty hand-written error sites spanned nine kinds, two of which — `bad_request` and `invalid` — meant the same thing for the same class of input. Seven handlers answered axum's rejections as plain text outside the envelope, so a malformed body got `Expected request with 'Content-Type: application/json'` with no `kind` at all. `/discovery/resolve` answered a second, incompatible object. No error body carried `schema`. And `body()` could answer `200 OK` with `null` if serialisation failed.

A client cannot branch on that, and an agent reading `--json` cannot either.

## Decision

```json
{ "schema": 1, "error": { "kind": "archive_hash_mismatch", "message": "…" } }
```

**Every failure, including a rejection.** Hand-written `FromRequest`/`FromRequestParts` for `Body<T>`, `MaybeBody<T>` and `Params<T>` answer the envelope: syntax, query and path errors are 400 `invalid`, a missing JSON content type is 415, a data error is 400 with serde's field message, over the body limit is 413. A `map_response` net rewrites anything else with a status ≥ 400 and a non-JSON content type, which catches axum's 405 and whatever a future layer adds. `Router::method_not_allowed_fallback` handles the 405 specifically, because a blanket response rewrite would have destroyed the 416's `Content-Range`.

**One kind per meaning.** `bad_request` is deleted; `invalid` is the single 400 kind for client input.

**Archive kinds reach the wire.** `UguisuError::kind()` returns `ArchiveErrorKind::as_str()` for an archive failure, so a client sees `archive_hash_mismatch` or `archive_path_collision` instead of `archive` with the real kind buried in prose. `kind` stays `&'static str`; no new field, no computed value.

**Errors carry `schema` too.** Additive: the web client reads `error.kind` and `error.message`, the CLI reads `error`, and neither notices a sibling key.

**`body()` cannot lie.** A payload that will not serialise, or is not a JSON object, is a logged 500 rather than `200 OK` with `null`. A payload that already carries its own `schema` keeps it — `Inspection`, `RefreshReport` and `Sidecar` are versioned documents, and replacing their number with the API's would claim they are a version they are not.

**Resolution joins the envelope.** `/discovery/resolve` answers `{ schema, error: { kind, message, suggestion, detail }, provenance }`: the standard envelope plus the two things only this route has. The separate `POST /api/v1/podcasts` path keeps its own `unresolvable` kind, because "tell me what this is" and "add this" fail differently, and that difference is documented in `docs/API.md` rather than smoothed away.

## Consequences

- A client can switch on `error.kind` for every failure the API produces, and `docs/API.md` lists the vocabulary with its statuses.
- Changing a message means grepping `crates/*/tests` and `docs/CLI.md` for the literal, which is now the only way a message moves.
- The seven new kinds authentication needs — `unauthenticated`, `forbidden`, `csrf_required`, `too_many_requests`, `unsupported_media_type`, `payload_too_large` — are in the same vocabulary, so the middleware needs no shape of its own.
- Bytes a user sees changed on `GET /api/v1/archive/{id}/sidecar`, on `/discovery/resolve`'s failures and on every rejection that used to be plain text. Each is named in the commit that changed it.

## Alternatives considered

- **`problem+json` (RFC 9457).** A second vocabulary beside the one the CLI already renders, for no gain on a single-node API.
- **Keeping `bad_request` as an alias.** Two names for one meaning is what this ADR exists to remove.
- **A computed `kind` string.** `&'static str` means the vocabulary is enumerable from the source, and a typo is a compile error.
