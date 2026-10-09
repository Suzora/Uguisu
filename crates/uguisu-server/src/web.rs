//! Serving the built web UI from a directory (ADR 0031).
//!
//! The router's fallback: anything that is not an API route is a request for
//! the SPA. A path that names no file and carries no extension is a
//! client-side route, so `index.html` answers it and the browser survives a
//! refresh on a deep link.
//!
//! The request path goes through the same [`RelativePath`] and
//! [`resolve_checked`] the archive uses, so `..`, an absolute path and a
//! symlink out of the directory are all refused. It is **not**
//! percent-decoded: every name Vite emits is plain ASCII, and not decoding
//! removes the decode-then-traverse class of bug entirely.

use std::path::Path;

use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use uguisu_archive::{RelativePath, resolve_checked};

use crate::{ApiError, AppState};

const INDEX: &str = "index.html";

/// Answers every request the API did not claim.
pub(crate) async fn fallback(State(state): State<AppState>, uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return no_route(path);
    }
    let Some(root) = state.web.as_deref() else {
        return no_route(path);
    };
    let requested = path.trim_start_matches('/');
    if !requested.is_empty()
        && let Ok(relative) = RelativePath::parse(requested)
        && let Some(response) = file(root, &relative).await
    {
        return response;
    }
    // A name with a dot asked for an asset, not a route: answering it with
    // the shell would make a missing bundle look like a working page.
    if requested
        .rsplit('/')
        .next()
        .is_some_and(|n| n.contains('.'))
    {
        return no_route(path);
    }
    match RelativePath::parse(INDEX) {
        Ok(index) => match file(root, &index).await {
            Some(response) => response,
            None => no_route(path),
        },
        Err(_) => no_route(path),
    }
}

/// Reads one file under `root`, or `None` when it is absent or is not a
/// regular file. Static assets are small and served whole: no `Range`, and
/// no validator beyond the immutable file names Vite emits.
async fn file(root: &Path, relative: &RelativePath) -> Option<Response> {
    let resolved = resolve_checked(root, relative).ok()?;
    let meta = tokio::fs::symlink_metadata(&resolved).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = tokio::fs::read(&resolved).await.ok()?;
    let mut response = Response::new(Body::from(bytes));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type(relative.file_name())),
    );
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache_control(relative.as_str())),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    Some(response)
}

/// Vite writes content-hashed names under `assets/`; everything else, the
/// shell above all, must be revalidated or a deploy never reaches the tab.
fn cache_control(relative: &str) -> &'static str {
    if relative.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn content_type(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, e)| e);
    match extension {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn no_route(path: &str) -> Response {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("no route {path}"),
    )
    .into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hashed_assets_are_immutable() {
        assert_eq!(
            cache_control("assets/index-a1b2c3.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control("index.html"), "no-cache");
    }

    #[test]
    fn bundles_get_executable_types() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("index-a1b2c3.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("index-a1b2c3.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("logo.svg"), "image/svg+xml");
        assert_eq!(content_type("LICENSE"), "application/octet-stream");
        assert_eq!(content_type("payload.exe"), "application/octet-stream");
    }
}
