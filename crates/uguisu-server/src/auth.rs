//! Who may ask what, and the routes that decide it (ADR 0035, ADR 0036).
//!
//! Authorization is one classifier plus one middleware, rather than an
//! extractor named in every handler. Sixty handlers would each have to
//! remember, and handler sixty-one would not; here the rule is "anything not
//! on the public list is authenticated", the layer wraps the router's fallback
//! too, and an unauthenticated probe of an unknown `/api/` path is answered
//! `401` rather than `404`, so it cannot enumerate routes either.
//!
//! When no credential has been set the middleware is a single early return.
//! That is the whole "authentication is optional on loopback" path, and both
//! sides of it are tested.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::extract::{ConnectInfo, FromRequestParts, Path, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uguisu_core::auth::{ApiToken, Scope, Session};
use uguisu_core::ids::ApiTokenId;
use uguisu_core::secret::Secret;

use crate::extract::{Body, Rejection};
use crate::library::{ApiResult, api_error, body, engine, parse_id};
use crate::{ApiError, AppState, expose};

/// Name of the session cookie.
pub const COOKIE: &str = "uguisu_session";

/// Header a mutating request from a browser must echo the session's token in.
pub const CSRF_HEADER: &str = "x-uguisu-csrf";

/// Failed logins one peer may make inside [`LIMIT_WINDOW`].
const LIMIT_PER_PEER: u32 = 10;

/// Failed logins the whole server will absorb inside [`LIMIT_WINDOW`].
///
/// A distributed attempt does not get a fresh allowance per address, and this
/// is also what bounds how much argon2 memory a flood can ask for.
const LIMIT_GLOBAL: u32 = 100;

/// How long a failure counts against the limits.
const LIMIT_WINDOW: Duration = Duration::minutes(5);

/// How many peers the limiter remembers before it forgets the oldest.
///
/// The limiter must not become the resource exhaustion it exists to prevent.
const LIMIT_PEERS: usize = 1_024;

/// What a request needs before it is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Answerable without a credential, even when authentication is on.
    Public,
    /// Needs a credential.
    Read,
    /// Needs a credential that may change things, and CSRF for a cookie.
    Mutate,
}

/// Who is asking.
#[derive(Debug, Clone)]
pub(crate) enum Principal {
    /// Nobody in particular: either authentication is off, or the request
    /// carried nothing that resolved.
    Anonymous,
    /// A browser session.
    Session(Session),
    /// An API token.
    Token(ApiToken),
}

impl Principal {
    /// Whether the request may change something.
    fn may_mutate(&self) -> bool {
        match self {
            Self::Anonymous => false,
            // A session is the operator at a keyboard; a token is only as
            // powerful as its scope.
            Self::Session(_) => true,
            Self::Token(t) => t.scope.may_mutate(),
        }
    }

    fn named(&self) -> Option<&'static str> {
        match self {
            Self::Anonymous => None,
            Self::Session(_) => Some("session"),
            Self::Token(_) => Some("token"),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Principal {
    type Rejection = Rejection;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // The middleware runs before every handler and always inserts one, so
        // a miss is this crate wired wrong rather than anything a client did.
        std::future::ready(parts.extensions.get::<Self>().cloned().ok_or_else(|| {
            tracing::error!("a handler asked for the principal outside the auth layer");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "the server could not decide who was asking",
            )
        }))
    }
}

/// Everything the auth layer needs that is not in the database.
#[derive(Debug)]
pub struct AuthState {
    /// Whether a credential exists. Read on every request, written when the
    /// first password is set — a second process cannot set one behind this
    /// server's back, because the engine holds an exclusive data-directory
    /// lock.
    required: AtomicBool,
    /// Whether the session cookie carries `Secure`.
    cookie_secure: bool,
    /// Whether an exposed bind without a credential was deliberately allowed.
    allow_insecure: bool,
    /// Reverse proxies whose `X-Forwarded-For` names the client (ADR 0062).
    trusted_proxies: Vec<TrustedProxy>,
    /// Failed logins, per peer and overall.
    limits: Mutex<Limits>,
}

#[derive(Debug, Default)]
struct Limits {
    peers: HashMap<IpAddr, Window>,
    global: Window,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started_at: OffsetDateTime,
    failures: u32,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            started_at: OffsetDateTime::UNIX_EPOCH,
            failures: 0,
        }
    }
}

impl Window {
    /// Counts a failure, rolling the window when it has run out.
    fn record(&mut self, now: OffsetDateTime) -> u32 {
        if now - self.started_at > LIMIT_WINDOW {
            *self = Self {
                started_at: now,
                failures: 0,
            };
        }
        self.failures += 1;
        self.failures
    }

    fn over(&self, now: OffsetDateTime, ceiling: u32) -> bool {
        now - self.started_at <= LIMIT_WINDOW && self.failures >= ceiling
    }
}

/// A reverse proxy, by address or network, whose `X-Forwarded-For` header
/// is believed (ADR 0062). Never every address: `0.0.0.0/0` and `::/0` are
/// refused, since they would let any client name itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxy(ipnet::IpNet);

impl std::str::FromStr for TrustedProxy {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let raw = raw.trim();
        let net = raw
            .parse::<ipnet::IpNet>()
            .or_else(|_| raw.parse::<IpAddr>().map(ipnet::IpNet::from))
            .map_err(|_| format!("`{raw}` is not an IP address or network"))?;
        // A peer is compared in its canonical form, where a mapped IPv4
        // address is IPv4, so a mapped network has to be one too.
        let net = match net {
            ipnet::IpNet::V6(v6) if v6.prefix_len() >= 96 => v6
                .addr()
                .to_ipv4_mapped()
                .and_then(|v4| ipnet::Ipv4Net::new(v4, v6.prefix_len() - 96).ok())
                .map_or(net, ipnet::IpNet::V4),
            other => other,
        };
        if net.prefix_len() == 0 {
            return Err(format!("`{raw}` would trust every address"));
        }
        Ok(Self(net))
    }
}

impl TrustedProxy {
    fn holds(&self, ip: IpAddr) -> bool {
        self.0.contains(&ip.to_canonical())
    }
}

/// The client a request came from. Only when the socket's peer is a trusted
/// proxy is `X-Forwarded-For` read, from the right: each hop a trusted proxy
/// added is skipped, and the first one that is not trusted is the client.
/// A hop that is not an address ends the walk at the proxy that sent it.
fn client_address(
    socket: IpAddr,
    headers: &axum::http::HeaderMap,
    trusted: &[TrustedProxy],
) -> IpAddr {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|t| t.holds(ip));
    if !is_trusted(socket) {
        return socket;
    }
    // A line that is not text is a hop that is not an address, never skipped.
    let hops: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .map(|v| v.to_str().unwrap_or(""))
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    let mut client = socket;
    for hop in hops.iter().rev() {
        let Ok(ip) = hop.parse::<IpAddr>() else {
            return client;
        };
        client = ip;
        if !is_trusted(ip) {
            break;
        }
    }
    client
}

/// How the authentication layer behaves, decided once at startup.
#[derive(Debug, Clone, Default)]
pub struct AuthOptions {
    /// A credential exists, so requests need one.
    pub required: bool,
    /// Add `Secure` to the session cookie, which a deployment behind TLS must
    /// set: Uguisu does not terminate TLS and will not guess from a header it
    /// does not trust.
    pub cookie_secure: bool,
    /// Serve a network address without a credential anyway (ADR 0037).
    pub allow_insecure_exposure: bool,
    /// Reverse proxies whose `X-Forwarded-For` names the client (ADR 0062).
    pub trusted_proxies: Vec<TrustedProxy>,
}

impl AuthState {
    /// Builds the layer's state.
    #[must_use]
    pub fn new(options: AuthOptions) -> Self {
        Self {
            required: AtomicBool::new(options.required),
            cookie_secure: options.cookie_secure,
            allow_insecure: options.allow_insecure_exposure,
            trusted_proxies: options.trusted_proxies,
            limits: Mutex::new(Limits::default()),
        }
    }

    /// Whether the deployment said to serve a network address without a
    /// credential (ADR 0037).
    #[must_use]
    pub fn allow_insecure(&self) -> bool {
        self.allow_insecure
    }

    /// Whether a credential exists, so whether anything is refused.
    #[must_use]
    pub fn required(&self) -> bool {
        self.required.load(Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Limits> {
        self.limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Seconds this peer must wait, or `None` while it still has an allowance.
    ///
    /// The answer is what the client is told in `Retry-After`, so it is the
    /// longer of the two windows that apply: clearing the per-peer one early
    /// would not help while the global one still holds.
    fn retry_after(&self, peer: IpAddr) -> Option<i64> {
        let now = OffsetDateTime::now_utc();
        let limits = self.lock();
        let spent = [
            Some(&limits.global).filter(|w| w.over(now, LIMIT_GLOBAL)),
            limits
                .peers
                .get(&peer)
                .filter(|w| w.over(now, LIMIT_PER_PEER)),
        ];
        spent
            .into_iter()
            .flatten()
            .map(|w| (w.started_at + LIMIT_WINDOW - now).whole_seconds().max(1))
            .max()
    }

    fn record_failure(&self, peer: IpAddr) {
        let now = OffsetDateTime::now_utc();
        let mut limits = self.lock();
        limits.global.record(now);
        if limits.peers.len() >= LIMIT_PEERS && !limits.peers.contains_key(&peer) {
            // Forget the window that began longest ago. An attacker who fills
            // the map evicts their own oldest entry, not a defence.
            if let Some(oldest) = limits
                .peers
                .iter()
                .min_by_key(|(_, w)| w.started_at)
                .map(|(ip, _)| *ip)
            {
                limits.peers.remove(&oldest);
            }
        }
        limits.peers.entry(peer).or_default().record(now);
    }

    fn clear(&self, peer: IpAddr) {
        self.lock().peers.remove(&peer);
    }
}

/// What a request needs, from its method and path alone.
///
/// `credential_set` only affects the bootstrap route: setting the first
/// password has to be possible before there is anything to authenticate with,
/// and the exposure gate guarantees that state means loopback.
pub(crate) fn access_of(method: &Method, path: &str, credential_set: bool) -> Access {
    // The SPA and its assets: the login page must load, and none of it is data.
    if !path.starts_with("/api/") {
        return Access::Public;
    }
    // The public list, spelled once. Everything not on it is authenticated,
    // including a path that does not exist.
    let public =
        matches!(
            (method, path),
            (&Method::GET, "/api/v1/health" | "/api/v1/auth/session")
                | (&Method::POST, "/api/v1/auth/login")
        ) || (method == Method::POST && path == "/api/v1/auth/password" && !credential_set);
    if public {
        return Access::Public;
    }
    if method == Method::GET || method == Method::HEAD {
        Access::Read
    } else {
        Access::Mutate
    }
}

fn cookie(headers: &HeaderMap) -> Option<&str> {
    // `Cookie` is `name=value` pairs separated by `; `. Twelve lines rather
    // than a cookie crate, because this is the only header Uguisu parses and
    // it does not need a jar.
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| name.trim() == COOKIE)
        .map(|(_, value)| value.trim())
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// The address a request arrived from, for the login limiter's bucket.
///
/// The client, for the login limiter and the logs: the socket's peer, or,
/// behind a configured reverse proxy, the client its `X-Forwarded-For` names
/// (ADR 0062). With no proxy configured no header is read, so a client cannot
/// pick its own bucket. Absent in the in-process tests, which have no socket,
/// so it falls back to one shared bucket rather than failing the request.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Peer(IpAddr);

impl FromRequestParts<AppState> for Peer {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let socket = socket_peer(parts);
        std::future::ready(Ok(Self(client_address(
            socket,
            &parts.headers,
            &state.auth.trusted_proxies,
        ))))
    }
}

/// The socket's peer and nothing else, for a decision no header may sway:
/// whether a request comes from this machine.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SocketPeer(IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for SocketPeer {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self(socket_peer(parts))))
    }
}

fn socket_peer(parts: &Parts) -> IpAddr {
    parts
        .extensions
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |c| c.0.ip())
}

/// Decides every request: resolves who is asking, then refuses or continues.
pub(crate) async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    if cross_site(&request) {
        return refuse(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a change sent from a page on another site is refused",
        );
    }
    // Authentication off: one early return, and no database read.
    if !state.auth.required() {
        request.extensions_mut().insert(Principal::Anonymous);
        return next.run(request).await;
    }
    let Some(engine) = state.engine.as_ref() else {
        // A credential lives in the database, so a server without one cannot
        // check anything. Reachable only in a discovery-only process, where
        // `required` is false, so this is belt and braces.
        request.extensions_mut().insert(Principal::Anonymous);
        return next.run(request).await;
    };

    let access = access_of(
        request.method(),
        request.uri().path(),
        state.auth.required(),
    );
    let principal = match resolve(engine, request.headers()).await {
        Ok(p) => p,
        Err(response) => return response,
    };

    if matches!(principal, Principal::Anonymous) && access != Access::Public {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "this request needs a session cookie or an API token",
        );
    }
    if access == Access::Mutate {
        if !principal.may_mutate() {
            return refuse(
                StatusCode::FORBIDDEN,
                "forbidden",
                "this token may read but not change anything",
            );
        }
        if let Principal::Session(session) = &principal {
            // A browser attaches cookies to a cross-site form post but cannot
            // attach this header, which is why a bearer token is exempt: there
            // is nothing for another site to forge.
            let presented = request
                .headers()
                .get(CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !Secret::new(session.csrf_token.as_bytes()).ct_eq(presented.as_bytes()) {
                return refuse(
                    StatusCode::FORBIDDEN,
                    "csrf_required",
                    "a cookie-authenticated change needs the session's CSRF token",
                );
            }
        }
    }
    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// Whether a state change was sent by a page on another site.
///
/// A browser names the page in `Origin` on every request whose method is not
/// `GET` or `HEAD`, a form post included, and a form post or a `no-cors` fetch
/// reaches a mutation without a preflight, so with authentication off nothing
/// else would stop another site driving this one (ADR 0058). A change without
/// `Origin` is the CLI or curl, and is not refused. Only the authority is
/// compared: behind a TLS proxy the browser's scheme is not the one this
/// server sees.
fn cross_site(request: &Request) -> bool {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return false;
    }
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return false;
    };
    let theirs =
        origin
            .to_str()
            .ok()
            .and_then(|o| o.split_once("://"))
            .map(|(scheme, authority)| {
                let default = if scheme.eq_ignore_ascii_case("https") {
                    ":443"
                } else {
                    ":80"
                };
                authority.strip_suffix(default).unwrap_or(authority)
            });
    let own = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(axum::http::uri::Authority::as_str)
        })
        .map(|host| {
            let host = host.trim();
            host.strip_suffix(":80")
                .or_else(|| host.strip_suffix(":443"))
                .unwrap_or(host)
        });
    // An opaque `Origin: null` has no authority, and is refused with the rest.
    !matches!((theirs, own), (Some(theirs), Some(own)) if theirs.eq_ignore_ascii_case(own))
}

/// Resolves the credential a request carries, if it carries one.
#[allow(clippy::result_large_err)] // a `Response` is axum's refusal; built at most once per request
async fn resolve(
    engine: &uguisu_engine::Engine,
    headers: &HeaderMap,
) -> Result<Principal, Response> {
    if let Some(token) = bearer(headers) {
        return match engine.token_for(token).await {
            Ok(Some(t)) => Ok(Principal::Token(t)),
            Ok(None) => Ok(Principal::Anonymous),
            Err(e) => Err(api_error(&e).into_response()),
        };
    }
    if let Some(value) = cookie(headers) {
        return match engine.session_for(value).await {
            Ok(Some(s)) => Ok(Principal::Session(s)),
            Ok(None) => Ok(Principal::Anonymous),
            Err(e) => Err(api_error(&e).into_response()),
        };
    }
    Ok(Principal::Anonymous)
}

fn refuse(status: StatusCode, kind: &'static str, message: &'static str) -> Response {
    ApiError::new(status, kind, message).into_response()
}

/// The authentication routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/session", get(session))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/exchange", post(exchange))
        .route("/api/v1/auth/password", post(password))
        .route("/api/v1/auth/tokens", get(list_tokens).post(create_token))
        .route(
            "/api/v1/auth/tokens/{id}",
            axum::routing::delete(revoke_token),
        )
        // The same route under POST, like `/policy/clear`: the CLI's HTTP
        // client speaks GET and POST only, and widening it for one route is a
        // worse trade than one alias.
        .route("/api/v1/auth/tokens/{id}/revoke", post(revoke_token))
}

/// Body of `GET /api/v1/auth/session`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct SessionBody {
    /// Whether anything on this server needs a credential.
    auth_required: bool,
    /// Whether a password has been set, which is the same question asked of
    /// the state rather than of this request.
    credential_set: bool,
    /// Whether *this* request is authenticated.
    authenticated: bool,
    /// `session`, `token`, or absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    principal: Option<&'static str>,
    /// The operator's name, when there is a credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    /// The token a mutating request must echo, for a session only.
    #[serde(skip_serializing_if = "Option::is_none")]
    csrf_token: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/session",
    tag = "auth",
    security(),
    responses((status = 200, description = "What this request is, and what the server needs", body = SessionBody))
)]
pub(crate) async fn session(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let required = state.auth.required();
    let username = match state.engine.as_ref() {
        Some(engine) => engine
            .credential_username()
            .await
            .map_err(|e| api_error(&e))?,
        None => None,
    };
    Ok(body(
        StatusCode::OK,
        &SessionBody {
            auth_required: required,
            credential_set: username.is_some(),
            authenticated: !matches!(principal, Principal::Anonymous),
            principal: principal.named(),
            username,
            csrf_token: match &principal {
                Principal::Session(s) => Some(s.csrf_token.clone()),
                _ => None,
            },
        },
    ))
}

/// Body of `POST /api/v1/auth/login`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
}

/// What a login answers with. Never the password, and never the cookie value:
/// that goes in `Set-Cookie`, where script cannot read it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct LoginBody {
    username: String,
    csrf_token: String,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    security(),
    request_body = LoginRequest,
    responses(
        (status = 200, description = "The session cookie is in `Set-Cookie`", body = LoginBody),
        (status = 401, description = "One message for both an unknown user and a wrong password", body = ApiError),
        (status = 429, description = "Too many failures from this peer; `Retry-After` says how long to wait", body = ApiError)
    )
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    Peer(peer): Peer,
    Body(credentials): Body<LoginRequest>,
) -> ApiResult {
    if let Some(seconds) = state.auth.retry_after(peer) {
        tracing::warn!(%peer, seconds, "login refused: too many failures");
        let mut response = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            format!("too many failed logins; wait {seconds} seconds"),
        )
        .into_response();
        if let Ok(value) = axum::http::HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return Ok(response);
    }
    let engine = engine(&state)?;
    let opened = engine
        .login(&credentials.username, &Secret::new(credentials.password))
        .await
        .map_err(|e| api_error(&e))?;
    let Some(opened) = opened else {
        state.auth.record_failure(peer);
        // One message for an unknown user and a wrong password, and the log
        // says which category it was, never which value.
        tracing::warn!(%peer, reason = "credentials", "login failed");
        return Ok(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "username or password is incorrect",
        )
        .into_response());
    };
    state.auth.clear(peer);
    let cookie = set_cookie(
        opened.cookie.expose(),
        opened.session.absolute_expires_at,
        state.auth.cookie_secure,
    );
    let mut response = body(
        StatusCode::OK,
        &LoginBody {
            username: credentials.username.trim().to_owned(),
            csrf_token: opened.session.csrf_token.clone(),
            expires_at: opened.session.absolute_expires_at,
        },
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

/// What a desktop bootstrap answers with.
///
/// The session cookie is in `Set-Cookie`, where script cannot read it; nothing
/// here is a secret the page has to keep.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct ExchangeBody {
    csrf_token: String,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/exchange",
    tag = "auth",
    operation_id = "auth_exchange",
    responses(
        (status = 201, description = "A session was opened; its cookie is in `Set-Cookie`", body = ExchangeBody),
        (status = 403, description = "Not a write token, or not from this machine", body = ApiError)
    )
)]
pub(crate) async fn exchange(
    State(state): State<AppState>,
    SocketPeer(peer): SocketPeer,
    principal: Principal,
) -> ApiResult {
    // The desktop shell holds its per-launch token in its own process and
    // never puts it in a URL, so the only way to present one is from this
    // machine. Refusing anything else means a token that did leak still buys
    // nothing from another host. The socket's address, never a header.
    if !expose::loopback(std::net::SocketAddr::new(peer, 0)) {
        tracing::warn!(%peer, "session exchange refused: not loopback");
        return Ok(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a session can only be exchanged from this machine",
        )
        .into_response());
    }
    // A cookie principal is already a session and has nothing to exchange;
    // the middleware has already refused anonymous and read-scoped callers.
    let Principal::Token(token) = &principal else {
        return Ok(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a session can only be exchanged for an API token",
        )
        .into_response());
    };
    let engine = engine(&state)?;
    let opened = engine.open_session().await.map_err(|e| api_error(&e))?;
    tracing::info!(token_id = %token.id, "session exchanged for a launch token");
    let cookie = set_cookie(
        opened.cookie.expose(),
        opened.session.absolute_expires_at,
        state.auth.cookie_secure,
    );
    let mut response = body(
        StatusCode::CREATED,
        &ExchangeBody {
            csrf_token: opened.session.csrf_token.clone(),
            expires_at: opened.session.absolute_expires_at,
        },
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

/// The `Set-Cookie` value for a session.
///
/// `HttpOnly` so script cannot read it, `SameSite=Lax` so another site cannot
/// drive a state change with it (CSRF covers the rest), `Path=/` because the
/// SPA, the byte routes and the API all need it, and `Secure` only when the
/// deployment says TLS is in front — Uguisu does not terminate it and will not
/// guess from a header it does not trust.
fn set_cookie(value: &str, expires_at: OffsetDateTime, secure: bool) -> String {
    let max_age = (expires_at - OffsetDateTime::now_utc())
        .whole_seconds()
        .max(0);
    let mut cookie = format!("{COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn clear_cookie(secure: bool) -> String {
    let mut cookie = format!("{COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses((status = 204, description = "This session is revoked and the cookie cleared"))
)]
pub(crate) async fn logout(State(state): State<AppState>, principal: Principal) -> ApiResult {
    if let Principal::Session(session) = &principal {
        engine(&state)?
            .logout(session.id)
            .await
            .map_err(|e| api_error(&e))?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&clear_cookie(state.auth.cookie_secure)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

/// Body of `POST /api/v1/auth/password`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct PasswordRequest {
    /// The operator's name. Defaults to the one already set, or `uguisu`.
    username: Option<String>,
    /// Required once a credential exists: a session cookie alone must not be
    /// enough to change the password it was opened with.
    current_password: Option<String>,
    new_password: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/password",
    tag = "auth",
    security(),
    request_body = PasswordRequest,
    responses(
        (status = 204, description = "Set; every other session is revoked"),
        (status = 401, description = "A credential exists and this request carries none"),
        (status = 403, description = "`current_password` is missing or wrong", body = ApiError)
    )
)]
pub(crate) async fn password(
    State(state): State<AppState>,
    principal: Principal,
    Body(request): Body<PasswordRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let existing = engine
        .credential_username()
        .await
        .map_err(|e| api_error(&e))?;
    if let Some(current) = &existing {
        let presented = request.current_password.clone().unwrap_or_default();
        if engine
            .login(current, &Secret::new(presented))
            .await
            .map_err(|e| api_error(&e))?
            .is_none()
        {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "the current password is required to change it",
            ));
        }
    }
    let username = request
        .username
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map_or_else(
            || existing.clone().unwrap_or_else(|| "uguisu".to_owned()),
            str::to_owned,
        );
    let keep = match &principal {
        Principal::Session(s) => Some(s.id),
        _ => None,
    };
    engine
        .set_password(&username, &Secret::new(request.new_password), keep)
        .await
        .map_err(|e| api_error(&e))?;
    state.auth.required.store(true, Ordering::Relaxed);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct TokenList {
    tokens: Vec<ApiToken>,
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/tokens",
    tag = "auth",
    responses((status = 200, description = "Records only: never a secret, never a digest", body = TokenList))
)]
pub(crate) async fn list_tokens(State(state): State<AppState>) -> ApiResult {
    let tokens = engine(&state)?
        .list_tokens()
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &TokenList { tokens }))
}

/// Body of `POST /api/v1/auth/tokens`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct TokenRequest {
    name: String,
    /// `read` or `write`; `write` when absent.
    scope: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

/// What creating a token answers with. The only place the secret exists.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct NewTokenBody {
    token: ApiToken,
    secret: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/tokens",
    tag = "auth",
    request_body = TokenRequest,
    responses(
        (status = 201, description = "The only answer that carries the secret", body = NewTokenBody),
        (status = 400, body = ApiError)
    )
)]
pub(crate) async fn create_token(
    State(state): State<AppState>,
    Body(request): Body<TokenRequest>,
) -> ApiResult {
    let scope = match request.scope.as_deref() {
        None => Scope::Write,
        Some(raw) => Scope::parse(raw).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("`{raw}` is not a scope (read, write)"),
            )
        })?,
    };
    let issued = engine(&state)?
        .issue_token(&request.name, scope, request.expires_at)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::CREATED,
        &NewTokenBody {
            token: issued.token,
            secret: issued.secret.into_inner(),
        },
    ))
}

#[utoipa::path(
    method(delete),
    path = "/api/v1/auth/tokens/{id}",
    tag = "auth",
    params(("id" = String, Path, description = "Token id")),
    responses((status = 204, description = "Revoked, and kept so it can be shown as revoked"), (status = 404, body = ApiError))
)]
pub(crate) async fn revoke_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let id: ApiTokenId = parse_id(&id, "token")?;
    let revoked = engine(&state)?
        .revoke_token(id)
        .await
        .map_err(|e| api_error(&e))?;
    if revoked {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("token {id} is not there to revoke"),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap_or(header::COOKIE),
                axum::http::HeaderValue::from_str(value)
                    .unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
            );
        }
        h
    }

    #[test]
    fn the_spa_and_the_login_page_stay_public() {
        for (method, path) in [
            (Method::GET, "/"),
            (Method::GET, "/assets/index-abc.js"),
            (Method::GET, "/library"),
            (Method::GET, "/api/v1/health"),
            (Method::GET, "/api/v1/auth/session"),
            (Method::POST, "/api/v1/auth/login"),
        ] {
            assert_eq!(access_of(&method, path, true), Access::Public, "{path}");
        }
    }

    #[test]
    fn reading_and_changing_are_told_apart() {
        assert_eq!(
            access_of(&Method::GET, "/api/v1/podcasts", true),
            Access::Read
        );
        assert_eq!(
            access_of(&Method::HEAD, "/api/v1/podcasts", true),
            Access::Read
        );
        for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            assert_eq!(
                access_of(&method, "/api/v1/podcasts", true),
                Access::Mutate,
                "{method}"
            );
        }
    }

    /// Default deny: a path nobody has heard of still needs a credential, so a
    /// probe cannot tell a real route from an imaginary one.
    #[test]
    fn an_unknown_api_path_is_not_public() {
        assert_eq!(
            access_of(&Method::GET, "/api/v1/nothing-here", true),
            Access::Read
        );
        assert_eq!(
            access_of(&Method::POST, "/api/v2/anything", true),
            Access::Mutate
        );
    }

    #[test]
    fn setting_the_first_password_needs_no_credential() {
        assert_eq!(
            access_of(&Method::POST, "/api/v1/auth/password", false),
            Access::Public
        );
        assert_eq!(
            access_of(&Method::POST, "/api/v1/auth/password", true),
            Access::Mutate,
            "once there is one, changing it is a change like any other"
        );
    }

    #[test]
    fn a_cookie_is_read_out_of_a_list() {
        assert_eq!(
            cookie(&headers(&[("cookie", "uguisu_session=abc")])),
            Some("abc")
        );
        assert_eq!(
            cookie(&headers(&[(
                "cookie",
                "theme=dark; uguisu_session=abc; x=1"
            )])),
            Some("abc")
        );
        assert_eq!(
            cookie(&headers(&[("cookie", " uguisu_session = abc ")])),
            Some("abc")
        );
        assert_eq!(cookie(&headers(&[("cookie", "other=abc")])), None);
        assert_eq!(cookie(&headers(&[("cookie", "uguisu_session")])), None);
        assert_eq!(cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn a_bearer_token_is_read_out_of_the_header() {
        assert_eq!(
            bearer(&headers(&[("authorization", "Bearer abc")])),
            Some("abc")
        );
        assert_eq!(bearer(&headers(&[("authorization", "Bearer ")])), None);
        assert_eq!(bearer(&headers(&[("authorization", "Basic abc")])), None);
        assert_eq!(bearer(&headers(&[("authorization", "bearer abc")])), None);
    }

    #[test]
    fn the_cookie_says_httponly_and_lax() {
        let expires = OffsetDateTime::now_utc() + Duration::days(30);
        let plain = set_cookie("abc", expires, false);
        assert!(plain.starts_with("uguisu_session=abc; "), "{plain}");
        assert!(plain.contains("HttpOnly"), "{plain}");
        assert!(plain.contains("SameSite=Lax"), "{plain}");
        assert!(plain.contains("Path=/"), "{plain}");
        assert!(!plain.contains("Secure"), "{plain}");
        assert!(set_cookie("abc", expires, true).contains("; Secure"));
        assert!(clear_cookie(false).contains("Max-Age=0"));
    }

    #[test]
    fn the_window_rolls_and_the_ceiling_holds() {
        let start = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
        let mut w = Window::default();
        for _ in 0..LIMIT_PER_PEER {
            w.record(start);
        }
        assert!(w.over(start, LIMIT_PER_PEER));
        assert!(
            !w.over(start + LIMIT_WINDOW + Duration::seconds(1), LIMIT_PER_PEER),
            "an old window stops counting"
        );
        w.record(start + LIMIT_WINDOW + Duration::seconds(2));
        assert_eq!(
            w.failures, 1,
            "the window restarted rather than accumulated"
        );
    }

    fn proxies(raw: &[&str]) -> Vec<TrustedProxy> {
        raw.iter().map(|p| p.parse().unwrap()).collect()
    }

    fn forwarded(values: &[&str]) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        for v in values {
            headers.append("x-forwarded-for", v.parse().unwrap());
        }
        headers
    }

    #[test]
    fn trusted_proxy_must_be_narrow() {
        assert!("10.0.0.1".parse::<TrustedProxy>().is_ok());
        assert!("172.16.0.0/12".parse::<TrustedProxy>().is_ok());
        assert!("::1".parse::<TrustedProxy>().is_ok());
        for refused in [
            "0.0.0.0/0",
            "::/0",
            "::ffff:0:0/96",
            "proxy.example",
            "10.0.0.0/33",
            "",
        ] {
            assert!(refused.parse::<TrustedProxy>().is_err(), "{refused}");
        }
        // Written as mapped IPv6, it still names the IPv4 peer.
        let mapped = "::ffff:10.0.0.2".parse::<TrustedProxy>().unwrap();
        assert_eq!(mapped, "10.0.0.2".parse().unwrap());
        assert_eq!(
            "::ffff:10.0.0.0/104".parse::<TrustedProxy>().unwrap(),
            "10.0.0.0/8".parse().unwrap()
        );
    }

    #[test]
    fn forwarded_for_read_right_to_left() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let trusted = proxies(&["10.0.0.0/8"]);
        // Not from a proxy: the header is ignored.
        assert_eq!(
            client_address(ip("198.51.100.7"), &forwarded(&["203.0.113.5"]), &trusted),
            ip("198.51.100.7")
        );
        // A client cannot hide behind a hop it wrote itself: the rightmost
        // untrusted address is the client, whatever stands left of it.
        assert_eq!(
            client_address(
                ip("10.0.0.2"),
                &forwarded(&["1.2.3.4, 203.0.113.5", "10.0.0.9"]),
                &trusted
            ),
            ip("203.0.113.5")
        );
        // A hop that is not an address stops at the proxy that sent it.
        assert_eq!(
            client_address(ip("10.0.0.2"), &forwarded(&["203.0.113.5, junk"]), &trusted),
            ip("10.0.0.2")
        );
        // So does a line that is not text, with whatever stands left of it.
        let mut unreadable = forwarded(&["6.6.6.6"]);
        unreadable.append(
            "x-forwarded-for",
            axum::http::HeaderValue::from_bytes(b"\xff, 198.51.100.9").unwrap(),
        );
        assert_eq!(
            client_address(ip("10.0.0.2"), &unreadable, &trusted),
            ip("10.0.0.2")
        );
        // No header behind a proxy: the proxy itself.
        assert_eq!(
            client_address(ip("10.0.0.2"), &forwarded(&[]), &trusted),
            ip("10.0.0.2")
        );
        // An IPv4 peer seen as mapped IPv6 is still the proxy.
        assert_eq!(
            client_address(
                ip("::ffff:10.0.0.2"),
                &forwarded(&["203.0.113.5"]),
                &trusted
            ),
            ip("203.0.113.5")
        );
    }

    #[test]
    fn the_limiter_forgets_the_oldest_peer() {
        let state = AuthState::new(AuthOptions {
            required: true,
            ..AuthOptions::default()
        });
        for i in 0..u32::try_from(LIMIT_PEERS).unwrap_or(u32::MAX) + 50 {
            state.record_failure(IpAddr::V4(Ipv4Addr::from_bits(i)));
        }
        assert!(
            state.lock().peers.len() <= LIMIT_PEERS,
            "the limiter must not become the exhaustion it prevents"
        );
    }

    #[test]
    fn a_read_token_may_not_change_anything() {
        let token = |scope| {
            Principal::Token(ApiToken {
                id: ApiTokenId::new(),
                name: "t".to_owned(),
                scope,
                created_at: OffsetDateTime::UNIX_EPOCH,
                last_used_at: None,
                expires_at: None,
                revoked_at: None,
            })
        };
        assert!(!token(Scope::Read).may_mutate());
        assert!(token(Scope::Write).may_mutate());
        assert!(!Principal::Anonymous.may_mutate());
    }
}
