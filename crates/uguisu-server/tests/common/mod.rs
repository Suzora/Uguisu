//! One harness for the server's integration tests.
//!
//! Every test file used to declare its own `App` and `call`, and that `call`
//! returned `(StatusCode, Value)`: it threw the headers away and turned a body
//! that was not JSON into `Value::Null`. That is precisely why plain-text
//! extractor rejections went unnoticed for two phases. This one keeps the
//! status, the headers and the bytes, and carries a cookie jar, because a
//! session cannot be tested without one.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uguisu_core::config::{Config, DataConfig};
use uguisu_engine::{Engine, EngineConfig};
use uguisu_server::{AppState, router};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A server under test, with the temporary directory it lives in.
pub struct App {
    pub router: Router,
    pub server: MockServer,
    pub engine: Engine,
    pub dir: tempfile::TempDir,
    /// Sent on every request once a login has set it.
    cookie: Option<String>,
    /// Sent on every mutating request once a login has returned it.
    csrf: Option<String>,
    /// Sent instead of a cookie once a token has been presented.
    bearer: Option<String>,
}

/// How the server should be built.
#[derive(Debug, Default, Clone, Copy)]
pub struct Options {
    /// Set a password before the router is built, so authentication is on.
    pub credential: bool,
    /// Add `Secure` to the session cookie.
    pub cookie_secure: bool,
    /// Serve media from the data directory's `media` subdirectory.
    pub media: bool,
    /// A reverse proxy whose `X-Forwarded-For` is believed (ADR 0062).
    pub trusted_proxy: Option<&'static str>,
}

impl App {
    /// A server with no credential: authentication is off, as on a fresh
    /// loopback install.
    pub async fn open() -> Self {
        Self::with(Options::default()).await
    }

    /// A server with a credential, so every route is behind authentication.
    pub async fn secured() -> Self {
        Self::with(Options {
            credential: true,
            ..Options::default()
        })
        .await
    }

    pub async fn with(options: Options) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let mut config = Config::from_lookup(|k| match k {
            "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
            "UGUISU_DISCOVERY_APPLE_ENABLED" => Some("false".into()),
            _ => None,
        })
        .unwrap();
        config.data = DataConfig {
            data_dir: Some(dir.path().to_path_buf()),
            media_dir: None,
        };
        let engine = Engine::open(EngineConfig::from(config)).await.unwrap();
        if options.credential {
            engine
                .set_password(
                    "uguisu",
                    &uguisu_core::secret::Secret::new(PASSWORD.to_owned()),
                    None,
                )
                .await
                .unwrap();
        }
        let state = AppState::new(engine.discovery().clone(), Some(engine.clone())).with_auth(
            uguisu_server::auth::AuthOptions {
                required: options.credential,
                cookie_secure: options.cookie_secure,
                allow_insecure_exposure: false,
                trusted_proxies: options
                    .trusted_proxy
                    .map(|p| p.parse().unwrap())
                    .into_iter()
                    .collect(),
            },
        );
        Self {
            router: router(state),
            server,
            engine,
            dir,
            cookie: None,
            csrf: None,
            bearer: None,
        }
    }

    /// Where media lands.
    pub fn media_root(&self) -> std::path::PathBuf {
        self.engine.config().data.media_dir().unwrap()
    }

    /// Logs in as the operator and keeps the cookie and CSRF token.
    pub async fn login(&mut self) -> Reply {
        let reply = self
            .send(
                "POST",
                "/api/v1/auth/login",
                Some(serde_json::json!({ "username": "uguisu", "password": PASSWORD })),
            )
            .await;
        if let Some(cookie) = reply.set_cookie() {
            self.cookie = Some(cookie.to_owned());
        }
        if let Some(csrf) = reply.json["csrf_token"].as_str() {
            self.csrf = Some(csrf.to_owned());
        }
        reply
    }

    /// Presents this bearer token instead of a cookie from now on.
    pub fn bearing(&mut self, secret: &str) {
        self.cookie = None;
        self.csrf = None;
        self.bearer = Some(secret.to_owned());
    }

    /// Forgets every credential, so the next request is anonymous.
    pub fn anonymous(&mut self) {
        self.cookie = None;
        self.csrf = None;
        self.bearer = None;
    }

    /// Sends a request, attaching whatever credentials are held.
    pub async fn send(&self, method: &str, uri: &str, body: Option<serde_json::Value>) -> Reply {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(cookie) = &self.cookie {
            headers.push((header::COOKIE.to_string(), cookie.clone()));
        }
        if let Some(bearer) = &self.bearer {
            headers.push((
                header::AUTHORIZATION.to_string(),
                format!("Bearer {bearer}"),
            ));
        }
        if let Some(csrf) = self
            .csrf
            .as_ref()
            .filter(|_| method != "GET" && method != "HEAD")
        {
            headers.push(("x-uguisu-csrf".to_owned(), csrf.clone()));
        }
        raw(&self.router, method, uri, &headers, body).await
    }

    /// Sends a request that appears to arrive from `peer`, with whatever
    /// credentials are held.
    pub async fn send_from(
        &self,
        peer: std::net::SocketAddr,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> Reply {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(bearer) = &self.bearer {
            headers.push((
                header::AUTHORIZATION.to_string(),
                format!("Bearer {bearer}"),
            ));
        }
        raw_from(&self.router, peer, method, uri, &headers, body).await
    }

    /// Sends a request with no credentials, whatever is held.
    pub async fn send_anonymous(
        &self,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> Reply {
        raw(&self.router, method, uri, &[], body).await
    }

    /// Sends a mutating request without the CSRF header.
    pub async fn send_without_csrf(
        &self,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> Reply {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(cookie) = &self.cookie {
            headers.push((header::COOKIE.to_string(), cookie.clone()));
        }
        raw(&self.router, method, uri, &headers, body).await
    }

    /// Serves `body` at `p` on the mock feed host.
    pub async fn serve(&self, p: &str, body: &str) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(body),
            )
            .mount(&self.server)
            .await;
    }

    pub async fn close(self) {
        self.engine.close().await;
    }
}

/// The password the harness sets, long enough for the engine to accept it.
pub const PASSWORD: &str = "correct horse battery";

/// One answer, with everything a test might need to look at.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
    /// The body parsed as JSON, or `Null` when it is not JSON. Tests that care
    /// about the difference read `bytes`.
    pub json: serde_json::Value,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The first `Set-Cookie`, which is the only one Uguisu sends.
    pub fn set_cookie(&self) -> Option<&str> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with(uguisu_server::auth::COOKIE))
    }

    /// The cookie's value alone, without its attributes.
    pub fn cookie_value(&self) -> Option<&str> {
        self.set_cookie()?
            .split(';')
            .next()?
            .split_once('=')
            .map(|(_, v)| v)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Sends one request with exactly the headers given.
pub async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Option<serde_json::Value>,
) -> Reply {
    match body {
        Some(value) => {
            let mut with_type: Vec<(String, String)> = headers.to_vec();
            with_type.push((
                header::CONTENT_TYPE.to_string(),
                "application/json".to_owned(),
            ));
            send_body(app, method, uri, &with_type, Body::from(value.to_string())).await
        }
        None => send_body(app, method, uri, headers, Body::empty()).await,
    }
}

/// Sends a body verbatim, so a test can send one that is not JSON at all and
/// set (or leave out) the content type itself.
pub async fn raw_text(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: &'static str,
) -> Reply {
    send_body(app, method, uri, headers, Body::from(body)).await
}

/// The peer a request appears to come from unless a test says otherwise.
///
/// `oneshot` carries no socket, so without this every request looks like it
/// arrived from `0.0.0.0` — which is neither what a real client looks like nor
/// what the routes that care about loopback should be tested against.
pub const LOOPBACK: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 51_000);

/// Sends a request that appears to come from `peer`.
pub async fn raw_from(
    app: &Router,
    peer: std::net::SocketAddr,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Option<serde_json::Value>,
) -> Reply {
    let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    send_from(app, peer, method, uri, headers, body).await
}

async fn send_body(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Body,
) -> Reply {
    send_from(app, LOOPBACK, method, uri, headers, body).await
}

async fn send_from(
    app: &Router,
    peer: std::net::SocketAddr,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Body,
) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(name, value);
    }
    let req = req.extension(axum::extract::ConnectInfo(peer));
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Reply {
        status,
        headers,
        json: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        bytes,
    }
}
