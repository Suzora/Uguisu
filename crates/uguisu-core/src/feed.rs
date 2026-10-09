//! Feed refresh vocabulary: error classification, outcomes and the refresh
//! report (`docs/FEED_ENGINE.md`).

use serde::{Deserialize, Serialize};
use url::Url;

use crate::ids::{FetchId, PodcastId, SourceId};

/// Classification of a failed feed fetch or parse. Drives retry and
/// scheduling policies later; stored with the source and the fetch log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchErrorKind {
    /// Connection reset, transport failure, redirect problems.
    NetworkError,
    /// The request exceeded its deadline.
    Timeout,
    /// The host could not be resolved.
    DnsError,
    /// TLS handshake or certificate failure.
    TlsError,
    /// 4xx other than the dedicated variants.
    HttpClientError,
    /// 5xx.
    HttpServerError,
    /// 429.
    RateLimited,
    /// 401.
    Unauthorized,
    /// 403.
    Forbidden,
    /// 404 or 410.
    NotFound,
    /// The body is not well-formed XML.
    MalformedXml,
    /// Well-formed XML but not a feed Uguisu understands.
    UnsupportedFeed,
    /// The body exceeded the configured size limit.
    TooLarge,
    /// The XML nests deeper than the configured limit.
    TooDeep,
    /// The body is not XML at all (HTML, JSON, binary).
    InvalidContentType,
    /// A feed without any media items.
    InvalidPodcastFeed,
    /// Refused by the network policy (private address, blocked host).
    BlockedByPolicy,
    /// Cancelled by the caller or shutdown.
    Cancelled,
}

impl FetchErrorKind {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NetworkError => "network_error",
            Self::Timeout => "timeout",
            Self::DnsError => "dns_error",
            Self::TlsError => "tls_error",
            Self::HttpClientError => "http_client_error",
            Self::HttpServerError => "http_server_error",
            Self::RateLimited => "rate_limited",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::MalformedXml => "malformed_xml",
            Self::UnsupportedFeed => "unsupported_feed",
            Self::TooLarge => "too_large",
            Self::TooDeep => "too_deep",
            Self::InvalidContentType => "invalid_content_type",
            Self::InvalidPodcastFeed => "invalid_podcast_feed",
            Self::BlockedByPolicy => "blocked_by_policy",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Every variant.
    pub const ALL: [Self; 18] = [
        Self::NetworkError,
        Self::Timeout,
        Self::DnsError,
        Self::TlsError,
        Self::HttpClientError,
        Self::HttpServerError,
        Self::RateLimited,
        Self::Unauthorized,
        Self::Forbidden,
        Self::NotFound,
        Self::MalformedXml,
        Self::UnsupportedFeed,
        Self::TooLarge,
        Self::TooDeep,
        Self::InvalidContentType,
        Self::InvalidPodcastFeed,
        Self::BlockedByPolicy,
        Self::Cancelled,
    ];

    /// Whether a later retry can reasonably succeed without user action.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::NetworkError
                | Self::Timeout
                | Self::DnsError
                | Self::HttpServerError
                | Self::RateLimited
                | Self::Cancelled
        )
    }

    /// Whether the failure is about the feed's content rather than transport.
    #[must_use]
    pub const fn is_content(self) -> bool {
        matches!(
            self,
            Self::MalformedXml
                | Self::UnsupportedFeed
                | Self::TooLarge
                | Self::TooDeep
                | Self::InvalidContentType
                | Self::InvalidPodcastFeed
        )
    }

    /// Classifies an HTTP status code.
    #[must_use]
    pub const fn from_status(status: u16) -> Option<Self> {
        match status {
            200..=299 | 304 => None,
            401 => Some(Self::Unauthorized),
            403 => Some(Self::Forbidden),
            404 | 410 => Some(Self::NotFound),
            429 => Some(Self::RateLimited),
            400..=499 => Some(Self::HttpClientError),
            500..=599 => Some(Self::HttpServerError),
            _ => Some(Self::NetworkError),
        }
    }
}

impl std::fmt::Display for FetchErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a refresh found nothing new without parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotModifiedReason {
    /// The server answered `304 Not Modified`.
    Http304,
    /// The body was byte-identical to the last processed one.
    Fingerprint,
}

/// Outcome of a refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RefreshOutcome {
    /// A feed document was fetched and processed.
    Fetched,
    /// Nothing changed.
    NotModified {
        /// How that was determined.
        reason: NotModifiedReason,
    },
    /// The refresh failed; the previous state is retained.
    Failed {
        /// Classification.
        kind: FetchErrorKind,
        /// Detail.
        detail: String,
    },
}

impl RefreshOutcome {
    /// True for `Fetched` and `NotModified`.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        !matches!(self, Self::Failed { .. })
    }

    /// The serialized `outcome` tag, without the failure's `detail`, which
    /// may quote the feed URL whole.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Fetched => "fetched",
            Self::NotModified { .. } => "not_modified",
            Self::Failed { .. } => "failed",
        }
    }
}

/// How an item compared to the stored episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeChangeKind {
    /// New episode.
    Added,
    /// Existing episode with changed fields.
    Updated,
    /// Existing episode, nothing changed.
    Unchanged,
    /// Item could not be fully parsed.
    Malformed,
    /// Stored episode missing from the feed for the configured streak.
    PotentiallyRemoved,
}

/// What the HTTP exchange looked like.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HttpSummary {
    /// Status code, when a response was received.
    pub status: Option<u16>,
    /// Final URL after redirects.
    pub final_url: Option<Url>,
    /// Number of redirects followed.
    pub redirects: u32,
    /// The `ETag` changed compared to the stored one.
    pub etag_changed: bool,
    /// `ETag` of the response.
    pub etag: Option<String>,
    /// `Last-Modified` of the response.
    pub last_modified: Option<String>,
    /// Body size in bytes.
    pub bytes: Option<u64>,
    /// Whether a conditional request was sent.
    pub conditional: bool,
}

/// Change counts per episode class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodeCounts {
    /// Items in the feed.
    pub seen: u32,
    /// New episodes.
    pub added: u32,
    /// Changed episodes.
    pub updated: u32,
    /// Unchanged episodes.
    pub unchanged: u32,
    /// Items that could not be fully parsed.
    pub malformed: u32,
    /// Episodes newly detected as removed.
    pub removed_detected: u32,
    /// Episodes stored as candidate duplicates.
    pub ambiguous: u32,
}

/// Feed URL status after a refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FeedUrlStatus {
    /// Nothing announced.
    Unchanged,
    /// A new location was announced but could not be verified yet.
    ChangeDetected {
        /// The announced URL.
        announced: Url,
        /// Why it was not adopted.
        reason: String,
    },
    /// The source was migrated.
    Changed {
        /// Previous URL.
        from: Url,
        /// New URL.
        to: Url,
        /// How the change was announced.
        via: crate::model::ReplacementReason,
    },
}

/// One row of the fetch log (`feed_fetches`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[allow(clippy::struct_excessive_bools)] // mirrors the log columns one to one
pub struct FeedFetch {
    /// Identifier.
    pub id: FetchId,
    /// Podcast.
    pub podcast_id: PodcastId,
    /// Source that was fetched.
    pub source_id: SourceId,
    /// When the attempt started.
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: time::OffsetDateTime,
    /// Outcome.
    #[serde(flatten)]
    pub outcome: RefreshOutcome,
    /// HTTP summary.
    pub http: HttpSummary,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Some items were malformed.
    pub partial: bool,
    /// The document was truncated.
    pub truncated: bool,
    /// The body fingerprint differed from the previous one.
    pub fingerprint_changed: bool,
    /// Episode counts.
    pub episodes: EpisodeCounts,
    /// Podcast metadata changed.
    pub podcast_changed: bool,
    /// A new feed URL was announced.
    pub url_change_detected: bool,
    /// Warnings.
    pub warnings: Vec<String>,
}

/// Everything a refresh found out (brief §56).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RefreshReport {
    /// Report schema version.
    pub schema: u32,
    /// The podcast.
    pub podcast_id: PodcastId,
    /// The source that was refreshed (the current one before any migration).
    pub source_id: SourceId,
    /// The fetch log entry.
    pub fetch_id: FetchId,
    /// Outcome.
    #[serde(flatten)]
    pub outcome: RefreshOutcome,
    /// HTTP exchange summary.
    pub http: HttpSummary,
    /// Podcast-level fields that changed.
    pub podcast_changed_fields: Vec<String>,
    /// Episode counts.
    pub episodes: EpisodeCounts,
    /// Feed URL status.
    pub feed_url: FeedUrlStatus,
    /// Parser and pipeline warnings.
    pub warnings: Vec<String>,
    /// Whether the feed document was truncated by a limit or an XML error.
    pub truncated: bool,
    /// Whether removal detection was skipped and why.
    pub removal_suppressed: Option<String>,
    /// Wall-clock duration.
    pub duration_ms: u64,
}

impl RefreshReport {
    /// Current report schema.
    pub const SCHEMA: u32 = 1;

    /// True when the fetch succeeded but not every item made it: some
    /// were malformed, or the parser stopped early (`truncated`).
    #[must_use]
    pub const fn is_partial(&self) -> bool {
        matches!(self.outcome, RefreshOutcome::Fetched)
            && (self.episodes.malformed > 0 || self.truncated)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn error_kinds_round_trip_and_classify() {
        for k in FetchErrorKind::ALL {
            assert_eq!(FetchErrorKind::parse(k.as_str()), Some(k));
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
        assert_eq!(FetchErrorKind::from_status(200), None);
        assert_eq!(FetchErrorKind::from_status(304), None);
        assert_eq!(
            FetchErrorKind::from_status(404),
            Some(FetchErrorKind::NotFound)
        );
        assert_eq!(
            FetchErrorKind::from_status(410),
            Some(FetchErrorKind::NotFound)
        );
        assert_eq!(
            FetchErrorKind::from_status(429),
            Some(FetchErrorKind::RateLimited)
        );
        assert_eq!(
            FetchErrorKind::from_status(418),
            Some(FetchErrorKind::HttpClientError)
        );
        assert_eq!(
            FetchErrorKind::from_status(503),
            Some(FetchErrorKind::HttpServerError)
        );
        assert!(FetchErrorKind::Timeout.is_transient());
        assert!(!FetchErrorKind::MalformedXml.is_transient());
        assert!(FetchErrorKind::MalformedXml.is_content());
    }

    #[test]
    fn outcome_serializes_with_tag() {
        let o = RefreshOutcome::NotModified {
            reason: NotModifiedReason::Http304,
        };
        assert_eq!(
            serde_json::to_string(&o).unwrap(),
            r#"{"outcome":"not_modified","reason":"http304"}"#
        );
        assert!(o.is_success());
        let f = RefreshOutcome::Failed {
            kind: FetchErrorKind::Timeout,
            detail: "x".into(),
        };
        assert!(!f.is_success());
    }
}
