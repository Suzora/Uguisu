//! The error contract: every failure answers one shape, whatever refused it
//! (ADR 0038).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::App;

/// Sends a request with a body that is deliberately not built by `serde_json`,
/// so a malformed one can be sent.
async fn send(
    t: &App,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &'static str,
) -> common::Reply {
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
        .collect();
    common::raw_text(&t.router, method, uri, &headers, body).await
}

const JSON: &[(&str, &str)] = &[("content-type", "application/json")];

#[tokio::test]
async fn malformed_json_answers_the_envelope() {
    let t = App::open().await;
    let r = send(&t, "POST", "/api/v1/podcasts", JSON, "{not json").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "invalid", "{}", r.json);
    assert_eq!(r.json["schema"], 1, "{}", r.json);
    assert!(
        r.json["error"]["message"].is_string(),
        "a refusal has to say what was wrong: {}",
        r.json
    );
    t.close().await;
}

#[tokio::test]
async fn a_body_missing_a_field_is_refused_in_the_envelope() {
    let t = App::open().await;
    let r = send(&t, "POST", "/api/v1/podcasts", JSON, "{}").await;
    // Well-formed JSON that is the wrong shape is 422, not 400 — axum's
    // split, and the same one `status_for` already makes for `Unresolvable`.
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "invalid", "{}", r.json);
    assert!(
        r.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("input"),
        "the refusal should name the field: {}",
        r.json
    );
    t.close().await;
}

#[tokio::test]
async fn a_body_without_a_content_type_is_415() {
    let t = App::open().await;
    let r = send(&t, "POST", "/api/v1/podcasts", &[], "{}").await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{}", r.json);
    assert_eq!(
        r.json["error"]["kind"], "unsupported_media_type",
        "{}",
        r.json
    );
    t.close().await;
}

#[tokio::test]
async fn an_unparsable_query_parameter_is_refused() {
    let t = App::open().await;
    let r = send(&t, "GET", "/api/v1/downloads?limit=abc", &[], "").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "invalid", "{}", r.json);
    assert_eq!(r.json["schema"], 1, "{}", r.json);
    t.close().await;
}

#[tokio::test]
async fn a_path_that_refuses_the_method_is_405() {
    let t = App::open().await;
    let r = send(&t, "DELETE", "/api/v1/podcasts", &[], "").await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "method_not_allowed", "{}", r.json);
    assert_eq!(r.json["schema"], 1, "{}", r.json);
    t.close().await;
}

#[tokio::test]
async fn an_unknown_path_under_api_stays_json() {
    let t = App::open().await;
    let r = send(&t, "GET", "/api/v1/nothing-here", &[], "").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "not_found", "{}", r.json);
    t.close().await;
}

#[tokio::test]
async fn no_response_carries_a_cors_header() {
    let t = App::open().await;
    // Same-origin only: the SPA is served by this process (ADR 0031), so a
    // cross-origin grant would only ever widen what a hostile page can reach.
    let evil = &[("origin", "https://evil.example")][..];
    for (method, uri) in [
        ("GET", "/api/v1/health"),
        ("GET", "/api/v1/podcasts"),
        ("OPTIONS", "/api/v1/podcasts"),
        ("POST", "/api/v1/podcasts"),
    ] {
        let r = send(&t, method, uri, evil, "").await;
        let offered: Vec<&str> = r
            .headers
            .keys()
            .map(axum::http::HeaderName::as_str)
            .filter(|k| k.starts_with("access-control-"))
            .collect();
        assert!(
            offered.is_empty(),
            "{method} {uri} offered {offered:?} to another origin"
        );
    }
    t.close().await;
}

#[tokio::test]
async fn every_failure_carries_the_schema() {
    let t = App::open().await;
    let unknown = uguisu_core::ids::PodcastId::new();
    for (method, uri, body, headers, expected) in [
        (
            "GET",
            "/api/v1/podcasts/nope".to_owned(),
            "",
            &[][..],
            StatusCode::BAD_REQUEST,
        ),
        (
            "GET",
            format!("/api/v1/podcasts/{unknown}"),
            "",
            &[][..],
            StatusCode::NOT_FOUND,
        ),
        (
            "GET",
            "/api/v1/discovery/search".to_owned(),
            "",
            &[][..],
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/api/v1/podcasts".to_owned(),
            "{not json",
            JSON,
            StatusCode::BAD_REQUEST,
        ),
        (
            "DELETE",
            "/api/v1/podcasts".to_owned(),
            "",
            &[][..],
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "GET",
            "/api/v1/nope".to_owned(),
            "",
            &[][..],
            StatusCode::NOT_FOUND,
        ),
    ] {
        let r = send(&t, method, &uri, headers, body).await;
        assert_eq!(r.status, expected, "{method} {uri}: {}", r.json);
        assert_eq!(r.json["schema"], 1, "{method} {uri}: {}", r.json);
        assert!(
            r.json["error"]["kind"].is_string(),
            "{method} {uri}: {}",
            r.json
        );
    }
    t.close().await;
}

#[tokio::test]
async fn every_answer_carries_the_security_headers() {
    let t = App::open().await;
    for (method, uri) in [
        ("GET", "/api/v1/health"),
        ("GET", "/api/v1/podcasts"),
        ("GET", "/api/v1/nothing-here"),
        ("DELETE", "/api/v1/podcasts"),
    ] {
        let r = send(&t, method, uri, &[], "").await;
        assert_eq!(
            r.header("x-content-type-options"),
            Some("nosniff"),
            "{method} {uri}"
        );
        assert_eq!(
            r.header("referrer-policy"),
            Some("same-origin"),
            "{method} {uri}"
        );
        // The CSP is for documents; a JSON body is not one.
        assert!(
            r.header("content-security-policy").is_none(),
            "{method} {uri} sent a CSP with a JSON body"
        );
    }
    t.close().await;
}

#[tokio::test]
async fn an_oversized_body_is_413() {
    let t = App::open().await;
    // One byte of JSON per character, so this is over axum's 2 MB limit.
    let body = serde_json::json!({ "input": "x".repeat(2 * 1024 * 1024) });
    let r = t.send("POST", "/api/v1/podcasts", Some(body)).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "payload_too_large", "{}", r.json);
    t.close().await;
}
