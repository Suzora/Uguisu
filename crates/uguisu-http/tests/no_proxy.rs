//! A proxy named in the environment is never used. Through one, the proxy
//! resolves the host, and the resolver hook that returns only
//! policy-approved addresses (`docs/SECURITY.md` §3.1) would not be asked.
//!
//! The proxy variables are set only for a child process running this same
//! binary, since no test may change its own process's environment.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use uguisu_http::{ClientConfig, GetOptions, HttpClient, NetworkPolicy, Profile, Url};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const TARGET: &str = "UGUISU_TEST_PROXY_TARGET";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_proxy_is_ignored() {
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
        .mount(&target)
        .await;
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("through the proxy"))
        .mount(&proxy)
        .await;

    let (target_uri, proxy_uri) = (target.uri(), proxy.uri());
    // Blocking, off the runtime that serves the two mock servers.
    let child = tokio::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fetch_with_the_environment", "--nocapture"])
            .env(TARGET, target_uri)
            .env("HTTP_PROXY", &proxy_uri)
            .env("http_proxy", &proxy_uri)
            .env("ALL_PROXY", &proxy_uri)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let out = String::from_utf8_lossy(&child.stdout);
    assert!(
        child.status.success(),
        "{out}{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(out.contains("1 passed"), "the child ran nothing: {out}");
    assert!(proxy.received_requests().await.unwrap().is_empty());
}

/// Runs only in the child `environment_proxy_is_ignored` starts.
#[tokio::test]
async fn fetch_with_the_environment() {
    let Ok(target) = std::env::var(TARGET) else {
        return;
    };
    let client = HttpClient::new(
        Profile::Feed,
        ClientConfig {
            policy: std::sync::Arc::new(
                NetworkPolicy::strict().allow_private_hosts(["127.0.0.1", "localhost"]),
            ),
            ..ClientConfig::default()
        },
    )
    .unwrap();
    let url = Url::parse(&format!("{target}/feed.xml")).unwrap();
    let response = client.get_with(&url, &GetOptions::default()).await.unwrap();
    assert_eq!(&response.body[..], b"direct");
}
