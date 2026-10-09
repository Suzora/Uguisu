//! `feed inspect`: fetch and parse a feed URL and describe what the engine
//! would make of it, without touching the database. Works without an
//! [`Engine`](crate::Engine) so the command needs neither a data directory
//! nor the process lock.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::UguisuError;
use uguisu_core::config::FeedLimits;
use uguisu_core::feed::{FetchErrorKind, HttpSummary};
use uguisu_core::ids::EpisodeId;
use uguisu_core::model::{DateQuality, FeedKind};
use uguisu_feed::identity::{resolve_identities, signals};
use uguisu_feed::normalize::{normalize_channel, normalize_item};
use uguisu_feed::parse;
use uguisu_http::{CancellationToken, GetOptions, HttpClient};
use url::Url;

use crate::refresh::{error_kind, looks_like_xml, parse_error_kind};

/// Number of episodes listed in an inspection.
pub const PREVIEW_ITEMS: usize = 10;

/// One item as the engine would see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct InspectedItem {
    /// Position in the feed.
    pub index: usize,
    /// Normalized title.
    pub title: String,
    /// Identity key the item would get.
    pub identity_key: String,
    /// Publication instant, when parseable.
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    /// Date quality.
    pub published_at_quality: DateQuality,
    /// Duration in seconds, when parseable.
    pub duration_secs: Option<u32>,
    /// Primary enclosure URL.
    pub enclosure_url: Option<Url>,
    /// Number of enclosures (primary and alternates).
    pub enclosures: usize,
    /// Normalization warnings.
    pub warnings: Vec<String>,
}

/// What `feed inspect` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Inspection {
    /// Report schema version.
    pub schema: u32,
    /// The URL that was asked for.
    pub url: Url,
    /// HTTP facts of the fetch.
    pub http: HttpSummary,
    /// Syntax family.
    pub kind: FeedKind,
    /// Encoding the body was decoded with.
    pub encoding: String,
    /// Channel title.
    pub title: String,
    /// Channel author.
    pub author: Option<String>,
    /// `podcast:guid`.
    pub podcast_guid: Option<String>,
    /// Language.
    pub language: Option<String>,
    /// Website.
    pub website: Option<Url>,
    /// `itunes:new-feed-url`.
    pub new_feed_url: Option<Url>,
    /// `podcast:locked`.
    pub locked: Option<bool>,
    /// Number of categories.
    pub categories: usize,
    /// Items parsed.
    pub items: usize,
    /// Items with at least one enclosure.
    pub items_with_enclosure: usize,
    /// Items the parser had to isolate.
    pub malformed_items: usize,
    /// Whether the parser stopped early.
    pub truncated: bool,
    /// Whether the feed looks like a podcast feed.
    pub looks_like_podcast: bool,
    /// Identity source → number of items.
    pub identity_sources: BTreeMap<String, usize>,
    /// The first items.
    pub preview: Vec<InspectedItem>,
    /// Parser, channel and item warnings (capped).
    pub warnings: Vec<String>,
    /// Total number of warnings before capping.
    pub warning_count: usize,
    /// Wall-clock time of fetch and parse.
    pub duration_ms: u64,
}

impl Inspection {
    /// Current schema version.
    pub const SCHEMA: u32 = 1;
}

/// Longest warning list an inspection carries.
pub const MAX_WARNINGS: usize = 50;

/// Fetches and parses `url` with the given client and limits.
#[allow(clippy::too_many_lines)] // fetch, parse, normalize, summarize: one linear pass
pub async fn inspect(
    client: &HttpClient,
    limits: &FeedLimits,
    url: &Url,
    cancel: CancellationToken,
) -> Result<Inspection, UguisuError> {
    let started = std::time::Instant::now();
    let get = GetOptions {
        max_bytes: Some(limits.max_bytes),
        cancel: Some(cancel),
        ..GetOptions::default()
    };
    let response = client.get_with(url, &get).await.map_err(|e| {
        let kind = error_kind(&e);
        if kind == FetchErrorKind::BlockedByPolicy {
            UguisuError::BlockedByPolicy(e.to_string())
        } else {
            UguisuError::Network {
                kind,
                detail: e.to_string(),
            }
        }
    })?;
    let http = HttpSummary {
        status: Some(response.status.as_u16()),
        final_url: Some(response.url.clone()),
        redirects: u32::try_from(response.redirects.len()).unwrap_or(u32::MAX),
        etag_changed: false,
        etag: response.etag().map(str::to_owned),
        last_modified: response.last_modified().map(str::to_owned),
        bytes: Some(response.body.len() as u64),
        conditional: false,
    };
    if !response.status.is_success() {
        return Err(UguisuError::Network {
            kind: FetchErrorKind::from_status(response.status.as_u16())
                .unwrap_or(FetchErrorKind::HttpClientError),
            detail: format!("http status {}", response.status.as_u16()),
        });
    }
    if !looks_like_xml(&response.body) {
        return Err(UguisuError::Feed {
            kind: FetchErrorKind::InvalidContentType,
            detail: format!(
                "body is not XML (content-type {})",
                response.content_type().unwrap_or("unknown")
            ),
        });
    }
    let parsed = parse(&response.body, limits).map_err(|e| UguisuError::Feed {
        kind: parse_error_kind(&e),
        detail: e.to_string(),
    })?;
    let now = OffsetDateTime::now_utc();
    let channel = normalize_channel(&parsed.channel);
    let normalized: Vec<_> = parsed
        .items
        .iter()
        .map(|i| normalize_item(i, EpisodeId::new(), now))
        .collect();
    let sigs: Vec<_> = parsed
        .items
        .iter()
        .zip(&normalized)
        .map(|(p, n)| signals(p, n))
        .collect();
    let identities = resolve_identities(&sigs);
    let mut identity_sources: BTreeMap<String, usize> = BTreeMap::new();
    for id in &identities {
        *identity_sources
            .entry(id.source.as_str().to_owned())
            .or_default() += 1;
    }
    let mut warnings: Vec<String> = parsed.warnings.clone();
    warnings.extend(channel.warnings.iter().cloned());
    for m in &parsed.malformed_items {
        warnings.push(format!("item {}: malformed: {}", m.index, m.reason));
    }
    for (item, n) in parsed.items.iter().zip(&normalized) {
        for w in &n.warnings {
            warnings.push(format!("item {}: {w}", item.index));
        }
    }
    let warning_count = warnings.len();
    warnings.truncate(MAX_WARNINGS);
    let preview = parsed
        .items
        .iter()
        .zip(&normalized)
        .zip(&identities)
        .take(PREVIEW_ITEMS)
        .map(|((item, n), id)| InspectedItem {
            index: item.index,
            title: n.title.clone(),
            identity_key: id.key.clone(),
            published_at: n.published.value,
            published_at_quality: n.published.quality,
            duration_secs: n.duration_secs,
            enclosure_url: n
                .enclosures
                .iter()
                .find(|e| e.is_primary)
                .map(|e| e.url.clone()),
            enclosures: n.enclosures.len(),
            warnings: n.warnings.clone(),
        })
        .collect();
    Ok(Inspection {
        schema: Inspection::SCHEMA,
        url: url.clone(),
        http,
        kind: parsed.kind,
        encoding: parsed.encoding.clone(),
        title: channel.title,
        author: channel.author,
        podcast_guid: channel.podcast_guid,
        language: channel.language,
        website: channel.website,
        new_feed_url: channel.new_feed_url,
        locked: channel.locked,
        categories: channel.categories.len(),
        items: parsed.items.len(),
        items_with_enclosure: parsed.stats.items_with_enclosure,
        malformed_items: parsed.malformed_items.len(),
        truncated: parsed.truncated,
        looks_like_podcast: parsed.looks_like_podcast(),
        identity_sources,
        preview,
        warnings,
        warning_count,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

impl crate::Engine {
    /// Inspects a URL with the engine's feed client and limits (no writes).
    pub async fn inspect_url(
        &self,
        url: &Url,
        cancel: CancellationToken,
    ) -> Result<Inspection, UguisuError> {
        inspect(self.feed_client(), &self.config().feed.limits, url, cancel).await
    }
}

/// Builds the feed client `feed inspect` uses when no engine is open.
pub fn standalone_client(
    network: &uguisu_core::config::NetworkConfig,
) -> Result<HttpClient, UguisuError> {
    HttpClient::new(
        uguisu_http::Profile::Feed,
        uguisu_http::ClientConfig::from_network(network),
    )
    .map_err(|e| UguisuError::Config(format!("feed http client: {e}")))
}
