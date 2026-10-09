//! Built-in discovery providers.
//!
//! Each provider maps its own response format to [`PodcastCandidate`] and
//! nothing provider-specific escapes its module. Every network call goes
//! through `uguisu-http`, so the SSRF policy, size caps and retries apply
//! uniformly.
//!
//! | Provider | Status (2026-09-17, after Part B) |
//! |---|---|
//! | [`apple`] | **live-verified**: recorded fixtures in `tests/fixtures/discovery/live/apple` |
//! | [`gpoddernet`] | **live-verified**: recorded fixtures in `tests/fixtures/discovery/live/gpoddernet` |
//! | [`podcastindex`] | implemented against the OpenAPI spec; **live-verified only for the unauthenticated path** (401 shapes recorded); authenticated calls need a user key |
//! | fyyd | after v1 (ADR 0005) — live schema observed (`tests/fixtures/discovery/live/fyyd`), primary documentation still unreachable (see `docs/research/DISCOVERY_ECOSYSTEM.md`) |

pub mod apple;
pub mod gpoddernet;
pub mod podcastindex;

use std::time::Duration;

use time::OffsetDateTime;
use uguisu_http::{GetOptions, HttpClient, HttpError, Response, Url};

use crate::candidate::PodcastCandidate;
use crate::provider::{ProviderContext, ProviderError};

/// Performs a GET with the call's cancellation token and maps transport errors.
pub(crate) async fn get(
    client: &HttpClient,
    url: &Url,
    ctx: &ProviderContext,
    headers: Vec<(uguisu_http::HeaderName, uguisu_http::HeaderValue)>,
) -> Result<Response, ProviderError> {
    let opts = GetOptions {
        cancel: Some(ctx.cancel.clone()),
        headers,
        ..GetOptions::default()
    };
    client.get_with(url, &opts).await.map_err(|e| match e {
        HttpError::Cancelled => ProviderError::Cancelled,
        HttpError::Timeout(_) => ProviderError::Timeout,
        other => ProviderError::Http(other),
    })
}

/// Parses a JSON body, reporting a short excerpt on failure.
pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(
    resp: &Response,
) -> Result<T, ProviderError> {
    serde_json::from_slice(&resp.body).map_err(|e| {
        let excerpt: String =
            String::from_utf8_lossy(&resp.body[..resp.body.len().min(120)]).into_owned();
        ProviderError::InvalidResponse(format!("{e} (body starts with: {excerpt:?})"))
    })
}

/// Converts a non-success status into a provider error.
pub(crate) fn check_status(resp: &Response) -> Result<(), ProviderError> {
    if resp.status.is_success() {
        return Ok(());
    }
    Err(ProviderError::from_status(
        resp.status.as_u16(),
        uguisu_http::retry_after(&resp.headers),
    ))
}

/// Builds `base/path?params`.
pub(crate) fn build_url(
    base: &Url,
    path: &str,
    params: &[(&str, &str)],
) -> Result<Url, ProviderError> {
    let mut url = base
        .join(path)
        .map_err(|e| ProviderError::InvalidResponse(format!("bad url: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    Ok(url)
}

/// Position-based confidence: providers return relevance-ordered lists, so
/// earlier entries get slightly more weight (1.0, 0.98, 0.96 … ≥ 0.5).
#[allow(clippy::cast_precision_loss)] // positions are tiny
pub(crate) fn positional_confidence(position: usize) -> f32 {
    (1.0 - 0.02 * position as f32).max(0.5)
}

/// Parses a URL, dropping empty and invalid values.
pub(crate) fn opt_url(s: Option<&str>) -> Option<Url> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| Url::parse(s).ok())
}

/// Non-empty trimmed string.
pub(crate) fn opt_str(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Unix seconds to a timestamp, ignoring zero/negative values.
pub(crate) fn unix_ts(secs: Option<i64>) -> Option<OffsetDateTime> {
    secs.filter(|s| *s > 0)
        .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
}

/// Current time.
pub(crate) fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Truncates a candidate list to the requested limit.
pub(crate) fn truncate(mut items: Vec<PodcastCandidate>, limit: usize) -> Vec<PodcastCandidate> {
    items.truncate(limit.max(1));
    items
}

/// Cache hint from a response.
pub(crate) fn cache_hint(resp: &Response) -> Option<Duration> {
    resp.cache_max_age()
}
