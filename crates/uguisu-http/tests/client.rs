//! Integration tests for the HTTP client against a local wiremock server.
//!
//! The mock server listens on 127.0.0.1, so tests that need a *reachable*
//! target allow-list `127.0.0.1` (and `localhost`) explicitly; tests that
//! verify blocking use hosts that are not allow-listed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use uguisu_http::{
    ClientConfig, GetOptions, HeaderName, HeaderValue, HttpClient, HttpError, NetworkPolicy,
    Profile, RetryPolicy, StatusCode, Url,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn policy_for(server: &MockServer) -> NetworkPolicy {
    let _ = server;
    NetworkPolicy::strict().allow_private_hosts(["127.0.0.1", "localhost"])
}

fn client(server: &MockServer, profile: Profile) -> HttpClient {
    HttpClient::new(
        profile,
        ClientConfig {
            policy: std::sync::Arc::new(policy_for(server)),
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

fn url(server: &MockServer, p: &str) -> Url {
    Url::parse(&format!("{}{p}", server.uri())).unwrap()
}

#[tokio::test]
async fn gets_json_and_sends_its_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(header("user-agent", uguisu_core::USER_AGENT))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;
    let resp = client(&server, Profile::Discovery)
        .get(&url(&server, "/search"))
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.body.as_ref(), br#"{"ok":true}"#);
    assert!(resp.redirects.is_empty());
    assert_eq!(resp.attempts, 1);
    let req = &server.received_requests().await.unwrap()[0];
    assert!(
        req.headers
            .get("accept")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("application/json")
    );
}

#[tokio::test]
async fn follows_same_host_redirects_and_records_them() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/old"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/new"))
        .respond_with(ResponseTemplate::new(200).set_body_string("final"))
        .mount(&server)
        .await;
    let resp = client(&server, Profile::Feed)
        .get(&url(&server, "/old"))
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.body.as_ref(), b"final");
    assert_eq!(resp.redirects.len(), 1);
    assert_eq!(resp.redirects[0].path(), "/old");
    assert_eq!(resp.url.path(), "/new");
    assert!(resp.permanent_redirect, "301 hops are permanent");
}

#[tokio::test]
async fn temporary_redirect_is_not_reported_as_permanent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(308).insert_header("location", "/b"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/c"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/c"))
        .respond_with(ResponseTemplate::new(200).set_body_string("final"))
        .mount(&server)
        .await;
    let resp = client(&server, Profile::Feed)
        .get(&url(&server, "/a"))
        .await
        .unwrap();
    assert_eq!(resp.redirects.len(), 2);
    assert!(
        !resp.permanent_redirect,
        "a 302 in the chain breaks permanence"
    );
    let direct = client(&server, Profile::Feed)
        .get(&url(&server, "/c"))
        .await
        .unwrap();
    assert!(!direct.permanent_redirect);
}

#[tokio::test]
async fn redirect_to_private_address_is_blocked() {
    let server = MockServer::start().await;
    for (p, target) in [
        ("/to-10", "http://10.0.0.1/secret"),
        ("/to-metadata", "http://169.254.169.254/latest/meta-data/"),
        ("/to-localhost-name", "http://intranet/"),
        ("/to-v6-loop", "http://[::1]/"),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(302).insert_header("location", target))
            .mount(&server)
            .await;
        let err = client(&server, Profile::Feed)
            .get(&url(&server, p))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Policy(_)), "{p}: {err:?}");
    }
}

#[tokio::test]
async fn private_targets_need_the_allow_list() {
    let server = MockServer::start().await;
    let strict = HttpClient::with_policy(Profile::Feed, NetworkPolicy::strict()).unwrap();
    let err = strict.get(&url(&server, "/anything")).await.unwrap_err();
    assert!(matches!(err, HttpError::Policy(_)), "{err:?}");
    // The request never reached the server.
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn trusted_profile_reaches_loopback() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let trusted = HttpClient::new(Profile::Trusted, ClientConfig::default()).unwrap();
    assert!(trusted.get(&url(&server, "/api")).await.is_ok());
}

#[tokio::test]
async fn too_many_redirects() {
    let server = MockServer::start().await;
    for i in 0..8 {
        Mock::given(method("GET"))
            .and(path(format!("/r{i}")))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", format!("/r{}", i + 1)),
            )
            .mount(&server)
            .await;
    }
    let err = client(&server, Profile::Feed)
        .get(&url(&server, "/r0"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, HttpError::TooManyRedirects { limit: 5 }),
        "{err:?}"
    );
}

#[tokio::test]
async fn redirect_without_location_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nowhere"))
        .respond_with(ResponseTemplate::new(302))
        .mount(&server)
        .await;
    let err = client(&server, Profile::Feed)
        .get(&url(&server, "/nowhere"))
        .await
        .unwrap_err();
    assert!(matches!(err, HttpError::BadRedirect(302)), "{err:?}");
}

#[tokio::test]
async fn credentials_stay_on_their_origin() {
    let other_port = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/other"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&other_port)
        .await;
    let server = MockServer::start().await;
    // 127.0.0.1 → localhost is a host change even though it is the same server.
    let other_host = format!("http://localhost:{}/other", server.address().port());
    Mock::given(method("GET"))
        .and(path("/other"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    for (start, target) in [
        ("/host", other_host),
        ("/port", url(&other_port, "/other").to_string()),
    ] {
        Mock::given(method("GET"))
            .and(path(start))
            .respond_with(ResponseTemplate::new(307).insert_header("location", target.as_str()))
            .mount(&server)
            .await;
    }
    let opts = GetOptions {
        headers: vec![
            (
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer secret"),
            ),
            (
                HeaderName::from_static("x-auth-key"),
                HeaderValue::from_static("provider key"),
            ),
        ],
        ..GetOptions::default()
    };
    let client = client(&server, Profile::Discovery);
    for start in ["/host", "/port"] {
        client.get_with(&url(&server, start), &opts).await.unwrap();
    }
    let mut reqs = server.received_requests().await.unwrap();
    reqs.extend(other_port.received_requests().await.unwrap());
    let (starts, others): (Vec<_>, Vec<_>) = reqs.iter().partition(|r| r.url.path() != "/other");
    assert_eq!((starts.len(), others.len()), (2, 2));
    for credential in ["authorization", "x-auth-key"] {
        assert!(starts.iter().all(|r| r.headers.get(credential).is_some()));
        for other in &others {
            assert!(
                other.headers.get(credential).is_none(),
                "{credential} went to {}",
                other.url
            );
        }
    }
}

#[tokio::test]
async fn the_body_cap_holds_both_ways() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 10_000]))
        .mount(&server)
        .await;
    let c = client(&server, Profile::Discovery);
    let opts = GetOptions {
        max_bytes: Some(1000),
        ..GetOptions::default()
    };
    let err = c.get_with(&url(&server, "/big"), &opts).await.unwrap_err();
    assert!(
        matches!(err, HttpError::BodyTooLarge { limit: 1000 }),
        "{err:?}"
    );
    let ok = c
        .get_with(
            &url(&server, "/big"),
            &GetOptions {
                max_bytes: Some(10_000),
                ..GetOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ok.body.len(), 10_000);
}

#[tokio::test]
async fn retries_on_503_then_succeeds() {
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
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .with_priority(5)
        .mount(&server)
        .await;
    let resp = client(&server, Profile::Discovery)
        .get(&url(&server, "/flaky"))
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.attempts, 2);
}

#[tokio::test]
async fn retry_after_honoured_then_dropped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/limited"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .mount(&server)
        .await;
    let resp = client(&server, Profile::Discovery)
        .get(&url(&server, "/limited"))
        .await
        .unwrap();
    assert_eq!(resp.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.attempts, 3, "default policy makes three attempts");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/down"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let c = client(&server, Profile::Discovery);
    let r = c.get(&url(&server, "/missing")).await.unwrap();
    assert_eq!((r.status.as_u16(), r.attempts), (404, 1));
    let r = c
        .get_with(
            &url(&server, "/down"),
            &GetOptions {
                retry: Some(false),
                ..GetOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((r.status.as_u16(), r.attempts), (500, 1));
}

#[tokio::test]
async fn cancellation_stops_a_slow_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let cancel = CancellationToken::new();
    let opts = GetOptions {
        cancel: Some(cancel.clone()),
        ..GetOptions::default()
    };
    let c = client(&server, Profile::Discovery);
    let handle = tokio::spawn(async move { c.get_with(&url(&server, "/slow"), &opts).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    let started = std::time::Instant::now();
    let err = handle.await.unwrap().unwrap_err();
    assert!(matches!(err, HttpError::Cancelled), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn timeout_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stall"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let c = HttpClient::new(
        Profile::Discovery,
        ClientConfig {
            policy: std::sync::Arc::new(policy_for(&server)),
            request_timeout: Duration::from_millis(200),
            retry: RetryPolicy::none(),
            ..ClientConfig::default()
        },
    )
    .unwrap();
    let err = c.get(&url(&server, "/stall")).await.unwrap_err();
    assert!(matches!(err, HttpError::Timeout(_)), "{err:?}");
}

#[tokio::test]
async fn conditional_headers_are_sent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed"))
        .and(header("if-none-match", "\"abc\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let opts = GetOptions {
        conditional: Some(uguisu_http::Conditional {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".into()),
        }),
        ..GetOptions::default()
    };
    let r = client(&server, Profile::Feed)
        .get_with(&url(&server, "/feed"), &opts)
        .await
        .unwrap();
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn gzip_bodies_are_decompressed() {
    use std::io::Write;
    let server = MockServer::start().await;
    let mut enc = flate2_lite::GzEncoder::new(Vec::new());
    enc.write_all(b"<rss/>").unwrap();
    let gz = enc.finish().unwrap();
    Mock::given(method("GET"))
        .and(path("/gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(gz),
        )
        .mount(&server)
        .await;
    let r = client(&server, Profile::Feed)
        .get(&url(&server, "/gz"))
        .await
        .unwrap();
    assert_eq!(r.body.as_ref(), b"<rss/>");
}

/// Minimal gzip writer so the test does not need another dependency.
#[allow(clippy::cast_possible_truncation, clippy::unnecessary_wraps)]
mod flate2_lite {
    use std::io::{self, Write};

    pub struct GzEncoder {
        out: Vec<u8>,
        crc: u32,
        len: u32,
        data: Vec<u8>,
    }

    impl GzEncoder {
        pub fn new(out: Vec<u8>) -> Self {
            Self {
                out,
                crc: 0xFFFF_FFFF,
                len: 0,
                data: Vec::new(),
            }
        }

        pub fn finish(mut self) -> io::Result<Vec<u8>> {
            // header
            self.out
                .extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
            // stored (uncompressed) deflate block
            let n = self.data.len() as u16;
            self.out.push(1);
            self.out.extend_from_slice(&n.to_le_bytes());
            self.out.extend_from_slice(&(!n).to_le_bytes());
            self.out.extend_from_slice(&self.data);
            self.out.extend_from_slice(&(!self.crc).to_le_bytes());
            self.out.extend_from_slice(&self.len.to_le_bytes());
            Ok(self.out)
        }
    }

    impl Write for GzEncoder {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            for &b in buf {
                let mut c = self.crc ^ u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
                self.crc = c;
            }
            self.len += buf.len() as u32;
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
