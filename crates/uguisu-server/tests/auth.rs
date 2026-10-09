//! Authentication and authorization over the real router: what is refused,
//! what is not, and what never appears in an answer.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::{StatusCode, header};
use common::{App, Options, PASSWORD};

/// Every route a client can reach, and what it should need. A route added
/// without a decision here fails `every_route_is_accounted_for`.
const ROUTES: &[(&str, &str, Need)] = &[
    ("GET", "/api/v1/health", Need::Public),
    ("GET", "/api/v1/auth/session", Need::Public),
    ("POST", "/api/v1/auth/login", Need::Public),
    ("POST", "/api/v1/auth/logout", Need::Mutate),
    ("POST", "/api/v1/auth/password", Need::Mutate),
    ("GET", "/api/v1/auth/tokens", Need::Read),
    ("GET", "/api/v1/status", Need::Read),
    ("GET", "/api/v1/podcasts", Need::Read),
    ("POST", "/api/v1/podcasts", Need::Mutate),
    ("GET", "/api/v1/podcasts/opml", Need::Read),
    ("POST", "/api/v1/podcasts/opml", Need::Mutate),
    ("POST", "/api/v1/podcasts/x/move-feed", Need::Mutate),
    ("POST", "/api/v1/podcasts/x/archive", Need::Mutate),
    ("DELETE", "/api/v1/podcasts/x", Need::Mutate),
    ("POST", "/api/v1/podcasts/x/remove", Need::Mutate),
    ("GET", "/api/v1/episodes/duplicates", Need::Read),
    ("POST", "/api/v1/episodes/x/resolve", Need::Mutate),
    ("GET", "/api/v1/downloads", Need::Read),
    ("POST", "/api/v1/downloads", Need::Mutate),
    ("GET", "/api/v1/archive", Need::Read),
    ("POST", "/api/v1/archive/verify", Need::Mutate),
    ("GET", "/api/v1/archive/orphans", Need::Read),
    ("POST", "/api/v1/archive/restore", Need::Mutate),
    ("POST", "/api/v1/archive/x/redownload", Need::Mutate),
    ("GET", "/api/v1/events?limit=1", Need::Read),
    ("GET", "/api/v1/settings", Need::Read),
    ("GET", "/api/v1/search?q=x", Need::Read),
    ("GET", "/api/v1/scheduler", Need::Read),
    ("POST", "/api/v1/scheduler/pause", Need::Mutate),
    ("POST", "/api/v1/db/check", Need::Mutate),
    ("GET", "/api/v1/discovery/providers", Need::Read),
    ("GET", "/api/v1/discovery/resolve?input=x", Need::Read),
    // `/api/v1/auth/exchange` is deliberately absent: it is the one route that
    // refuses on every server, because it answers a token and nothing else.
    // `the_exchange_answers_only_a_launch_token` covers it.
    // Not a route: the point is that it is refused rather than described.
    ("GET", "/api/v1/nothing-here", Need::Read),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Public,
    Read,
    Mutate,
}

#[tokio::test]
async fn no_credential_means_nothing_is_refused() {
    let t = App::open().await;
    for (method, uri, _) in ROUTES {
        let r = t.send_anonymous(method, uri, None).await;
        assert_ne!(
            r.status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} asked for a credential on a server that has none"
        );
        assert_ne!(r.status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    let r = t.send_anonymous("GET", "/api/v1/auth/session", None).await;
    assert_eq!(r.json["auth_required"], false, "{}", r.json);
    assert_eq!(r.json["credential_set"], false, "{}", r.json);
    assert_eq!(r.json["authenticated"], false, "{}", r.json);
    t.close().await;
}

/// The headers a browser sends from a page at `origin` to this server.
fn from_page(origin: &str, host: &str) -> Vec<(String, String)> {
    vec![
        (header::HOST.to_string(), host.to_owned()),
        (header::ORIGIN.to_string(), origin.to_owned()),
    ]
}

#[tokio::test]
async fn another_site_cannot_change_anything() {
    for t in [App::open().await, App::secured().await] {
        for (method, uri, _) in ROUTES {
            for origin in [
                "https://evil.example",
                "null",
                "http://127.0.0.1:9999",
                "http://localhost:8484",
            ] {
                let headers = from_page(origin, "127.0.0.1:8484");
                let r = common::raw(&t.router, method, uri, &headers, None).await;
                if *method == "GET" {
                    assert_ne!(r.status, StatusCode::FORBIDDEN, "{method} {uri} {origin}");
                } else {
                    assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {uri} {origin}");
                    assert_eq!(r.json["error"]["kind"], "forbidden", "{}", r.json);
                }
            }
        }
        t.close().await;
    }
}

#[tokio::test]
async fn same_origin_and_cli_can_change() {
    let t = App::open().await;
    for headers in [
        from_page("http://127.0.0.1:8484", "127.0.0.1:8484"),
        // Behind a TLS proxy that passes the Host header on.
        from_page("https://podcasts.example", "podcasts.example"),
        from_page("https://podcasts.example", "podcasts.example:443"),
        // The CLI and curl send no Origin.
        Vec::new(),
    ] {
        let r = common::raw(&t.router, "POST", "/api/v1/scheduler/pause", &headers, None).await;
        assert!(
            r.status.is_success(),
            "{headers:?}: {} {}",
            r.status,
            r.json
        );
        let r = common::raw(
            &t.router,
            "POST",
            "/api/v1/scheduler/resume",
            &headers,
            None,
        )
        .await;
        assert!(
            r.status.is_success(),
            "{headers:?}: {} {}",
            r.status,
            r.json
        );
    }
    t.close().await;
}

#[tokio::test]
async fn a_credential_means_everything_else_is_refused() {
    let t = App::secured().await;
    for (method, uri, need) in ROUTES {
        let r = t.send_anonymous(method, uri, None).await;
        if *need == Need::Public {
            assert_ne!(
                r.status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must stay reachable, or nobody can log in"
            );
        } else {
            assert_eq!(
                r.status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} answered without a credential: {}",
                r.json
            );
            assert_eq!(r.json["error"]["kind"], "unauthenticated", "{method} {uri}");
            assert_eq!(r.json["schema"], 1, "{method} {uri}");
            // No challenge: a browser that saw one would offer its own login
            // dialog and then send `Authorization` by itself (docs/API.md).
            assert_eq!(r.header("www-authenticate"), None, "{method} {uri}");
        }
    }
    t.close().await;
}

/// A path that does not exist must not be distinguishable from one that does,
/// or an unauthenticated probe is a map of the API.
#[tokio::test]
async fn an_unknown_api_path_is_401_not_404() {
    let t = App::secured().await;
    let r = t.send_anonymous("GET", "/api/v1/nothing-here", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
    let known = t.send_anonymous("GET", "/api/v1/podcasts", None).await;
    assert_eq!(known.status, r.status);
    assert_eq!(known.json["error"]["kind"], r.json["error"]["kind"]);
    t.close().await;
}

#[tokio::test]
async fn the_spa_loads_so_a_login_is_possible() {
    let t = App::secured().await;
    for uri in ["/", "/library", "/assets/index-abc.js"] {
        let r = t.send_anonymous("GET", uri, None).await;
        assert_ne!(
            r.status,
            StatusCode::UNAUTHORIZED,
            "{uri} must load: the login form is in it"
        );
    }
    t.close().await;
}

#[tokio::test]
async fn a_login_opens_a_session_and_a_wrong_one_does_not() {
    let mut t = App::secured().await;
    let r = t
        .send_anonymous(
            "POST",
            "/api/v1/auth/login",
            Some(serde_json::json!({ "username": "uguisu", "password": "wrong wrong wrong" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
    assert_eq!(r.json["error"]["kind"], "unauthenticated");
    let wrong_name = t
        .send_anonymous(
            "POST",
            "/api/v1/auth/login",
            Some(serde_json::json!({ "username": "nobody", "password": PASSWORD })),
        )
        .await;
    assert_eq!(
        wrong_name.json["error"]["message"], r.json["error"]["message"],
        "an unknown username must read the same as a wrong password"
    );
    assert!(r.set_cookie().is_none(), "a failure opens nothing");

    let ok = t.login().await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.json);
    assert_eq!(ok.json["username"], "uguisu");
    assert!(ok.json["csrf_token"].is_string());
    assert!(
        !ok.text().contains(PASSWORD),
        "the password must not come back: {}",
        ok.text()
    );
    let cookie = ok.set_cookie().unwrap();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
    assert!(
        !cookie.contains("Secure"),
        "plain HTTP must not claim Secure, or the browser drops the cookie: {cookie}"
    );

    let listed = t.send("GET", "/api/v1/podcasts", None).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.json);
    t.close().await;
}

#[tokio::test]
async fn a_secure_deployment_marks_the_cookie() {
    let mut t = App::with(Options {
        credential: true,
        cookie_secure: true,
        ..Options::default()
    })
    .await;
    let ok = t.login().await;
    assert!(ok.set_cookie().unwrap().contains("; Secure"));
    t.close().await;
}

#[tokio::test]
async fn a_cookie_change_needs_the_csrf_token() {
    let mut t = App::secured().await;
    t.login().await;

    let without = t
        .send_without_csrf(
            "POST",
            "/api/v1/podcasts",
            Some(serde_json::json!({ "input": "https://example.com/feed.xml" })),
        )
        .await;
    assert_eq!(without.status, StatusCode::FORBIDDEN, "{}", without.json);
    assert_eq!(without.json["error"]["kind"], "csrf_required");

    // With the header the request reaches the handler, which then fails on its
    // own merits — a refusal from the feed host, not from the auth layer.
    let with = t
        .send(
            "POST",
            "/api/v1/podcasts",
            Some(serde_json::json!({ "input": "https://example.invalid/feed.xml" })),
        )
        .await;
    assert_ne!(with.status, StatusCode::FORBIDDEN, "{}", with.json);
    assert_ne!(with.status, StatusCode::UNAUTHORIZED, "{}", with.json);

    // A reading request needs no header at all.
    assert_eq!(
        t.send_without_csrf("GET", "/api/v1/podcasts", None)
            .await
            .status,
        StatusCode::OK
    );
    t.close().await;
}

#[tokio::test]
async fn logging_out_closes_the_session() {
    let mut t = App::secured().await;
    t.login().await;
    let out = t.send("POST", "/api/v1/auth/logout", None).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert!(
        out.set_cookie().unwrap().contains("Max-Age=0"),
        "the browser is told to drop it"
    );
    assert_eq!(
        t.send("GET", "/api/v1/podcasts", None).await.status,
        StatusCode::UNAUTHORIZED,
        "the cookie stops working even though the browser still has it"
    );
    t.close().await;
}

#[tokio::test]
async fn a_write_token_works_without_csrf_and_a_read_token_cannot_change_anything() {
    let mut t = App::secured().await;
    t.login().await;

    let write = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "automation" })),
        )
        .await;
    assert_eq!(write.status, StatusCode::CREATED, "{}", write.json);
    assert_eq!(write.json["token"]["scope"], "write");
    let write_secret = write.json["secret"].as_str().unwrap().to_owned();

    let read = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "dashboard", "scope": "read" })),
        )
        .await;
    assert_eq!(read.status, StatusCode::CREATED, "{}", read.json);
    let read_secret = read.json["secret"].as_str().unwrap().to_owned();
    let read_id = read.json["token"]["id"].as_str().unwrap().to_owned();

    // A token needs no CSRF header: a browser cannot attach one by itself, so
    // there is nothing for another site to forge.
    t.bearing(&write_secret);
    let created = t
        .send(
            "POST",
            "/api/v1/podcasts",
            Some(serde_json::json!({ "input": "https://example.invalid/feed.xml" })),
        )
        .await;
    assert_ne!(created.status, StatusCode::FORBIDDEN, "{}", created.json);
    assert_ne!(created.status, StatusCode::UNAUTHORIZED, "{}", created.json);

    t.bearing(&read_secret);
    assert_eq!(
        t.send("GET", "/api/v1/podcasts", None).await.status,
        StatusCode::OK,
        "a read token reads"
    );
    let refused = t
        .send(
            "POST",
            "/api/v1/podcasts",
            Some(serde_json::json!({ "input": "https://example.invalid/feed.xml" })),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.json);
    assert_eq!(refused.json["error"]["kind"], "forbidden");

    // Revoking it stops it, and the list never carries a secret.
    t.bearing(&write_secret);
    let listed = t.send("GET", "/api/v1/auth/tokens", None).await;
    assert_eq!(listed.json["tokens"].as_array().unwrap().len(), 2);
    assert!(
        !listed.text().contains(&read_secret) && !listed.text().contains(&write_secret),
        "a listed token must not be a usable token"
    );
    assert_eq!(
        t.send("DELETE", &format!("/api/v1/auth/tokens/{read_id}"), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    t.bearing(&read_secret);
    assert_eq!(
        t.send("GET", "/api/v1/podcasts", None).await.status,
        StatusCode::UNAUTHORIZED,
        "a revoked token authenticates nothing"
    );
    t.close().await;
}

#[tokio::test]
async fn a_nonsense_credential_is_simply_not_one() {
    let t = App::secured().await;
    for headers in [
        vec![("cookie".to_owned(), "uguisu_session=nonsense".to_owned())],
        vec![("authorization".to_owned(), "Bearer nonsense".to_owned())],
        vec![("authorization".to_owned(), "Basic nonsense".to_owned())],
    ] {
        let r = common::raw(&t.router, "GET", "/api/v1/podcasts", &headers, None).await;
        assert_eq!(
            r.status,
            StatusCode::UNAUTHORIZED,
            "{headers:?}: {}",
            r.json
        );
    }
    t.close().await;
}

/// A password may not be changed by a cookie alone: a session that was left
/// open must not be enough to lock the operator out.
#[tokio::test]
async fn changing_the_password_needs_the_current_one() {
    let mut t = App::secured().await;
    t.login().await;
    let without = t
        .send(
            "POST",
            "/api/v1/auth/password",
            Some(serde_json::json!({ "new_password": "a whole new one" })),
        )
        .await;
    assert_eq!(without.status, StatusCode::FORBIDDEN, "{}", without.json);
    let with = t
        .send(
            "POST",
            "/api/v1/auth/password",
            Some(serde_json::json!({
                "current_password": PASSWORD,
                "new_password": "a whole new one",
            })),
        )
        .await;
    assert_eq!(with.status, StatusCode::NO_CONTENT, "{}", with.json);
    assert_eq!(
        t.send("GET", "/api/v1/podcasts", None).await.status,
        StatusCode::OK,
        "the browser that changed it stays logged in"
    );
    t.close().await;
}

/// On a fresh install the first password can be set without one, because there
/// is nothing to authenticate with yet — and the exposure gate is what makes
/// that state mean loopback.
#[tokio::test]
async fn the_first_password_can_be_set_then_the_route_closes() {
    let t = App::open().await;
    let first = t
        .send_anonymous(
            "POST",
            "/api/v1/auth/password",
            Some(serde_json::json!({ "new_password": "a first password" })),
        )
        .await;
    assert_eq!(first.status, StatusCode::NO_CONTENT, "{}", first.json);

    let again = t
        .send_anonymous(
            "POST",
            "/api/v1/auth/password",
            Some(serde_json::json!({ "new_password": "a second password" })),
        )
        .await;
    assert_eq!(
        again.status,
        StatusCode::UNAUTHORIZED,
        "the bootstrap window closes with the first password: {}",
        again.json
    );
    t.close().await;
}

#[tokio::test]
async fn the_login_limiter_answers_429() {
    let t = App::secured().await;
    let mut limited = false;
    for _ in 0..15 {
        let r = t
            .send_anonymous(
                "POST",
                "/api/v1/auth/login",
                Some(serde_json::json!({ "username": "uguisu", "password": "wrong wrong wrong" })),
            )
            .await;
        if r.status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(r.json["error"]["kind"], "too_many_requests");
            let wait = r
                .headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok());
            assert!(
                wait.is_some_and(|seconds| (1..=300).contains(&seconds)),
                "a client told to wait needs to know how long: {:?}",
                r.headers.get("retry-after")
            );
            limited = true;
            break;
        }
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.json);
    }
    assert!(limited, "guessing has to stop being free eventually");
    t.close().await;
}

#[tokio::test]
async fn the_event_stream_needs_a_credential() {
    let mut t = App::secured().await;
    for uri in ["/api/v1/events", "/api/v1/events?limit=5"] {
        assert_eq!(
            t.send_anonymous("GET", uri, None).await.status,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    t.login().await;
    let paged = t.send("GET", "/api/v1/events?limit=5", None).await;
    assert_eq!(paged.status, StatusCode::OK, "{}", paged.json);
    t.close().await;
}

#[tokio::test]
async fn no_answer_carries_the_password_or_a_token() {
    let mut t = App::secured().await;
    let login = t.login().await;
    let cookie = login.cookie_value().unwrap().to_owned();
    let created = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "laptop" })),
        )
        .await;
    let secret = created.json["secret"].as_str().unwrap().to_owned();

    for (method, uri) in [
        ("GET", "/api/v1/auth/session"),
        ("GET", "/api/v1/auth/tokens"),
        ("GET", "/api/v1/settings"),
        ("GET", "/api/v1/status"),
    ] {
        let r = t.send(method, uri, None).await;
        let text = r.text();
        assert!(
            !text.contains(PASSWORD),
            "{method} {uri} leaked the password"
        );
        assert!(!text.contains(&secret), "{method} {uri} leaked a token");
        assert!(!text.contains(&cookie), "{method} {uri} leaked the cookie");
    }
    t.close().await;
}

/// Bytes are behind the layer too, and the layer runs first: an anonymous
/// request for media or artwork is refused before the handler could say
/// whether the id exists.
#[tokio::test]
async fn media_and_artwork_bytes_need_a_credential() {
    let t = App::secured().await;
    let episode = uguisu_core::ids::EpisodeId::new();
    let podcast = uguisu_core::ids::PodcastId::new();
    for uri in [
        format!("/api/v1/archive/{episode}/media"),
        format!("/api/v1/podcasts/{podcast}/artwork/image"),
    ] {
        let r = t.send_anonymous("GET", &uri, None).await;
        assert_eq!(
            r.status,
            StatusCode::UNAUTHORIZED,
            "{uri} answered {}: {}",
            r.status,
            r.json
        );
        // Also a Range request, so the byte path cannot be reached another way.
        let ranged = common::raw(
            &t.router,
            "GET",
            &uri,
            &[("range".to_owned(), "bytes=0-1".to_owned())],
            None,
        )
        .await;
        assert_eq!(
            ranged.status,
            StatusCode::UNAUTHORIZED,
            "{uri} with a Range"
        );
    }
    t.close().await;
}

/// The table above is the contract this file tests; a route that answers
/// something no row predicted is a row somebody forgot to add.
#[tokio::test]
async fn every_route_is_accounted_for() {
    let t = App::secured().await;
    for (method, uri, need) in ROUTES {
        let r = t.send_anonymous(method, uri, None).await;
        let refused = r.status == StatusCode::UNAUTHORIZED;
        assert_eq!(
            refused,
            *need != Need::Public,
            "{method} {uri} is marked {need:?} but answered {}",
            r.status
        );
    }
    t.close().await;
}

/// Nothing but a write token, presented from this machine, may exchange.
#[tokio::test]
async fn the_exchange_answers_only_a_launch_token() {
    // With authentication off there is no principal to be, so the route
    // refuses rather than minting a session for whoever asked.
    let off = App::open().await;
    let anyone = off
        .send_anonymous("POST", "/api/v1/auth/exchange", None)
        .await;
    assert_eq!(anyone.status, StatusCode::FORBIDDEN, "{}", anyone.json);
    off.close().await;

    let mut t = App::secured().await;
    t.login().await;
    let read = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "reader", "scope": "read" })),
        )
        .await;
    let read_only = read.json["secret"].as_str().unwrap().to_owned();
    let write = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "desktop launch" })),
        )
        .await;
    let launch = write.json["secret"].as_str().unwrap().to_owned();

    // A cookie principal already has a session; there is nothing to exchange.
    let again = t.send("POST", "/api/v1/auth/exchange", None).await;
    assert_eq!(
        again.status,
        StatusCode::FORBIDDEN,
        "cookie: {}",
        again.json
    );

    t.anonymous();
    let nobody = t.send("POST", "/api/v1/auth/exchange", None).await;
    assert_eq!(
        nobody.status,
        StatusCode::UNAUTHORIZED,
        "anonymous: {}",
        nobody.json
    );

    t.bearing(&read_only);
    let scoped = t.send("POST", "/api/v1/auth/exchange", None).await;
    assert_eq!(
        scoped.status,
        StatusCode::FORBIDDEN,
        "read token: {}",
        scoped.json
    );

    t.bearing(&launch);
    let elsewhere = t
        .send_from(
            "203.0.113.9:40000".parse().unwrap(),
            "POST",
            "/api/v1/auth/exchange",
            None,
        )
        .await;
    assert_eq!(
        elsewhere.status,
        StatusCode::FORBIDDEN,
        "off-machine: {}",
        elsewhere.json
    );
    assert_eq!(elsewhere.json["error"]["kind"], "forbidden");
    t.close().await;
}

/// The desktop bootstrap: a per-launch write token becomes an ordinary
/// session, and that session is in no way privileged (ADR 0042).
#[tokio::test]
async fn a_launch_token_exchanges_for_an_ordinary_session() {
    let mut t = App::secured().await;
    t.login().await;
    let write = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "desktop launch" })),
        )
        .await;
    let launch = write.json["secret"].as_str().unwrap().to_owned();

    t.bearing(&launch);
    let opened = t.send("POST", "/api/v1/auth/exchange", None).await;
    assert_eq!(opened.status, StatusCode::CREATED, "{}", opened.json);
    let cookie = opened.set_cookie().unwrap().to_owned();
    let csrf = opened.json["csrf_token"].as_str().unwrap().to_owned();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"),
        "the exchanged cookie must be as locked down as a login's: {cookie}"
    );
    assert!(
        !opened.text().contains(&launch),
        "the answer must not echo the launch token"
    );

    let headers = vec![(header::COOKIE.to_string(), cookie.clone())];
    let listed = common::raw_from(
        &t.router,
        common::LOOPBACK,
        "GET",
        "/api/v1/podcasts",
        &headers,
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.json);

    let add = Some(serde_json::json!({ "input": "https://example.invalid/feed.xml" }));
    let forged = common::raw_from(
        &t.router,
        common::LOOPBACK,
        "POST",
        "/api/v1/podcasts",
        &headers,
        add.clone(),
    )
    .await;
    assert_eq!(
        forged.status,
        StatusCode::FORBIDDEN,
        "the exchanged session must not be exempt from CSRF: {}",
        forged.json
    );
    assert_eq!(forged.json["error"]["kind"], "csrf_required");

    // With the header it reaches the handler, which then fails on its own
    // merits rather than on authorization.
    let with_csrf = common::raw_from(
        &t.router,
        common::LOOPBACK,
        "POST",
        "/api/v1/podcasts",
        &[
            (header::COOKIE.to_string(), cookie),
            ("x-uguisu-csrf".to_owned(), csrf),
        ],
        add,
    )
    .await;
    assert_ne!(
        with_csrf.status,
        StatusCode::FORBIDDEN,
        "{}",
        with_csrf.json
    );
    assert_ne!(
        with_csrf.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        with_csrf.json
    );
    t.close().await;
}

/// Ten wrong passwords from `client`, sent by `socket` with `X-Forwarded-For`;
/// then whether `other` behind the same socket is still asked for one.
async fn other_client_still_tries(t: &App, socket: &str, client: &str, other: &str) -> StatusCode {
    let socket: std::net::SocketAddr = socket.parse().unwrap();
    let wrong = serde_json::json!({ "username": "uguisu", "password": "wrong wrong wrong" });
    for _ in 0..12 {
        common::raw_from(
            &t.router,
            socket,
            "POST",
            "/api/v1/auth/login",
            &[
                ("content-type".to_owned(), "application/json".to_owned()),
                ("x-forwarded-for".to_owned(), client.to_owned()),
            ],
            Some(wrong.clone()),
        )
        .await;
    }
    common::raw_from(
        &t.router,
        socket,
        "POST",
        "/api/v1/auth/login",
        &[
            ("content-type".to_owned(), "application/json".to_owned()),
            ("x-forwarded-for".to_owned(), other.to_owned()),
        ],
        Some(wrong),
    )
    .await
    .status
}

#[tokio::test]
async fn a_trusted_proxy_names_the_client() {
    let behind = App::with(Options {
        credential: true,
        trusted_proxy: Some("10.0.0.0/8"),
        ..Options::default()
    })
    .await;
    // Each client behind the proxy has its own bucket.
    assert_eq!(
        other_client_still_tries(&behind, "10.0.0.2:4000", "203.0.113.5", "203.0.113.6").await,
        StatusCode::UNAUTHORIZED
    );
    behind.close().await;

    // With no proxy configured the header is nobody's word: one bucket.
    let direct = App::secured().await;
    assert_eq!(
        other_client_still_tries(&direct, "10.0.0.2:4000", "203.0.113.5", "203.0.113.6").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    direct.close().await;
}

/// A credential never travels in a URL, where proxies and browser history
/// keep it: a valid token in a query string is not one.
#[tokio::test]
async fn query_token_is_refused() {
    let mut t = App::secured().await;
    t.login().await;
    let made = t
        .send(
            "POST",
            "/api/v1/auth/tokens",
            Some(serde_json::json!({ "name": "in a url" })),
        )
        .await;
    let secret = made.json["secret"].as_str().unwrap().to_owned();
    for name in [
        "token",
        "access_token",
        "api_key",
        "bearer",
        "authorization",
    ] {
        let r = t
            .send_anonymous("GET", &format!("/api/v1/podcasts?{name}={secret}"), None)
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "?{name}=");
    }
    t.close().await;
}
