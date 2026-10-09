//! The Uguisu HTTP server: the REST API under `/api/v1`. The contract is
//! `docs/API.md`, and `docs/api/openapi.json` is generated from these
//! handlers.
//!
//! | Module | Routes |
//! |---|---|
//! | here | `/health`, `/discovery/{search,providers,resolve}` |
//! | `auth.rs` | `/auth/*`, and the middleware every route passes (ADR 0035) |
//! | `library.rs` | `/podcasts/*`, `/episodes/*`, `/feeds/*`, `/events` |
//! | `downloads.rs` | `/downloads/*`, `/podcasts/{id}/downloads` |
//! | `archive.rs`, `archive_assets.rs` | `/archive/*` |
//! | `service.rs` | `/status`, `/scheduler/*`, `/settings/*`, `/search`, `/db/*`, `/discovery/records`, a podcast's place in the schedule |
//! | `bytes.rs` | `/archive/{id}/media`, `/podcasts/{id}/artwork/image`, `Range` included |
//!
//! Everything outside `/api` is the built web UI when one is configured
//! (`web.rs`, ADR 0031).
//!
//! Errors are `{ "error": { "kind": …, "message": … }, "schema": 1 }`
//! (ADR 0038).

mod archive;
mod archive_assets;
pub mod auth;
mod bytes;
mod downloads;
pub mod expose;
mod extract;
mod headers;
mod library;
pub mod openapi;
mod service;
mod web;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uguisu_discovery::{Discovery, ProviderId, ResolveError, ResolveFailure, SearchRequest};

use extract::{Body, Params};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

pub use library::{FeedStatus, status_for};

/// The default address the server binds to outside Docker (ADR 0013).
pub const DEFAULT_BIND: &str = "127.0.0.1:8484";

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// The discovery stack.
    pub discovery: Arc<Discovery>,
    /// The library and feed engine; `None` runs a discovery-only server.
    pub engine: Option<uguisu_engine::Engine>,
    /// Directory holding the built web UI; `None` serves the API only.
    pub web: Option<PathBuf>,
    /// The authentication layer's state.
    pub auth: Arc<auth::AuthState>,
}

impl AppState {
    /// Wraps a discovery stack and, when a data directory is open, the engine.
    ///
    /// Authentication starts off, which is what a server with no credential
    /// means; [`AppState::with_auth`] turns it on.
    pub fn new(discovery: Discovery, engine: Option<uguisu_engine::Engine>) -> Self {
        Self {
            discovery: Arc::new(discovery),
            engine,
            web: None,
            auth: Arc::new(auth::AuthState::new(auth::AuthOptions::default())),
        }
    }

    /// Sets how the authentication layer behaves.
    #[must_use]
    pub fn with_auth(mut self, options: auth::AuthOptions) -> Self {
        self.auth = Arc::new(auth::AuthState::new(options));
        self
    }

    /// Serves the built web UI from `dir`. A directory that does not exist
    /// is not an error: the API stays up and the UI answers `404`.
    #[must_use]
    pub fn with_web_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.web = dir;
        self
    }
}

/// Body of `GET /api/v1/health`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Health {
    /// Always `"ok"` when the process can answer at all.
    pub status: &'static str,
    /// Uguisu version.
    pub version: &'static str,
}

/// JSON error body: the shape every failure answers with.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiError {
    /// Schema version, as on every success body.
    pub schema: u32,
    /// The error.
    pub error: ApiErrorBody,
}

/// Inner error object.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiErrorBody {
    /// Stable kind.
    pub kind: &'static str,
    /// Message.
    pub message: String,
}

impl ApiError {
    pub(crate) fn new(
        status: StatusCode,
        kind: &'static str,
        message: impl Into<String>,
    ) -> (StatusCode, Json<Self>) {
        (
            status,
            Json(Self {
                schema: library::SCHEMA,
                error: ApiErrorBody {
                    kind,
                    message: message.into(),
                },
            }),
        )
    }
}

/// Builds the application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/discovery/search", get(search))
        .route("/api/v1/discovery/providers", get(providers))
        .route(
            "/api/v1/discovery/resolve",
            get(resolve_get).post(resolve_post),
        )
        .merge(auth::routes())
        .merge(library::routes())
        .merge(downloads::routes())
        .merge(archive::routes())
        .merge(archive_assets::routes())
        .merge(service::routes())
        .fallback(web::fallback)
        .method_not_allowed_fallback(extract::method_not_allowed)
        // The layer wraps the fallbacks too, which is what makes an
        // unauthenticated probe of an unknown path a 401 rather than a map of
        // what exists.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        // Outside the auth layer, so a refusal carries them too.
        .layer(axum::middleware::map_response(headers::secure))
        .with_state(state)
}

/// How long in-flight requests get to finish once shutdown is asked for.
///
/// A bound rather than a wait, because the web UI holds `/api/v1/events` open
/// for as long as a tab is, and that stream ends only when its subscription
/// does — which is part of what `Engine::close` tears down, after serving has
/// returned. An unbounded drain would therefore wait for something that is
/// waiting for it. Abandoning a connection here does not cancel it: its task
/// runs on, and every write path is finished by the engine's own shutdown.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(2);

/// A listener past the exposure gate, not yet serving.
///
/// Binding and serving are separate so an embedder can ask for port 0 and read
/// the port back before it points a window at the server.
#[derive(Debug)]
pub struct Bound {
    listener: tokio::net::TcpListener,
    addr: SocketAddr,
}

impl Bound {
    /// The address that was bound — the real port when 0 was asked for.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Serves until `shutdown` resolves, then stops accepting connections and
    /// gives in-flight requests [`DRAIN`] to finish.
    pub async fn serve<F>(self, state: AppState, shutdown: F) -> Result<(), expose::ServeError>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let (stopping, stopped) = tokio::sync::oneshot::channel::<()>();
        let shutdown = async move {
            shutdown.await;
            let _ = stopping.send(());
        };
        // `into_make_service_with_connect_info` is what puts the peer address in
        // the request, which the login limiter buckets on. Without it every
        // attempt would share one bucket — which is what the in-process tests get,
        // deliberately, since they have no socket.
        let serving = axum::serve(
            self.listener,
            router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown);
        let drained = async {
            // The sender is dropped unsent only with the runtime.
            if stopped.await.is_ok() {
                tokio::time::sleep(DRAIN).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            result = serving => result?,
            () = drained => tracing::debug!("stopped serving with a connection still open"),
        }
        Ok(())
    }
}

/// Runs the exposure gate, then binds `addr`.
///
/// Refuses a network address when no credential exists, unless the deployment
/// said to (ADR 0037). The check is here rather than in the caller so that no
/// embedder can skip it, and it runs *before* the listener, so a refused server
/// never accepts a connection at all.
pub async fn bind(addr: SocketAddr, state: &AppState) -> Result<Bound, expose::ServeError> {
    expose::check(addr, state.auth.required(), state.auth.allow_insecure())?;
    if !state.auth.required() && !expose::loopback(addr) {
        tracing::warn!(
            %addr,
            auth = "disabled",
            insecure_override = true,
            "serving an unauthenticated server on a network address"
        );
    } else if !state.auth.required() {
        // The accepted DNS-rebinding residual (ADR 0058) is closed only by a password.
        tracing::warn!(
            %addr,
            auth = "disabled",
            "serving without a password; any website can reach this server through DNS rebinding"
        );
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // The bound address, not the requested one: port 0 is a real port by now.
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "uguisu server listening");
    Ok(Bound { listener, addr })
}

/// Serves the router on `addr` until the process is killed.
pub async fn serve(addr: SocketAddr, state: AppState) -> Result<(), expose::ServeError> {
    serve_with_shutdown(addr, state, std::future::pending()).await
}

/// Serves the router on `addr` until `shutdown` resolves.
pub async fn serve_with_shutdown<F>(
    addr: SocketAddr,
    state: AppState,
    shutdown: F,
) -> Result<(), expose::ServeError>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    bind(addr, &state).await?.serve(state, shutdown).await
}

#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "service",
    security(),
    responses((status = 200, description = "The process can answer", body = Health))
)]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: uguisu_core::VERSION,
    })
}

/// Query parameters of the search endpoint.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchParams {
    /// Query text.
    pub q: Option<String>,
    /// Comma-separated provider ids.
    pub providers: Option<String>,
    /// Result limit.
    pub limit: Option<usize>,
    /// Storefront country.
    pub country: Option<String>,
    /// Bypass the cache.
    #[serde(default)]
    pub no_cache: bool,
    /// Stream snapshots as Server-Sent Events.
    #[serde(default)]
    pub stream: bool,
}

fn parse_request(params: &SearchParams) -> Result<SearchRequest, (StatusCode, Json<ApiError>)> {
    let query = params
        .q
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    if query.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "query parameter `q` is required",
        ));
    }
    let providers = match &params.providers {
        Some(list) if !list.trim().is_empty() => {
            let mut ids = Vec::new();
            for name in list.split(',') {
                let id: ProviderId =
                    name.trim()
                        .parse()
                        .map_err(|e: uguisu_core::provider::UnknownProvider| {
                            ApiError::new(StatusCode::BAD_REQUEST, "invalid", e.to_string())
                        })?;
                ids.push(id);
            }
            Some(ids)
        }
        _ => None,
    };
    Ok(SearchRequest {
        query,
        providers,
        limit: params.limit,
        country: params.country.clone(),
        no_cache: params.no_cache,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/discovery/search",
    tag = "discovery",
    operation_id = "discovery_search",
    params(SearchParams),
    responses(
        (status = 200, description = "One snapshot, or an SSE stream of them when `stream=true`", body = uguisu_discovery::SearchResponse),
        (status = 400, body = ApiError)
    )
)]
pub(crate) async fn search(
    State(state): State<AppState>,
    Params(params): Params<SearchParams>,
) -> Response {
    let request = match parse_request(&params) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let cancel = CancellationToken::new();
    if params.stream {
        let rx = state.discovery.engine.search_stream(request, cancel);
        let stream = ReceiverStream::new(rx).map(|snapshot| {
            let event_name = if snapshot.complete {
                "complete"
            } else {
                "snapshot"
            };
            Ok::<Event, Infallible>(
                Event::default()
                    .event(event_name)
                    .json_data(&snapshot)
                    .unwrap_or_else(|_| {
                        Event::default().event("error").data("serialization failed")
                    }),
            )
        });
        return Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }
    let response = state.discovery.engine.search(&request, cancel).await;
    Json(response).into_response()
}

/// Body of `GET /api/v1/discovery/providers`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProvidersResponse {
    /// Every registered provider with status and health.
    pub providers: Vec<uguisu_discovery::ProviderStatus>,
    /// Cache counters.
    pub cache: uguisu_discovery::CacheStats,
}

#[utoipa::path(
    get,
    path = "/api/v1/discovery/providers",
    tag = "discovery",
    responses((status = 200, body = ProvidersResponse))
)]
pub(crate) async fn providers(State(state): State<AppState>) -> Json<ProvidersResponse> {
    let registry = &state.discovery.registry;
    Json(ProvidersResponse {
        providers: registry.status(),
        cache: registry.cache().stats(),
    })
}

/// Body of `POST /api/v1/discovery/resolve`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResolveBody {
    /// Feed URL, website URL or directory page URL.
    pub input: String,
}

/// Query of `GET /api/v1/discovery/resolve`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ResolveParams {
    /// Feed URL, website URL or directory page URL.
    pub input: Option<String>,
}

/// Failure body of `/discovery/resolve`: the standard envelope, plus the two
/// things only this route has.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResolveFailureBody {
    /// Schema version, as on every other body.
    pub schema: u32,
    /// The error.
    pub error: ResolveErrorBody,
    /// Steps taken, in order.
    pub provenance: Vec<uguisu_discovery::ResolutionStep>,
}

/// The `error` object of a resolution failure.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResolveErrorBody {
    /// Stable kind, from the same place `kind` comes from on every failure.
    pub kind: &'static str,
    /// Message.
    pub message: String,
    /// What to try next.
    pub suggestion: &'static str,
    /// The failure with its own fields, for a client that renders them — the
    /// CLI lists the URLs a `no_feed_link_found` tried, which `message` cannot
    /// carry.
    pub detail: ResolveError,
}

/// Resolution has its own status table because it has its own error type:
/// `ResolveError` is not an `UguisuError`, and the two disagree on purpose —
/// `POST /podcasts` reports the same underlying failures as `unresolvable`,
/// because "add this" and "tell me what this is" fail differently.
fn failure_status(error: &ResolveError) -> StatusCode {
    match error {
        ResolveError::NotAUrl { .. } => StatusCode::BAD_REQUEST,
        ResolveError::BlockedByPolicy { .. } => StatusCode::FORBIDDEN,
        ResolveError::NotAFeed { .. }
        | ResolveError::NoFeedLinkFound { .. }
        | ResolveError::FeedInvalid { .. }
        | ResolveError::NoFeedAvailable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        ResolveError::HttpStatus { .. }
        | ResolveError::Network { .. }
        | ResolveError::ProviderUnavailable { .. } => StatusCode::BAD_GATEWAY,
        ResolveError::BudgetExceeded { .. } => StatusCode::GATEWAY_TIMEOUT,
        ResolveError::Cancelled => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn do_resolve(state: &AppState, input: &str) -> Response {
    let input = input.trim();
    if input.is_empty() {
        return ApiError::new(StatusCode::BAD_REQUEST, "invalid", "`input` is required")
            .into_response();
    }
    match state
        .discovery
        .resolver
        .resolve(input, CancellationToken::new())
        .await
    {
        Ok(feed) => Json(feed).into_response(),
        Err(ResolveFailure { error, provenance }) => {
            let status = failure_status(&error);
            let body = ResolveFailureBody {
                schema: library::SCHEMA,
                error: ResolveErrorBody {
                    kind: error.kind(),
                    message: error.to_string(),
                    suggestion: error.suggestion(),
                    detail: error,
                },
                provenance,
            };
            (status, Json(body)).into_response()
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/discovery/resolve",
    tag = "discovery",
    request_body = ResolveBody,
    responses(
        (status = 200, body = uguisu_discovery::ResolvedFeed),
        (status = 400, description = "Not a URL, with the steps taken; an empty `input` answers the plain error envelope", body = ResolveFailureBody),
        (status = 403, description = "Blocked by the network policy", body = ResolveFailureBody),
        (status = 422, description = "Not resolvable, with the steps taken", body = ResolveFailureBody),
        (status = 500, description = "Cancelled", body = ResolveFailureBody),
        (status = 502, description = "The origin or a provider failed", body = ResolveFailureBody),
        (status = 504, description = "The resolution budget ran out", body = ResolveFailureBody)
    )
)]
pub(crate) async fn resolve_post(
    State(state): State<AppState>,
    Body(body): Body<ResolveBody>,
) -> Response {
    do_resolve(&state, &body.input).await
}

#[utoipa::path(
    get,
    path = "/api/v1/discovery/resolve",
    tag = "discovery",
    params(ResolveParams),
    responses(
        (status = 200, body = uguisu_discovery::ResolvedFeed),
        (status = 400, description = "Not a URL, with the steps taken; an empty `input` answers the plain error envelope", body = ResolveFailureBody),
        (status = 403, description = "Blocked by the network policy", body = ResolveFailureBody),
        (status = 422, description = "Not resolvable, with the steps taken", body = ResolveFailureBody),
        (status = 500, description = "Cancelled", body = ResolveFailureBody),
        (status = 502, description = "The origin or a provider failed", body = ResolveFailureBody),
        (status = 504, description = "The resolution budget ran out", body = ResolveFailureBody)
    )
)]
pub(crate) async fn resolve_get(
    State(state): State<AppState>,
    Params(params): Params<ResolveParams>,
) -> Response {
    do_resolve(&state, params.input.as_deref().unwrap_or_default()).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use uguisu_core::config::DiscoveryConfig;
    use uguisu_discovery::testing::Fixture;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const FEED: &str = include_str!("../../../tests/fixtures/feeds/probe/rss_itunes_podcast.xml");

    async fn app() -> (Router, MockServer) {
        let server = MockServer::start().await;
        Fixture::load("apple", "search_darknet")
            .unwrap()
            .mount(&server)
            .await;
        Fixture::load("apple", "search_empty")
            .unwrap()
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(FEED),
            )
            .mount(&server)
            .await;
        let uri = server.uri();
        let cfg = DiscoveryConfig::from_lookup(|k| match k {
            "UGUISU_DISCOVERY_APPLE_BASE_URL" => Some(uri.clone()),
            "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS" => Some("127.0.0.1".into()),
            "UGUISU_DISCOVERY_SOFT_DEADLINE_MS" => Some("100".into()),
            // Well above the 3 s Apple's throttle puts between two searches,
            // which the second search in a test waits out.
            "UGUISU_DISCOVERY_HARD_DEADLINE_MS" => Some("10000".into()),
            _ => None,
        })
        .unwrap();
        let discovery = uguisu_discovery::assemble(cfg).unwrap();
        (router(AppState::new(discovery, None)), server)
    }

    async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// Discovery-only state: enough to bind, with no engine and no credential.
    fn bare_state() -> AppState {
        let cfg = DiscoveryConfig::from_lookup(|_| None).unwrap();
        AppState::new(uguisu_discovery::assemble(cfg).unwrap(), None)
    }

    #[tokio::test]
    async fn port_zero_reports_the_port_it_got() {
        let bound = bind("127.0.0.1:0".parse().unwrap(), &bare_state())
            .await
            .unwrap();
        let addr = bound.addr();
        assert_ne!(
            addr.port(),
            0,
            "an embedder cannot point a window at port 0"
        );
        assert!(expose::loopback(addr), "asked for loopback, got {addr}");
    }

    #[tokio::test]
    async fn the_gate_runs_before_the_listener() {
        // An exposed bind with no credential must be refused without ever
        // holding the socket, or a refused server would still have taken it.
        let addr = "0.0.0.0:0".parse().unwrap();
        let err = bind(addr, &bare_state()).await.unwrap_err();
        assert!(
            matches!(err, expose::ServeError::InsecureExposure { .. }),
            "expected a refusal, got {err:?}"
        );
    }

    #[tokio::test]
    async fn stops_with_an_open_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = tempfile::tempdir().unwrap();
        let mut config = uguisu_core::config::Config::from_lookup(|_| None).unwrap();
        config.data.data_dir = Some(dir.path().to_path_buf());
        let engine = uguisu_engine::Engine::open(config).await.unwrap();
        let state = AppState::new(engine.discovery().clone(), Some(engine.clone()));
        let bound = bind("127.0.0.1:0".parse().unwrap(), &state).await.unwrap();
        let addr = bound.addr();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(bound.serve(state, async {
            let _ = stopped.await;
        }));

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut head = [0; 64];
        let read = stream.read(&mut head).await.unwrap();
        let head = String::from_utf8_lossy(&head[..read]);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");

        stop.send(()).unwrap();
        let stopped = tokio::time::timeout(2 * DRAIN, serving).await;
        engine.close().await;
        assert!(
            matches!(stopped, Ok(Ok(Ok(())))),
            "the stream held the stop: {stopped:?}"
        );
    }

    #[tokio::test]
    async fn health_reports_ok_and_version() {
        let (app, _server) = app().await;
        let (status, json) = get_json(&app, "/api/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], uguisu_core::VERSION);
    }

    #[tokio::test]
    async fn search_json() {
        let (app, _server) = app().await;
        let (status, json) = get_json(&app, "/api/v1/discovery/search?q=darknet%20diaries").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["outcome"], "results");
        assert_eq!(json["results"][0]["candidate"]["title"], "Darknet Diaries");
        assert!(json["complete"].as_bool().unwrap());
        let (status, json) = get_json(
            &app,
            "/api/v1/discovery/search?q=zzqqxxnothing&providers=apple&limit=3",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["outcome"], "no_results");
        let (status, json) = get_json(&app, "/api/v1/discovery/search").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["kind"], "invalid");
        let (status, _) = get_json(&app, "/api/v1/discovery/search?q=x&providers=spotify").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_stream_emits_sse_events() {
        let (app, _server) = app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/discovery/search?q=darknet%20diaries&stream=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("event: complete"), "{text}");
        assert!(text.contains("\"outcome\":\"results\""), "{text}");
    }

    #[tokio::test]
    async fn providers_status() {
        let (app, _server) = app().await;
        let (status, json) = get_json(&app, "/api/v1/discovery/providers").await;
        assert_eq!(status, StatusCode::OK);
        let providers = json["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 3);
        let apple = providers.iter().find(|p| p["id"] == "apple").unwrap();
        assert_eq!(apple["enabled"], true);
        assert!(apple["attribution"].as_str().unwrap().contains("Apple"));
        let pi = providers
            .iter()
            .find(|p| p["id"] == "podcastindex")
            .unwrap();
        assert_eq!(pi["enabled"], false);
        assert!(
            pi["disabled_reason"]
                .as_str()
                .unwrap()
                .contains("credentials")
        );
        assert!(json["cache"]["entries"].is_number());
    }

    #[tokio::test]
    async fn resolve_post_and_get_with_error_mapping() {
        let (app, server) = app().await;
        let feed = format!("{}/feed.xml", server.uri());
        let body = serde_json::json!({ "input": feed }).to_string();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/discovery/resolve")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["title"], "Example Diaries");

        let (status, json) = get_json(
            &app,
            &format!("/api/v1/discovery/resolve?input={}", urlencode(&feed)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items_with_media"], 3);

        let (status, json) =
            get_json(&app, "/api/v1/discovery/resolve?input=darknet%20diaries").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["kind"], "not_a_url");
        assert_eq!(json["schema"], library::SCHEMA);
        let (status, json) = get_json(
            &app,
            "/api/v1/discovery/resolve?input=https://open.spotify.com/show/abc",
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["error"]["kind"], "no_feed_available");
        assert!(
            json["error"]["suggestion"]
                .as_str()
                .unwrap()
                .contains("search")
        );
        // The tagged failure survives beside the envelope: the CLI lists
        // fields `message` cannot carry.
        assert!(json["error"]["detail"].is_object(), "{json}");
        let (status, json) =
            get_json(&app, "/api/v1/discovery/resolve?input=http://10.0.0.1/feed").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["error"]["kind"], "blocked_by_policy");
        let (status, _) = get_json(&app, "/api/v1/discovery/resolve").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    fn urlencode(s: &str) -> String {
        s.replace(':', "%3A").replace('/', "%2F")
    }
}
