//! Smoke tests of every scenario the media server offers, through the
//! streaming client of `uguisu-http`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use uguisu_download::testing::{MediaServer, content_bytes, content_sha256};
use uguisu_http::{
    ClientConfig, ContentRange, GetOptions, HttpClient, HttpError, NetworkPolicy, Profile,
    RetryPolicy, StatusCode, Url,
};

fn client() -> HttpClient {
    HttpClient::new(
        Profile::Media,
        ClientConfig {
            policy: Arc::new(NetworkPolicy::strict().allow_private_hosts(["127.0.0.1"])),
            retry: RetryPolicy {
                jitter: false,
                base: Duration::from_millis(10),
                ..RetryPolicy::default()
            },
            request_timeout: Duration::from_secs(5),
            ..ClientConfig::default()
        },
    )
    .unwrap()
}

async fn fetch(
    server: &MediaServer,
    path: &str,
    opts: &GetOptions,
) -> (StatusCode, Vec<u8>, uguisu_http::HeaderMap) {
    let mut r = client()
        .get_stream(&Url::parse(&server.url(path)).unwrap(), opts)
        .await
        .unwrap();
    let status = r.status;
    let headers = r.headers.clone();
    let mut out = Vec::new();
    while let Some(c) = r.body.chunk().await.unwrap() {
        out.extend_from_slice(&c);
    }
    (status, out, headers)
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

#[tokio::test]
async fn generated_content_is_deterministic_and_seekable() {
    let whole = content_bytes(200_000, 0, 200_000);
    assert_eq!(whole.len(), 200_000);
    assert_eq!(content_bytes(200_000, 70_000, 5_000), whole[70_000..75_000]);
    assert_eq!(content_sha256(200_000), sha(&whole));
    assert_ne!(content_sha256(200_000), content_sha256(200_001));
}

#[tokio::test]
async fn range_routes_honour_range_and_if_range() {
    let server = MediaServer::start().await;
    let (status, body, headers) = fetch(&server, "/range/300000", &GetOptions::default()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sha(&body), content_sha256(300_000));
    assert_eq!(headers.get("accept-ranges").unwrap(), "bytes");
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_owned();
    assert!(etag.starts_with('"'));

    let opts = GetOptions {
        range_start: Some(100_000),
        if_range: Some(etag.clone()),
        ..GetOptions::default()
    };
    let (status, body, headers) = fetch(&server, "/range/300000", &opts).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, content_bytes(300_000, 100_000, 200_000));
    assert_eq!(
        ContentRange::parse(headers.get("content-range").unwrap().to_str().unwrap()).unwrap(),
        ContentRange::Bytes {
            start: 100_000,
            end: 299_999,
            total: Some(300_000)
        }
    );
    // A stale If-Range yields the full body.
    let stale = GetOptions {
        range_start: Some(100_000),
        if_range: Some("\"other\"".to_owned()),
        ..GetOptions::default()
    };
    let (status, body, _) = fetch(&server, "/range/300000", &stale).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 300_000);
    // Beyond the end: 416.
    let beyond = GetOptions {
        range_start: Some(300_000),
        ..GetOptions::default()
    };
    let (status, _, headers) = fetch(&server, "/range/300000", &beyond).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(headers.get("content-range").unwrap(), "bytes */300000");
    let reqs = server.requests_for("/range/300000");
    assert_eq!(reqs.len(), 4);
    assert_eq!(reqs[1].range_start(), Some(100_000));
    assert_eq!(reqs[1].if_range.as_deref(), Some(etag.as_str()));
    assert_eq!(reqs[0].accept_encoding.as_deref(), Some("identity"));
    server.stop().await;
}

#[tokio::test]
async fn degraded_validator_and_encoding_routes() {
    let server = MediaServer::start().await;
    let (_, _, h) = fetch(&server, "/weak-etag/1000", &GetOptions::default()).await;
    assert!(h.get("etag").unwrap().to_str().unwrap().starts_with("W/"));

    let opts = GetOptions {
        range_start: Some(500),
        ..GetOptions::default()
    };
    let (status, body, h) = fetch(&server, "/no-range/1000", &opts).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 1000);
    assert!(h.get("accept-ranges").is_none());

    let (status, body, h) = fetch(&server, "/no-length/70000", &GetOptions::default()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 70_000);
    assert!(h.get("content-length").is_none());

    let (_, _, h1) = fetch(&server, "/changed-etag/1000", &GetOptions::default()).await;
    let (_, _, h2) = fetch(&server, "/changed-etag/1000", &GetOptions::default()).await;
    assert_ne!(h1.get("etag"), h2.get("etag"));

    let (_, body, h) = fetch(&server, "/gzip/1000", &GetOptions::default()).await;
    assert_eq!(h.get("content-encoding").unwrap(), "gzip");
    assert_eq!(body, content_bytes(1000, 0, 1000), "served as-is");
    server.stop().await;
}

#[tokio::test]
async fn redirects_statuses_rate_limit_and_flaky() {
    let server = MediaServer::start().await;
    for status in [301, 302, 307, 308] {
        let mut r = client()
            .get_stream(
                &Url::parse(&server.url(&format!("/redirect/{status}/3/1000"))).unwrap(),
                &GetOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.redirects.len(), 3);
        assert_eq!(r.permanent_redirect, status == 301 || status == 308);
        let mut n = 0;
        while let Some(c) = r.body.chunk().await.unwrap() {
            n += c.len();
        }
        assert_eq!(n, 1000);
    }
    for target in ["metadata", "lan", "loopback"] {
        let err = client()
            .get_stream(
                &Url::parse(&server.url(&format!("/redirect-private/{target}"))).unwrap(),
                &GetOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Policy(_)), "{target}: {err:?}");
    }
    for code in [304u16, 403, 404, 408, 410, 429, 500, 502, 503, 504] {
        let opts = GetOptions {
            retry: Some(false),
            ..GetOptions::default()
        };
        let r = client()
            .get_stream(
                &Url::parse(&server.url(&format!("/status/{code}"))).unwrap(),
                &opts,
            )
            .await
            .unwrap();
        assert_eq!(r.status.as_u16(), code);
        if code == 429 || code == 503 {
            assert_eq!(
                uguisu_http::retry_after(&r.headers),
                Some(Duration::from_secs(1))
            );
        }
    }
    // rate-limit: 429 first (retried by the client), then the body.
    let (status, body, _) = fetch(&server, "/rate-limit/5000", &GetOptions::default()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 5000);
    assert_eq!(server.hits("/rate-limit/5000"), 2);
    // flaky: 503 twice then ok; with retries disabled the 503 shows.
    let opts = GetOptions {
        retry: Some(false),
        ..GetOptions::default()
    };
    let (status, _, _) = fetch(&server, "/flaky/2/100", &opts).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, _) = fetch(&server, "/flaky/2/100", &opts).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, body, _) = fetch(&server, "/flaky/2/100", &opts).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 100);
    server.stop().await;
}

#[tokio::test]
async fn truncate_disconnect_stall_slow_and_bad_ranges() {
    let server = MediaServer::start().await;
    let c = client();
    // truncate: declared 200k, 70k sent, then closed → body error.
    for path in ["/truncate/200000/70000", "/disconnect/200000/70000"] {
        let mut r = client()
            .get_stream(
                &Url::parse(&server.url(path)).unwrap(),
                &GetOptions::default(),
            )
            .await
            .unwrap_or_else(|e| panic!("{path}: {e:?}"));
        assert_eq!(r.content_length(), Some(200_000));
        let mut got = 0usize;
        let err = loop {
            match r.body.chunk().await {
                Ok(Some(ch)) => got += ch.len(),
                Ok(None) => panic!("{path}: ended cleanly"),
                Err(e) => break e,
            }
        };
        assert_eq!(got, 70_000, "{path}");
        assert!(
            matches!(err, HttpError::Body(_) | HttpError::Transport(_)),
            "{path}: {err:?}"
        );
    }
    // stall: idle timeout.
    let opts = GetOptions {
        idle_timeout: Some(Duration::from_millis(300)),
        ..GetOptions::default()
    };
    let mut r = c
        .get_stream(
            &Url::parse(&server.url("/stall/100000/1000")).unwrap(),
            &opts,
        )
        .await
        .unwrap();
    let mut got = 0usize;
    let err = loop {
        match r.body.chunk().await {
            Ok(Some(ch)) => got += ch.len(),
            Ok(None) => panic!("stall ended"),
            Err(e) => break e,
        }
    };
    assert_eq!(got, 1000);
    assert!(matches!(err, HttpError::Timeout(_)));
    // slow: 20 KB at 40 KB/s takes about half a second but is not an error.
    let started = std::time::Instant::now();
    let opts = GetOptions {
        idle_timeout: Some(Duration::from_secs(2)),
        ..GetOptions::default()
    };
    let (status, body, _) = fetch(&server, "/slow/20000/40000", &opts).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 20_000);
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "{:?}",
        started.elapsed()
    );
    // bad ranges
    let opts = GetOptions {
        range_start: Some(10),
        ..GetOptions::default()
    };
    let r = c
        .get_stream(
            &Url::parse(&server.url("/content-range-bad/1000")).unwrap(),
            &opts,
        )
        .await
        .unwrap();
    assert!(matches!(
        r.content_range(),
        Some(Err(HttpError::MalformedHeader { .. }))
    ));
    let r = c
        .get_stream(
            &Url::parse(&server.url("/range-wrong-start/1000")).unwrap(),
            &opts,
        )
        .await
        .unwrap();
    assert_eq!(
        r.content_range().unwrap().unwrap(),
        ContentRange::Bytes {
            start: 9,
            end: 999,
            total: Some(1000)
        }
    );
    server.stop().await;
}

#[tokio::test]
async fn concurrency_is_observed_per_route() {
    let server = MediaServer::start().await;
    let c = client();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let c = c.clone();
        let url = Url::parse(&server.url("/slow/20000/40000")).unwrap();
        tasks.push(tokio::spawn(async move {
            let mut r = c.get_stream(&url, &GetOptions::default()).await.unwrap();
            while r.body.chunk().await.unwrap().is_some() {}
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(server.max_concurrent("slow"), 4);
    assert_eq!(server.max_concurrent_total(), 4);
    assert_eq!(server.max_concurrent("range"), 0);
    server.reset();
    assert!(server.requests().is_empty());
    assert_eq!(server.max_concurrent("slow"), 0);
    server.stop().await;
}
