//! Streaming `GET`: chunked bodies, `Range`/`If-Range` across redirects,
//! identity encoding, header/idle timeouts, cancellation and the
//! retry-before-headers rule. Scenarios wiremock cannot express (a stalled
//! or truncated body) use a raw TCP server.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uguisu_http::{
    ClientConfig, ContentRange, GetOptions, HttpClient, HttpError, NetworkPolicy, Profile,
    RetryPolicy, StatusCode, Url,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(profile: Profile, request_timeout: Duration) -> HttpClient {
    HttpClient::new(
        profile,
        ClientConfig {
            policy: Arc::new(NetworkPolicy::strict().allow_private_hosts(["127.0.0.1"])),
            retry: RetryPolicy {
                jitter: false,
                base: Duration::from_millis(10),
                ..RetryPolicy::default()
            },
            request_timeout,
            ..ClientConfig::default()
        },
    )
    .unwrap()
}

fn media() -> HttpClient {
    client(Profile::Media, Duration::from_secs(5))
}

fn url(base: &str, p: &str) -> Url {
    Url::parse(&format!("{base}{p}")).unwrap()
}

async fn drain(mut resp: uguisu_http::StreamingResponse) -> Result<Vec<u8>, HttpError> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.body.chunk().await? {
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// What the raw server does after reading the request head.
#[derive(Clone, Copy)]
enum Script {
    /// Declares `declared` bytes, sends `sent`, then stalls forever.
    Stall { declared: usize, sent: usize },
    /// Declares `declared` bytes, sends `sent`, then closes.
    Truncate { declared: usize, sent: usize },
}

/// A minimal HTTP/1.1 server on loopback; returns its base URL and a
/// counter of requests served.
async fn raw_server(script: Script) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut head = Vec::new();
                loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let (declared, sent, stall) = match script {
                    Script::Stall { declared, sent } => (declared, sent, true),
                    Script::Truncate { declared, sent } => (declared, sent, false),
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {declared}\r\nAccept-Ranges: bytes\r\nETag: \"raw-1\"\r\nConnection: close\r\n\r\n"
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&vec![0xAB; sent]).await;
                let _ = socket.flush().await;
                if stall {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (base, hits)
}

#[tokio::test]
async fn streams_a_body_in_chunks() {
    let server = MockServer::start().await;
    let body = vec![7u8; 1_048_576];
    Mock::given(method("GET"))
        .and(path("/a.mp3"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .insert_header("accept-ranges", "bytes")
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;
    let resp = media()
        .get_stream(&url(&server.uri(), "/a.mp3"), &GetOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.content_length(), Some(1_048_576));
    assert!(resp.accept_ranges_bytes());
    assert_eq!(resp.content_type(), Some("audio/mpeg"));
    assert_eq!(resp.attempts, 1);
    let got = drain(resp).await.unwrap();
    assert_eq!(got, body);
}

#[tokio::test]
async fn range_and_if_range_ride_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/go"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/media"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/media"))
        .and(header("range", "bytes=100-"))
        .and(header("if-range", "\"etag-1\""))
        .and(header("accept-encoding", "identity"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 100-199/200")
                .set_body_bytes(vec![1u8; 100]),
        )
        .mount(&server)
        .await;
    let opts = GetOptions {
        range_start: Some(100),
        if_range: Some("\"etag-1\"".to_owned()),
        ..GetOptions::default()
    };
    let resp = media()
        .get_stream(&url(&server.uri(), "/go"), &opts)
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.redirects.len(), 1);
    assert_eq!(
        resp.content_range().unwrap().unwrap(),
        ContentRange::Bytes {
            start: 100,
            end: 199,
            total: Some(200)
        }
    );
    assert_eq!(drain(resp).await.unwrap().len(), 100);
}

#[tokio::test]
async fn media_profile_keeps_bodies_as_served() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(b"not really gzip".to_vec()),
        )
        .mount(&server)
        .await;
    // Buffered media GET: no decompression attempted, bytes pass through.
    let resp = media().get(&url(&server.uri(), "/x")).await.unwrap();
    assert_eq!(resp.body.as_ref(), b"not really gzip");
    // Streaming: same, and the encoding is reported.
    let resp = media()
        .get_stream(&url(&server.uri(), "/x"), &GetOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content_encoding().as_deref(), Some("gzip"));
    assert_eq!(drain(resp).await.unwrap(), b"not really gzip");
    // A decompressing profile fails on the bogus payload.
    let err = client(Profile::Feed, Duration::from_secs(5))
        .get(&url(&server.uri(), "/x"))
        .await
        .unwrap_err();
    assert!(matches!(err, HttpError::Body(_)), "{err:?}");
}

#[tokio::test]
async fn headers_timeout_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let err = client(Profile::Media, Duration::from_millis(200))
        .get_stream(&url(&server.uri(), "/slow"), &GetOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, HttpError::Timeout(_)), "{err:?}");
}

#[tokio::test]
async fn idle_timeout_fires_on_a_stalled_body() {
    let (base, _) = raw_server(Script::Stall {
        declared: 1_000_000,
        sent: 65_536,
    })
    .await;
    let opts = GetOptions {
        idle_timeout: Some(Duration::from_millis(300)),
        ..GetOptions::default()
    };
    let mut resp = media().get_stream(&url(&base, "/s"), &opts).await.unwrap();
    assert_eq!(resp.content_length(), Some(1_000_000));
    let mut got = 0usize;
    let err = loop {
        match resp.body.chunk().await {
            Ok(Some(c)) => got += c.len(),
            Ok(None) => panic!("body ended although the server stalled"),
            Err(e) => break e,
        }
    };
    assert_eq!(got, 65_536);
    assert!(matches!(err, HttpError::Timeout(d) if d == Duration::from_millis(300)));
    assert_eq!(resp.body.received(), 65_536);
}

#[tokio::test]
async fn cancellation_stops_a_stream_mid_body() {
    let (base, _) = raw_server(Script::Stall {
        declared: 1_000_000,
        sent: 65_536,
    })
    .await;
    let cancel = CancellationToken::new();
    let opts = GetOptions {
        cancel: Some(cancel.clone()),
        ..GetOptions::default()
    };
    let mut resp = media().get_stream(&url(&base, "/s"), &opts).await.unwrap();
    let first = resp.body.chunk().await.unwrap().unwrap();
    assert!(!first.is_empty());
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let mut err = None;
    while err.is_none() {
        match resp.body.chunk().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("stalled body must not end"),
            Err(e) => err = Some(e),
        }
    }
    assert!(matches!(err, Some(HttpError::Cancelled)));
}

#[tokio::test]
async fn a_truncated_body_is_not_retried() {
    let (base, hits) = raw_server(Script::Truncate {
        declared: 100_000,
        sent: 10_000,
    })
    .await;
    let mut resp = media()
        .get_stream(&url(&base, "/t"), &GetOptions::default())
        .await
        .unwrap();
    let mut got = 0usize;
    let mut outcome = None;
    while outcome.is_none() {
        match resp.body.chunk().await {
            Ok(Some(c)) => got += c.len(),
            Ok(None) => outcome = Some(Ok(())),
            Err(e) => outcome = Some(Err(e)),
        }
    }
    assert_eq!(got, 10_000);
    assert!(
        matches!(
            outcome,
            Some(Err(HttpError::Body(_) | HttpError::Transport(_)))
        ),
        "{outcome:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a body failure is never retried by the client"
    );
}

#[tokio::test]
async fn retries_before_headers_then_streams() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
        .mount(&server)
        .await;
    let resp = media()
        .get_stream(&url(&server.uri(), "/flaky"), &GetOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.attempts, 2);
    assert_eq!(drain(resp).await.unwrap(), b"ok");

    // With retries disabled the 503 is handed back as-is.
    let opts = GetOptions {
        retry: Some(false),
        ..GetOptions::default()
    };
    Mock::given(method("GET"))
        .and(path("/down"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "7"))
        .mount(&server)
        .await;
    let resp = media()
        .get_stream(&url(&server.uri(), "/down"), &opts)
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(resp.attempts, 1);
    assert_eq!(
        uguisu_http::retry_after(&resp.headers),
        Some(Duration::from_secs(7))
    );
}

#[tokio::test]
async fn a_private_redirect_is_blocked_early() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/leak"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
        )
        .mount(&server)
        .await;
    let err = media()
        .get_stream(&url(&server.uri(), "/leak"), &GetOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, HttpError::Policy(_)), "{err:?}");
}

#[tokio::test]
async fn body_cap_applies_to_streams() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 4096]))
        .mount(&server)
        .await;
    let opts = GetOptions {
        max_bytes: Some(1000),
        ..GetOptions::default()
    };
    // Declared length above the cap: refused at the headers.
    let err = media()
        .get_stream(&url(&server.uri(), "/big"), &opts)
        .await
        .unwrap_err();
    assert!(matches!(err, HttpError::BodyTooLarge { limit: 1000 }));
}
