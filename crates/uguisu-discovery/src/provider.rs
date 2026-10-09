//! The provider abstraction.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uguisu_core::provider::ProviderId;
use uguisu_http::{HttpError, ThrottleConfig};
use url::Url;

use crate::candidate::PodcastCandidate;
use crate::query::NormalizedQuery;

/// What a provider can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[allow(clippy::struct_excessive_bools)] // a flag set, not state
pub struct Capabilities {
    /// Free-text search.
    pub search: bool,
    /// Lookup by the provider's own id.
    pub lookup_by_id: bool,
    /// Lookup by feed URL.
    pub lookup_by_feed_url: bool,
    /// Lookup by iTunes id.
    pub lookup_by_itunes_id: bool,
    /// Lookup by `podcast:guid`.
    pub lookup_by_guid: bool,
    /// Supplies a popularity signal.
    pub popularity: bool,
}

/// Static description of a provider.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProviderInfo {
    /// Identifier.
    pub id: ProviderId,
    /// Human-readable name.
    pub name: &'static str,
    /// Attribution text the UI must show when results from this provider are displayed.
    pub attribution: Option<&'static str>,
    /// Link to the provider's documentation or terms.
    pub docs_url: &'static str,
    /// Capabilities.
    pub capabilities: Capabilities,
    /// Whether the user must supply credentials.
    pub requires_credentials: bool,
    /// Default rate limit / concurrency, from the provider's documentation.
    #[serde(skip)]
    pub throttle: ThrottleConfig,
    /// Relative trust in this provider's data (0..1), used by merge and ranking.
    pub trust: f32,
}

/// A reference a provider can look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRef {
    /// The provider's own id (Apple collection id, Podcast Index feed id…).
    Id(String),
    /// A feed URL.
    FeedUrl(Url),
    /// An iTunes collection id.
    ItunesId(u64),
    /// A `podcast:guid`.
    PodcastGuid(String),
}

impl ProviderRef {
    /// A stable key for caches and logs.
    ///
    /// Written out rather than derived from `Debug`: a `Debug` rendering
    /// is a courtesy to a programmer reading a log, not a format anything
    /// may depend on, and a cache that outlives the process (ADR 0030)
    /// would be silently invalidated by a tidy-up of the derive.
    #[must_use]
    pub fn cache_key(&self) -> String {
        match self {
            Self::Id(v) => format!("id:{v}"),
            Self::FeedUrl(url) => format!("feed:{url}"),
            Self::ItunesId(n) => format!("itunes:{n}"),
            Self::PodcastGuid(v) => format!("guid:{v}"),
        }
    }
}

/// Per-call context.
#[derive(Debug, Clone)]
pub struct ProviderContext {
    /// Cancellation token; providers must stop promptly when it fires.
    pub cancel: CancellationToken,
    /// Absolute deadline for the call.
    pub deadline: Option<Instant>,
    /// Maximum results wanted.
    pub limit: usize,
    /// Storefront / country hint (ISO 3166-1 alpha-2).
    pub country: Option<String>,
}

impl Default for ProviderContext {
    fn default() -> Self {
        Self {
            cancel: CancellationToken::new(),
            deadline: None,
            limit: 25,
            country: None,
        }
    }
}

impl ProviderContext {
    /// Time left until the deadline (`None` = unbounded).
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }
}

/// Why a provider call failed.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Provider asked us to slow down.
    #[error("rate limited{}", .retry_after.map(|d| format!(" (retry after {d:?})")).unwrap_or_default())]
    RateLimited {
        /// Suggested wait.
        retry_after: Option<Duration>,
    },
    /// Provider is down or answered with a server error.
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    /// Provider needs credentials that are not configured.
    #[error("credentials required")]
    AuthRequired,
    /// Provider rejected the configured credentials.
    #[error("credentials rejected")]
    AuthRejected,
    /// Response could not be parsed into the expected shape.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    /// Transport-level failure.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// The call exceeded its deadline.
    #[error("timed out")]
    Timeout,
    /// The call was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The provider does not support this operation.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl ProviderError {
    /// Stable identifier for logs and API bodies.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RateLimited { .. } => "rate_limited",
            Self::Unavailable(_) => "unavailable",
            Self::AuthRequired => "auth_required",
            Self::AuthRejected => "auth_rejected",
            Self::InvalidResponse(_) => "invalid_response",
            Self::Http(e) => e.kind(),
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unsupported(_) => "unsupported",
        }
    }

    /// Whether the failure is likely to clear on its own (counts toward the circuit breaker).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Unavailable(_) | Self::Timeout => true,
            Self::Http(e) => {
                e.is_retryable() || matches!(e, HttpError::Dns { .. } | HttpError::Connect(_))
            }
            Self::AuthRequired
            | Self::AuthRejected
            | Self::InvalidResponse(_)
            | Self::Cancelled
            | Self::Unsupported(_) => false,
        }
    }

    /// Whether the failure is about credentials.
    pub const fn is_auth(&self) -> bool {
        matches!(self, Self::AuthRequired | Self::AuthRejected)
    }

    /// Maps an HTTP status the provider returned.
    pub fn from_status(status: u16, retry_after: Option<Duration>) -> Self {
        match status {
            401 | 403 => Self::AuthRejected,
            429 => Self::RateLimited { retry_after },
            500..=599 => Self::Unavailable(format!("http {status}")),
            other => Self::InvalidResponse(format!("unexpected http status {other}")),
        }
    }
}

/// A provider result plus caching hints.
#[derive(Debug, Clone)]
pub struct ProviderResponse<T> {
    /// The payload.
    pub value: T,
    /// `Cache-Control: max-age` from the provider, when present (Podcast
    /// Index's terms require honouring it).
    pub cache_max_age: Option<Duration>,
}

impl<T> ProviderResponse<T> {
    /// Wraps a value without cache hints.
    pub const fn new(value: T) -> Self {
        Self {
            value,
            cache_max_age: None,
        }
    }
}

/// A podcast directory or index.
#[async_trait]
pub trait DiscoveryProvider: Send + Sync {
    /// Static description.
    fn info(&self) -> ProviderInfo;

    /// Free-text search.
    async fn search(
        &self,
        query: &NormalizedQuery,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError>;

    /// Lookup by reference. Default: unsupported.
    async fn lookup(
        &self,
        reference: &ProviderRef,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Option<PodcastCandidate>>, ProviderError> {
        let _ = (reference, ctx);
        Err(ProviderError::Unsupported("lookup".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_and_transience() {
        assert!(matches!(
            ProviderError::from_status(429, None),
            ProviderError::RateLimited { .. }
        ));
        assert!(matches!(
            ProviderError::from_status(401, None),
            ProviderError::AuthRejected
        ));
        assert!(ProviderError::from_status(503, None).is_transient());
        assert!(!ProviderError::from_status(418, None).is_transient());
        assert!(ProviderError::AuthRequired.is_auth());
        assert_eq!(ProviderError::Timeout.kind(), "timeout");
    }
}
