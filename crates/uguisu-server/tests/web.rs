//! Serving the built SPA: deep links, asset types, and the paths a request
//! must never reach.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uguisu_core::config::DiscoveryConfig;
use uguisu_server::{AppState, router};

struct App {
    router: Router,
    dir: tempfile::TempDir,
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let at = root.join(relative);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::fs::write(at, contents).unwrap();
}

/// A discovery-only server with a web directory: nothing here needs a library.
fn app(with_web: bool) -> App {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("dist");
    write(&dist, "index.html", b"<!doctype html><title>Uguisu</title>");
    write(&dist, "assets/index-a1b2c3.js", b"export const x = 1;");
    write(&dist, "assets/index-a1b2c3.css", b":root{}");
    write(&dist, "favicon.svg", b"<svg/>");
    let discovery =
        uguisu_discovery::assemble(DiscoveryConfig::from_lookup(|_| None).unwrap()).unwrap();
    let state = AppState::new(discovery, None).with_web_dir(with_web.then_some(dist));
    App {
        router: router(state),
        dir,
    }
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, content_type, body)
}

#[tokio::test]
async fn the_root_serves_the_shell() {
    let t = app(true);
    let (status, content_type, body) = get(&t.router, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert!(String::from_utf8_lossy(&body).contains("<title>Uguisu</title>"));
}

#[tokio::test]
async fn a_deep_link_survives_a_refresh() {
    let t = app(true);
    for uri in [
        "/podcasts",
        "/podcasts/01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "/search?q=rust",
        "/downloads/01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ] {
        let (status, content_type, body) = get(&t.router, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(content_type, "text/html; charset=utf-8", "{uri}");
        assert!(String::from_utf8_lossy(&body).contains("Uguisu"), "{uri}");
    }
}

#[tokio::test]
async fn assets_keep_their_own_types() {
    let t = app(true);
    let (status, content_type, body) = get(&t.router, "/assets/index-a1b2c3.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/javascript; charset=utf-8");
    assert_eq!(body, b"export const x = 1;");

    let (status, content_type, _) = get(&t.router, "/assets/index-a1b2c3.css").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/css; charset=utf-8");

    let (status, content_type, _) = get(&t.router, "/favicon.svg").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/svg+xml");
}

#[tokio::test]
async fn a_missing_asset_is_not_the_shell() {
    let t = app(true);
    let (status, _, body) = get(&t.router, "/assets/gone-ffffff.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["error"]["kind"], "not_found");
}

#[tokio::test]
async fn an_unknown_api_route_stays_json() {
    let t = app(true);
    for uri in ["/api", "/api/v1/nope", "/api/v2/podcasts"] {
        let (status, content_type, body) = get(&t.router, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(content_type.starts_with("application/json"), "{uri}");
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"]["kind"], "not_found", "{uri}");
    }
}

#[tokio::test]
async fn traversal_never_leaves_the_directory() {
    let t = app(true);
    std::fs::write(t.dir.path().join("secret.txt"), b"TOPSECRET").unwrap();
    for uri in [
        "/../secret.txt",
        "/../../etc/passwd",
        "/assets/../../secret.txt",
        "/..%2Fsecret.txt",
        "/%2e%2e/secret.txt",
        "/./../secret.txt",
        "//etc/passwd",
    ] {
        let (_, _, body) = get(&t.router, uri).await;
        assert!(
            !String::from_utf8_lossy(&body).contains("TOPSECRET"),
            "{uri} reached outside the web directory"
        );
        assert!(
            !String::from_utf8_lossy(&body).contains("root:"),
            "{uri} reached outside the web directory"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_leaving_the_directory_fails() {
    let t = app(true);
    std::fs::write(t.dir.path().join("secret.txt"), b"TOPSECRET").unwrap();
    std::os::unix::fs::symlink(
        t.dir.path().join("secret.txt"),
        t.dir.path().join("dist/leak.txt"),
    )
    .unwrap();
    std::os::unix::fs::symlink(t.dir.path(), t.dir.path().join("dist/up")).unwrap();

    for uri in ["/leak.txt", "/up/secret.txt"] {
        let (status, _, body) = get(&t.router, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            !String::from_utf8_lossy(&body).contains("TOPSECRET"),
            "{uri} followed a link out of the web directory"
        );
    }
}

#[tokio::test]
async fn no_web_directory_serves_api_only() {
    let t = app(false);
    let (status, content_type, _) = get(&t.router, "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(content_type.starts_with("application/json"));

    let (status, _, _) = get(&t.router, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
}

/// The shell is the one response that is a document, so it is the one that
/// needs a policy about what the document may load.
#[tokio::test]
async fn the_shell_carries_a_content_security_policy() {
    let t = app(true);
    let resp = t
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let csp = header("content-security-policy");
    for directive in [
        "default-src 'self'",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "base-uri 'none'",
        "media-src 'self'",
    ] {
        assert!(csp.contains(directive), "{directive} missing from {csp}");
    }
    assert!(
        !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
        "the built page has no inline script or style, so nothing needs an \
         escape hatch: {csp}"
    );
    // Images are the one thing that may come from elsewhere: a feed's artwork
    // URL is the documented fallback, and artwork fetching is off by default.
    assert!(csp.contains("img-src * data:"), "{csp}");
    for executable in [
        "script-src 'self'",
        "style-src 'self'",
        "connect-src 'self'",
    ] {
        assert!(
            csp.contains(executable),
            "only images may come from elsewhere: {csp}"
        );
    }
    assert_eq!(header("x-content-type-options"), "nosniff");
    assert_eq!(header("referrer-policy"), "same-origin");
}
