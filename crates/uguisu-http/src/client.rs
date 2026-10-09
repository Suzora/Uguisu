//! The policy-enforcing HTTP client.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use http::header::{
    ACCEPT, ACCEPT_ENCODING, IF_MODIFIED_SINCE, IF_NONE_MATCH, IF_RANGE, LOCATION, RANGE,
};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use tokio_util::sync::CancellationToken;
use uguisu_core::config::NetworkConfig;
use uguisu_core::redact;
use url::Url;

use crate::error::HttpError;
use crate::headers;
use crate::policy::NetworkPolicy;
use crate::resolver::SafeResolver;
use crate::retry::{RetryPolicy, retry_after};
use crate::stream::{BodyStream, StreamingResponse};

/// What a client is used for; selects size caps and default headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Directory / provider JSON APIs (small bodies).
    Discovery,
    /// HTML pages for feed autodiscovery.
    Html,
    /// RSS/Atom feeds.
    Feed,
    /// Media enclosures (large; the cap is a safety net, not a policy).
    /// Bodies are transferred identity-encoded (no automatic decompression),
    /// so `Content-Length`, `Range` and hashes refer to the bytes on the wire.
    Media,
    /// Podcast artwork. Small, from a stranger's URL in a feed, and
    /// destined to be embedded in files other people's players parse - so
    /// it gets the same SSRF stack and redirect re-validation as every
    /// other untrusted profile and a cap several orders of magnitude below
    /// [`Self::Media`].
    Artwork,
    /// The user's own Uguisu server; private addresses allowed.
    Trusted,
}

impl Profile {
    /// Default body size cap.
    pub const fn max_body_bytes(self) -> u64 {
        match self {
            Self::Discovery => 2 * 1024 * 1024,
            Self::Html => 5 * 1024 * 1024,
            Self::Feed => 50 * 1024 * 1024,
            Self::Media => 8 * 1024 * 1024 * 1024,
            Self::Artwork => 16 * 1024 * 1024,
            Self::Trusted => 64 * 1024 * 1024,
        }
    }

    /// Default `Accept` header.
    pub const fn accept(self) -> &'static str {
        match self {
            Self::Discovery | Self::Trusted => "application/json, text/javascript;q=0.9, */*;q=0.1",
            Self::Html => {
                "text/html, application/xhtml+xml, application/rss+xml;q=0.9, application/atom+xml;q=0.9, application/xml;q=0.8, */*;q=0.1"
            }
            Self::Feed => {
                "application/rss+xml, application/atom+xml, application/xml;q=0.9, text/xml;q=0.9, */*;q=0.1"
            }
            Self::Media => "*/*",
            Self::Artwork => "image/jpeg, image/png, image/webp, image/*;q=0.8",
        }
    }
}

/// Client construction parameters.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `User-Agent` header.
    pub user_agent: String,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Total timeout per buffered request attempt (including the body). For
    /// [`HttpClient::get_stream`] it bounds only the wait for the headers;
    /// the body is bounded by [`GetOptions::idle_timeout`].
    pub request_timeout: Duration,
    /// Maximum redirect hops followed by [`HttpClient::get`].
    pub max_redirects: usize,
    /// Network policy (ignored for [`Profile::Trusted`], which always uses a trusted policy).
    pub policy: Arc<NetworkPolicy>,
    /// Retry policy for idempotent requests.
    pub retry: RetryPolicy,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            user_agent: uguisu_core::USER_AGENT.to_owned(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            max_redirects: 5,
            policy: Arc::new(NetworkPolicy::strict()),
            retry: RetryPolicy::default(),
        }
    }
}

impl ClientConfig {
    /// Builds a config from the shared network settings.
    pub fn from_network(cfg: &NetworkConfig) -> Self {
        Self {
            user_agent: cfg
                .user_agent
                .clone()
                .unwrap_or_else(|| uguisu_core::USER_AGENT.to_owned()),
            connect_timeout: cfg.connect_timeout,
            request_timeout: cfg.request_timeout,
            policy: Arc::new(
                NetworkPolicy::strict().allow_private_hosts(cfg.allow_private_hosts.iter()),
            ),
            ..Self::default()
        }
    }
}

/// Conditional request validators, as a feed refresh sends them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conditional {
    /// Value for `If-None-Match`.
    pub etag: Option<String>,
    /// Value for `If-Modified-Since`.
    pub last_modified: Option<String>,
}

/// Per-request options.
#[derive(Debug, Clone, Default)]
pub struct GetOptions {
    /// Extra headers. A redirect to another host keeps only `Accept`.
    pub headers: Vec<(HeaderName, HeaderValue)>,
    /// Override the profile's body cap.
    pub max_bytes: Option<u64>,
    /// Cancellation token; the request stops as soon as it is cancelled.
    pub cancel: Option<CancellationToken>,
    /// Conditional validators.
    pub conditional: Option<Conditional>,
    /// Whether transient failures are retried (default true; set false for non-idempotent calls).
    pub retry: Option<bool>,
    /// Sends `Range: bytes=<start>-` (kept on every redirect hop).
    pub range_start: Option<u64>,
    /// Sends `If-Range` verbatim (the caller guarantees a strong validator).
    pub if_range: Option<String>,
    /// Streaming only: the longest wait for the next body chunk.
    pub idle_timeout: Option<Duration>,
}

/// A fully read response.
#[derive(Debug, Clone)]
pub struct Response {
    /// URL that produced the final response (after redirects).
    pub url: Url,
    /// Status of the final response.
    pub status: StatusCode,
    /// Headers of the final response.
    pub headers: HeaderMap,
    /// Body (decompressed).
    pub body: Bytes,
    /// Every URL that answered with a redirect, in order.
    pub redirects: Vec<Url>,
    /// `true` when the response came through at least one redirect and
    /// every hop was permanent (301 or 308), so callers may adopt the
    /// final URL as the new canonical location.
    pub permanent_redirect: bool,
    /// Wall-clock time for the whole exchange.
    pub elapsed: Duration,
    /// Number of attempts made for the final hop (1 = no retry).
    pub attempts: u32,
}

impl Response {
    /// `Content-Type` header value, if any.
    pub fn content_type(&self) -> Option<&str> {
        headers::content_type(&self.headers)
    }

    /// `ETag` header value, if any (kept verbatim, including weak markers
    /// and quotes, so it can be sent back in `If-None-Match` unchanged).
    pub fn etag(&self) -> Option<&str> {
        headers::etag(&self.headers)
    }

    /// `Last-Modified` header value, if any (verbatim, for `If-Modified-Since`).
    pub fn last_modified(&self) -> Option<&str> {
        headers::last_modified(&self.headers)
    }

    /// `Content-Length` as declared by the server.
    pub fn content_length(&self) -> Option<u64> {
        headers::content_length(&self.headers)
    }

    /// How long the response may be cached according to `Cache-Control`:
    /// `max-age`/`s-maxage` in seconds, [`Duration::ZERO`] for `no-store` or
    /// `no-cache` (the origin forbids reuse), `None` when the header says
    /// nothing about freshness.
    pub fn cache_max_age(&self) -> Option<Duration> {
        headers::cache_max_age(&self.headers)
    }
}

/// Whether the status is a redirect that carries a `Location`.
fn redirect_target(
    url: &Url,
    status: StatusCode,
    headers: &HeaderMap,
) -> Option<Result<Url, HttpError>> {
    if !status.is_redirection() || status == StatusCode::NOT_MODIFIED {
        return None;
    }
    let Some(loc) = headers.get(LOCATION).and_then(|v| v.to_str().ok()) else {
        return Some(Err(HttpError::BadRedirect(status.as_u16())));
    };
    Some(
        url.join(loc)
            .map_err(|_| HttpError::BadRedirect(status.as_u16())),
    )
}

/// Redirect bookkeeping shared by the buffered and streaming `GET`s.
struct Hops {
    redirects: Vec<Url>,
    all_permanent: bool,
    headers: Vec<(HeaderName, HeaderValue)>,
    max: usize,
}

impl Hops {
    fn new(headers: Vec<(HeaderName, HeaderValue)>, max: usize) -> Self {
        Self {
            redirects: Vec::new(),
            all_permanent: true,
            headers,
            max,
        }
    }

    fn permanent(&self) -> bool {
        !self.redirects.is_empty() && self.all_permanent
    }

    /// Returns the next URL to fetch, or `None` when `status` is final.
    fn advance(
        &mut self,
        current: &Url,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Result<Option<Url>, HttpError> {
        match redirect_target(current, status, headers) {
            None => Ok(None),
            Some(Err(e)) => Err(e),
            Some(Ok(next)) => {
                if self.redirects.len() >= self.max {
                    return Err(HttpError::TooManyRedirects { limit: self.max });
                }
                tracing::debug!(from = %redact::urls(current.as_str()), to = %redact::urls(next.as_str()), status = status.as_u16(), "following redirect");
                self.all_permanent &= matches!(
                    status,
                    StatusCode::MOVED_PERMANENTLY | StatusCode::PERMANENT_REDIRECT
                );
                if next.origin() != current.origin() {
                    // Another origin (scheme, host or port) gets only the
                    // headers that say what is wanted: a credential, under
                    // whatever name a provider gives it (`Authorization`,
                    // `X-Auth-Key`), stays behind, also on a downgrade to http.
                    self.headers.retain(|(name, _)| *name == ACCEPT);
                }
                self.redirects.push(current.clone());
                Ok(Some(next))
            }
        }
    }
}

/// A reusable client bound to a [`Profile`] and a [`NetworkPolicy`].
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    profile: Profile,
    config: Arc<ClientConfig>,
    policy: Arc<NetworkPolicy>,
    resolver: SafeResolver,
}

impl HttpClient {
    /// Builds a client for `profile`.
    pub fn new(profile: Profile, config: ClientConfig) -> Result<Self, HttpError> {
        let policy = if profile == Profile::Trusted {
            Arc::new(NetworkPolicy::trusted())
        } else {
            Arc::clone(&config.policy)
        };
        let resolver = SafeResolver::new(Arc::clone(&policy));
        // Media bodies stay identity-encoded: byte ranges, declared lengths
        // and inline hashes must describe the bytes on the wire.
        let decompress = profile != Profile::Media;
        let inner = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .redirect(reqwest::redirect::Policy::none())
            // A proxy from the environment would resolve the host itself, and
            // the resolver hook below, which returns only approved addresses,
            // would never be asked.
            .no_proxy()
            .connect_timeout(config.connect_timeout)
            .gzip(decompress)
            .brotli(decompress)
            .deflate(decompress)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
            .dns_resolver(Arc::new(resolver.clone()))
            .build()
            .map_err(|e| HttpError::Build(e.to_string()))?;
        Ok(Self {
            inner,
            profile,
            config: Arc::new(config),
            policy,
            resolver,
        })
    }

    /// Convenience: default config with the given policy.
    pub fn with_policy(profile: Profile, policy: NetworkPolicy) -> Result<Self, HttpError> {
        Self::new(
            profile,
            ClientConfig {
                policy: Arc::new(policy),
                ..ClientConfig::default()
            },
        )
    }

    /// The profile this client serves.
    pub const fn profile(&self) -> Profile {
        self.profile
    }

    /// The policy in force.
    pub fn policy(&self) -> &NetworkPolicy {
        &self.policy
    }

    /// The configuration in force.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// `GET` with default options.
    pub async fn get(&self, url: &Url) -> Result<Response, HttpError> {
        self.get_with(url, &GetOptions::default()).await
    }

    /// `GET` following redirects manually, validating every hop, with retries and a body cap.
    pub async fn get_with(&self, url: &Url, opts: &GetOptions) -> Result<Response, HttpError> {
        let started = Instant::now();
        let cancel = opts.cancel.clone().unwrap_or_default();
        let max_bytes = opts.max_bytes.unwrap_or(self.profile.max_body_bytes());
        let retry_enabled = opts.retry.unwrap_or(true);
        let mut hops = Hops::new(opts.headers.clone(), self.config.max_redirects);
        let mut current = url.clone();

        loop {
            self.preflight(&current).await?;
            let (resp, attempts) = self
                .fetch_head(
                    &current,
                    &hops.headers,
                    opts,
                    max_bytes,
                    retry_enabled,
                    &cancel,
                    Mode::Buffered,
                )
                .await?;
            let status = resp.status();
            let headers = resp.headers().clone();
            let final_url = resp.url().clone();
            match hops.advance(&current, status, &headers)? {
                None => {
                    let body =
                        read_body(resp, max_bytes, &cancel, self.config.request_timeout).await?;
                    tracing::trace!(url = %redact::urls(final_url.as_str()), status = status.as_u16(), bytes = body.len(), "response read");
                    return Ok(Response {
                        url: final_url,
                        status,
                        headers,
                        body,
                        permanent_redirect: hops.permanent(),
                        redirects: hops.redirects,
                        elapsed: started.elapsed(),
                        attempts,
                    });
                }
                Some(next) => {
                    drop(resp);
                    current = next;
                }
            }
        }
    }

    /// `GET` whose body is read incrementally: for media downloads.
    ///
    /// Behaves like [`get_with`](Self::get_with) up to the headers of the
    /// final hop (policy, redirects, retries on transient failures and error
    /// statuses), then hands the body over as a [`BodyStream`]. Differences:
    /// `Accept-Encoding: identity` is requested, `request_timeout` bounds only
    /// the wait for the headers, and nothing is retried once the body has
    /// started — a body error is the caller's to resume from, typically with
    /// [`GetOptions::range_start`] and [`GetOptions::if_range`].
    pub async fn get_stream(
        &self,
        url: &Url,
        opts: &GetOptions,
    ) -> Result<StreamingResponse, HttpError> {
        let started = Instant::now();
        let cancel = opts.cancel.clone().unwrap_or_default();
        let max_bytes = opts.max_bytes.unwrap_or(self.profile.max_body_bytes());
        let retry_enabled = opts.retry.unwrap_or(true);
        let mut hops = Hops::new(opts.headers.clone(), self.config.max_redirects);
        let mut current = url.clone();

        loop {
            self.preflight(&current).await?;
            let (resp, attempts) = self
                .fetch_head(
                    &current,
                    &hops.headers,
                    opts,
                    max_bytes,
                    retry_enabled,
                    &cancel,
                    Mode::Streaming,
                )
                .await?;
            let status = resp.status();
            let headers = resp.headers().clone();
            let final_url = resp.url().clone();
            match hops.advance(&current, status, &headers)? {
                None => {
                    tracing::trace!(url = %redact::urls(final_url.as_str()), status = status.as_u16(), "streaming response started");
                    return Ok(StreamingResponse {
                        url: final_url,
                        status,
                        headers,
                        permanent_redirect: hops.permanent(),
                        redirects: hops.redirects,
                        attempts,
                        elapsed: started.elapsed(),
                        body: BodyStream::new(resp, opts.idle_timeout, cancel, max_bytes),
                    });
                }
                Some(next) => {
                    drop(resp);
                    current = next;
                }
            }
        }
    }

    /// `POST` with a body, for the CLI's calls to a trusted Uguisu server.
    /// Redirects are not followed and nothing is retried (the call is not
    /// idempotent); the policy, timeouts, cancellation and the body cap
    /// apply as for `GET`.
    pub async fn post(
        &self,
        url: &Url,
        body: Bytes,
        content_type: &str,
        opts: &GetOptions,
    ) -> Result<Response, HttpError> {
        let started = Instant::now();
        let cancel = opts.cancel.clone().unwrap_or_default();
        let max_bytes = opts.max_bytes.unwrap_or(self.profile.max_body_bytes());
        self.preflight(url).await?;
        let mut req = self
            .inner
            .post(url.clone())
            .header(ACCEPT, self.profile.accept())
            .header(http::header::CONTENT_TYPE, content_type)
            .body(body);
        for (name, value) in &opts.headers {
            req = req.header(name.clone(), value.clone());
        }
        let timeout = self.config.request_timeout;
        let resp = tokio::select! {
            () = cancel.cancelled() => return Err(HttpError::Cancelled),
            r = req.timeout(timeout).send() => r.map_err(|e| HttpError::from_reqwest(&e, timeout))?,
        };
        let status = resp.status();
        let headers = resp.headers().clone();
        let final_url = resp.url().clone();
        let body = read_body(resp, max_bytes, &cancel, timeout).await?;
        Ok(Response {
            url: final_url,
            status,
            headers,
            body,
            redirects: Vec::new(),
            permanent_redirect: false,
            elapsed: started.elapsed(),
            attempts: 1,
        })
    }

    /// Layer 1: URL check plus resolution of domain hosts against the policy.
    async fn preflight(&self, url: &Url) -> Result<(), HttpError> {
        self.policy.check_url(url)?;
        if self.policy.is_trusted() {
            return Ok(());
        }
        if let Some(url::Host::Domain(host)) = url.host() {
            if self.policy.is_private_allowed(host) {
                return Ok(());
            }
            match self.resolver.resolve_allowed(host).await {
                Ok(_) => Ok(()),
                Err(crate::resolver::ResolveError::Policy(v)) => Err(HttpError::Policy(v)),
                Err(crate::resolver::ResolveError::Lookup(detail)) => Err(HttpError::Dns {
                    host: host.to_owned(),
                    detail,
                }),
            }
        } else {
            Ok(())
        }
    }

    /// Sends the request until a final answer's headers arrive, retrying
    /// transient failures and error statuses per the retry policy.
    #[allow(clippy::too_many_arguments)]
    async fn fetch_head(
        &self,
        url: &Url,
        headers: &[(HeaderName, HeaderValue)],
        opts: &GetOptions,
        max_bytes: u64,
        retry_enabled: bool,
        cancel: &CancellationToken,
        mode: Mode,
    ) -> Result<(reqwest::Response, u32), HttpError> {
        let max_attempts = if retry_enabled {
            self.config.retry.max_attempts.max(1)
        } else {
            1
        };
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let result = self
                .send_head(url, headers, opts, max_bytes, cancel, mode)
                .await;
            match result {
                Ok(resp)
                    if resp.status().is_server_error()
                        || resp.status() == StatusCode::TOO_MANY_REQUESTS =>
                {
                    if attempt >= max_attempts {
                        return Ok((resp, attempt));
                    }
                    let wait = self
                        .config
                        .retry
                        .delay(attempt, retry_after(resp.headers()));
                    let Some(wait) = wait else {
                        return Ok((resp, attempt));
                    };
                    tracing::debug!(url = %redact::urls(url.as_str()), status = resp.status().as_u16(), attempt, ?wait, "retrying after error status");
                    drop(resp);
                    if !sleep_or_cancel(wait, cancel).await {
                        return Err(HttpError::Cancelled);
                    }
                }
                Ok(resp) => return Ok((resp, attempt)),
                Err(e) if e.is_retryable() && attempt < max_attempts => {
                    let wait = self.config.retry.delay(attempt, None).unwrap_or_default();
                    tracing::debug!(url = %redact::urls(url.as_str()), error = %redact::urls(&e.to_string()), attempt, ?wait, "retrying after transport error");
                    if !sleep_or_cancel(wait, cancel).await {
                        return Err(HttpError::Cancelled);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One request; returns as soon as the headers are in.
    async fn send_head(
        &self,
        url: &Url,
        headers: &[(HeaderName, HeaderValue)],
        opts: &GetOptions,
        max_bytes: u64,
        cancel: &CancellationToken,
        mode: Mode,
    ) -> Result<reqwest::Response, HttpError> {
        let mut req = self
            .inner
            .get(url.clone())
            .header(ACCEPT, self.profile.accept());
        if mode == Mode::Streaming {
            req = req.header(ACCEPT_ENCODING, "identity");
        }
        for (name, value) in headers {
            req = req.header(name.clone(), value.clone());
        }
        if let Some(cond) = &opts.conditional {
            if let Some(etag) = &cond.etag {
                req = req.header(IF_NONE_MATCH, etag);
            }
            if let Some(lm) = &cond.last_modified {
                req = req.header(IF_MODIFIED_SINCE, lm);
            }
        }
        if let Some(start) = opts.range_start {
            req = req.header(RANGE, format!("bytes={start}-"));
            if let Some(v) = &opts.if_range {
                req = req.header(IF_RANGE, v);
            }
        }
        let timeout = self.config.request_timeout;
        let send = async {
            match mode {
                // reqwest's per-request timeout covers the body read too.
                Mode::Buffered => req.timeout(timeout).send().await,
                Mode::Streaming => match tokio::time::timeout(timeout, req.send()).await {
                    Ok(r) => r,
                    Err(_) => return Err(HttpError::Timeout(timeout)),
                },
            }
            .map_err(|e| HttpError::from_reqwest(&e, timeout))
        };
        let resp = tokio::select! {
            () = cancel.cancelled() => return Err(HttpError::Cancelled),
            r = send => r?,
        };
        if headers::content_length(resp.headers()).is_some_and(|len| len > max_bytes) {
            return Err(HttpError::BodyTooLarge { limit: max_bytes });
        }
        Ok(resp)
    }
}

/// How the body of a `GET` is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Buffered,
    Streaming,
}

/// Reads a whole body into memory under the cap and the token.
async fn read_body(
    mut resp: reqwest::Response,
    max_bytes: u64,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<Bytes, HttpError> {
    let mut body = BytesMut::new();
    loop {
        let chunk = tokio::select! {
            () = cancel.cancelled() => return Err(HttpError::Cancelled),
            c = resp.chunk() => c.map_err(|e| HttpError::from_reqwest(&e, timeout))?,
        };
        let Some(chunk) = chunk else { break };
        if (body.len() as u64).saturating_add(chunk.len() as u64) > max_bytes {
            return Err(HttpError::BodyTooLarge { limit: max_bytes });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

/// Sleeps unless cancelled; returns `false` when cancelled.
async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        () = cancel.cancelled() => false,
        () = tokio::time::sleep(d) => true,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn resp(headers: &[(&str, &str)]) -> Response {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            map.insert(
                HeaderName::from_bytes(k.as_bytes()).expect("name"),
                v.parse().expect("value"),
            );
        }
        Response {
            url: Url::parse("https://example.com/").expect("url"),
            status: StatusCode::OK,
            headers: map,
            body: Bytes::new(),
            redirects: vec![],
            permanent_redirect: false,
            elapsed: Duration::ZERO,
            attempts: 1,
        }
    }

    #[test]
    fn validator_accessors_trim_and_drop_empty_values() {
        let r = resp(&[("etag", " W/\"abc\" "), ("last-modified", "")]);
        assert_eq!(r.etag(), Some("W/\"abc\""));
        assert_eq!(r.last_modified(), None);
        assert_eq!(resp(&[]).etag(), None);
    }

    #[test]
    fn cache_max_age_parsing() {
        assert_eq!(
            resp(&[("cache-control", "public, max-age=600")]).cache_max_age(),
            Some(Duration::from_secs(600))
        );
        assert_eq!(
            resp(&[("cache-control", "s-maxage=30")]).cache_max_age(),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            resp(&[("cache-control", "no-store")]).cache_max_age(),
            Some(Duration::ZERO)
        );
        assert_eq!(
            resp(&[("cache-control", "no-cache, must-revalidate")]).cache_max_age(),
            Some(Duration::ZERO),
            "Podcast Index answers with no-cache; ToS §5 forbids keeping it"
        );
        assert_eq!(
            resp(&[("cache-control", "private, max-age=0")]).cache_max_age(),
            Some(Duration::ZERO)
        );
        assert_eq!(resp(&[]).cache_max_age(), None);
    }

    #[test]
    fn profiles_have_sane_caps() {
        assert!(Profile::Discovery.max_body_bytes() < Profile::Html.max_body_bytes());
        assert!(Profile::Html.max_body_bytes() < Profile::Feed.max_body_bytes());
        assert!(Profile::Feed.accept().contains("rss"));
        // Artwork is a small body from an untrusted URL: its cap sits far
        // below the media one, and it asks for images rather than `*/*`.
        assert!(Profile::Artwork.max_body_bytes() < Profile::Feed.max_body_bytes());
        assert!(Profile::Artwork.accept().starts_with("image/"));
    }
}
