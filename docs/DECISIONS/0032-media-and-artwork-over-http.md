# ADR 0032 — Archived media and artwork over HTTP

**Status:** accepted · **Date:** 2026-09-21 · relates to [ADR 0020](0020-id-based-media-paths.md), [ADR 0022](0022-template-grammar-and-path-safety.md), [ADR 0026](0026-tagging-and-artwork.md)

## Context

Through Phase 7 the API described Uguisu's files but served none of them. An episode's audio was reachable only as `Enclosure.url` — the publisher's URL, which plays the publisher's copy and not the archive — and artwork only as a `relative_path` and a hash, which a browser cannot use at all. A web UI therefore could not play what Uguisu had downloaded, and could not show a cover without telling every publisher's CDN that this library exists.

## Decision

**Two routes, both addressed by id, never by path.**

```
GET /api/v1/archive/{episode_id}/media
GET /api/v1/podcasts/{id}/artwork/image
```

The caller names an entity; the handler reads the record and takes the path from it. There is no `?path=` parameter anywhere, which is the property that makes the rest of this tractable.

**Resolution is the archive's, not a new one.** `RelativePath::parse` → `resolve_checked` → `std::fs::symlink_metadata(…).is_file()`. The first refuses an absolute path, a drive letter, a UNC prefix, `..` and NUL; the second canonicalises the deepest existing ancestor and refuses anything that lands outside the media root; the third refuses a symlink standing where the artifact should be, whatever it points at, and a directory or device node. This is the sequence `uguisu-archive`'s verification already used, quoted rather than reimplemented.

**A media response never leaves the archive's own shape.** The media route additionally refuses a record whose path is under the reserved control directory or is a sidecar. Those shapes cannot occur in an `archive_files` row today; the check is at the HTTP boundary because a row is data, and this route turns data into bytes a browser executes against the same origin.

**The recorded `Content-Type` is never echoed.** It came from the publisher's response. It is mapped to a container through `uguisu_download::paths::mime_extension` and back to the one type Uguisu publishes for that container, falling back to the sniffed container and then to `application/octet-stream`. An archived HTML error page therefore cannot come back as `text/html` from the UI's own origin — which would be stored XSS — and a header-injection attempt cannot survive the round trip. Artwork needs none of this: `ArtworkFormat::mime()` is derived from bytes Phase 6 already validated.

**Range requests are answered** because `<audio>` seeking depends on them: one range per request, `bytes=a-b`, `bytes=a-` and `bytes=-n`, clamped to the length, `206` with `Content-Range`, `416` with `bytes */size` when the range names no byte, and `Accept-Ranges: bytes` on every response. Multiple ranges and unparseable values serve the whole file, which RFC 9110 permits. The strong validator is the recorded SHA-256 — it changes exactly when the bytes do — so `If-None-Match` answers `304` and a stale `If-Range` forces the whole file rather than splicing two versions of a file together.

**An error names the relative path, never the resolved one.** A path that leaves the root logs the full reason server-side, where absolute paths belong, and answers with a message that names only the relative path the API already publishes.

## Consequences

- A browser can play the archive and show Uguisu's own artwork, so the UI needs no third-party requests to render a library.
- Media bytes are streamed (`tokio-util`'s `io` feature, via `axum::body::Body::from_stream`), so a 400 MB episode costs no memory. That feature is the only dependency change.
- A missing file answers `409 archive (archive_missing)` rather than `404`: the record exists, and `status_for` already maps a present record with absent bytes to a conflict. The archive view is where that is diagnosed.
- Both routes need `read` access like every other `GET` ([ADR 0036](0036-three-access-levels.md)); until a password is set, `uguisu serve` answers them anonymously on loopback ([ADR 0037](0037-exposure-gate.md)).

## Alternatives considered

- **A generic file endpoint (`GET /file?path=…`).** Refused outright: it makes every path check the only thing between an HTTP request and the filesystem, and there is no version of it that stays safe as callers multiply.
- **Serving the media root with a static-file service.** Would expose sidecars, manifests, the control directory and `.part` files by construction, and would need the same allow-listing to be safe — at which point the id-addressed route is simpler.
- **Hotlinking `artwork_url`.** No server work, but it leaks the library's existence and the viewer's address to every publisher's CDN, and breaks whenever one blocks hotlinking. The UI keeps it only as a fallback for a podcast whose artwork Uguisu has not fetched.
- **Redirecting to a signed path served by a static handler.** Signing needs a secret the unauthenticated server has no way to keep meaningful, and it reintroduces path handling.
