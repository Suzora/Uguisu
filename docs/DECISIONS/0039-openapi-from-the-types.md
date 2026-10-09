# ADR 0039 — OpenAPI derived from the types, drift-checked in CI

**Status:** accepted, amends [ADR 0034](0034-web-ui-architecture.md) · **Date:** 2026-09-22

## Context

ADR 0004 promised "an API client generated from the OpenAPI document" and there was no document. ADR 0034 built a hand-written adapter instead and said the day a generated client arrives it replaces `types.ts` alone. That day is this phase.

The obvious way to add a document is to write one. It would be about 190 schemas of nested podcast, episode, job, archive, sidecar, discovery and settings shapes, maintained by hand beside the Rust types they mirror — which is exactly the decorative document a contract is supposed not to be. The frontend already showed what a hand-maintained mirror costs: 503 lines of TypeScript whose own header asked to be replaced.

## Decision

**The document is a function of the code.** `#[derive(ToSchema)]` beside the existing `Serialize` on every wire type, `#[utoipa::path]` beside every handler, `ApiDoc` in `crates/uguisu-server/src/openapi.rs`, and `tools/openapi` writing `docs/api/openapi.json`. `python3 scripts/check.py openapi` and a CI step fail when the committed file is not what the code produces, and `the_document_is_byte_stable` fails if generation is not deterministic.

The annotation cost was the risk, so it was measured before being committed to: the ten hardest wire types were derived first, and 53 schemas across `uguisu-core` needed **zero** `#[schema(value_type = …)]` overrides. The named fallback — a hand-written document plus key-set drift tests — was not needed.

**Two conventions the derive cannot see are stated once.** `library::body` adds `schema` to every success body, so each enveloped 2xx JSON response is wrapped as `allOf: [body, Envelope]`, with the seven operations that do not go through it named in one list. And two routes answer under a second method and path for the CLI's GET/POST-only client; each second spelling is copied in the same file with its own `operationId`, because a handler cannot carry two `utoipa::path` attributes and a function that only forwards would be worse.

**The document is held against the router, not trusted.** `every_route_has_an_operation` and `every_operation_has_a_route` read the `.route(…)` literals out of the seven routing modules' own source, because axum's `Router` cannot be asked what it contains; `the_scan_sees_every_route` stops that scan from passing by describing nothing. `declared_security_matches_the_classifier` holds each operation's `security` against `access_of` (ADR 0036). `no_two_types_share_a_schema_name` walks the crates, because a schema is named after the type's last path segment and two types with one name silently become one component.

**Types only, no runtime.** `openapi-typescript` is a frontend **devDependency**; `web/src/lib/api/generated/schema.d.ts` is committed and drift-checked, and `types.ts` re-exports it under the names the views already use. Production frontend dependencies stay at zero. `client.ts` keeps the transport and the `ApiFailure` taxonomy the views branch on — the split ADR 0034 predicted.

**No Swagger UI.** Vendored assets conflict with `deny.toml`'s crates-io-only `sources` policy, it needs axum features this workspace does not enable, and it would be an unauthenticated HTML surface for no gain. The document is a file; a client renders it.

## Consequences

- A wire shape cannot change without the document changing, and the document cannot change without the committed file and the generated TypeScript changing with it. Three checks, one source.
- `utoipa` is a dependency of six shipped crates. It brings `indexmap` and nothing else new, and `cargo deny` passes.
- Generating the types found three real defects the derive alone would not have: a schema-name collision that documented `/discovery/search` with the wrong enum, nine duplicated `operationId`s, and — caused by one of those collisions — `/api/v1/search` documented without the `schema` key it sends.
- The derive names a schema after the type, so a new type whose short name already exists needs `#[schema(as = …)]`. The test says so before the document does.

## Alternatives considered

- **A hand-written document.** ~190 schemas maintained beside the types they mirror; the thing rule 12 forbids.
- **A generated runtime client (`openapi-fetch`, generated axios).** A production frontend dependency, replacing a `client.ts` that already does exactly what the views need.
- **`utoipa-swagger-ui`.** See above; also a second HTML surface to authenticate.
- **Generating the TypeScript at build time instead of committing it.** Then a reviewer cannot see a wire change in the diff, which is the main thing the committed file buys.
