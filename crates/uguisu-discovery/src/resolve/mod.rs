//! Feed resolution: from a pasted URL or a selected candidate to a verified
//! podcast feed, with a bounded request budget and a step-by-step record.

// `ResolveError` is a serialized wire type returned only after network I/O; boxing its
// fields would reshape every caller to save bytes the I/O dwarfs.
#![allow(clippy::result_large_err)]

pub mod classify;
pub mod html;
pub mod patterns;

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uguisu_core::provider::ProviderId;
use uguisu_core::redact;
use uguisu_feed::{FeedKind, FeedProbe, ProbeError};
use uguisu_http::{GetOptions, HeaderName, HeaderValue, HttpClient, HttpError, Response, Url};

use crate::candidate::PodcastCandidate;
use crate::provider::{ProviderContext, ProviderError, ProviderRef};
use crate::query::NormalizedQuery;
use crate::registry::ProviderRegistry;
use classify::{UrlClass, classify};

/// Resolver limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverConfig {
    /// Maximum HTTP requests per resolution (each redirect hop counts inside `uguisu-http`, not here).
    pub max_requests: usize,
    /// Wall-clock budget per resolution.
    pub time_budget: Duration,
    /// Try the well-known paths when a site has no feed links.
    pub try_well_known_paths: bool,
    /// Try the HTTPS variant of an HTTP feed.
    pub https_upgrade: bool,
    /// Fetch `atom:link rel=self` to confirm it as the canonical URL.
    pub confirm_self_link: bool,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_requests: 8,
            time_budget: Duration::from_secs(30),
            try_well_known_paths: true,
            https_upgrade: true,
            confirm_self_link: true,
        }
    }
}

/// Kind of a resolution step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Input classified.
    Classify,
    /// A provider was asked for the feed.
    ProviderLookup,
    /// A URL was fetched.
    Fetch,
    /// A body was inspected.
    Sniff,
    /// Feed links were extracted from HTML.
    Autodiscovery,
    /// A platform pattern produced a candidate.
    PlatformPattern,
    /// A well-known path was tried.
    WellKnownPath,
    /// A feed body was validated.
    Validate,
    /// The self link was confirmed as canonical.
    Canonicalize,
    /// The HTTPS variant was tried.
    HttpsUpgrade,
}

impl StepKind {
    /// Stable string form, as the API and the recorded provenance spell
    /// it. Written out rather than derived from `Debug`, which is a
    /// courtesy to a programmer reading a log and not a format anything
    /// may depend on.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classify => "classify",
            Self::ProviderLookup => "provider_lookup",
            Self::Fetch => "fetch",
            Self::Sniff => "sniff",
            Self::Autodiscovery => "autodiscovery",
            Self::PlatformPattern => "platform_pattern",
            Self::WellKnownPath => "well_known_path",
            Self::Validate => "validate",
            Self::Canonicalize => "canonicalize",
            Self::HttpsUpgrade => "https_upgrade",
        }
    }
}

/// One step of a resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolutionStep {
    /// Kind.
    pub kind: StepKind,
    /// URL involved, if any.
    pub url: Option<Url>,
    /// Whether the step succeeded.
    pub ok: bool,
    /// Detail.
    pub detail: String,
    /// Time since the resolution started.
    pub elapsed_ms: u64,
}

/// A verified feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolvedFeed {
    /// The input as given.
    pub input: String,
    /// The feed URL to subscribe to.
    pub feed_url: Url,
    /// Canonical URL from `atom:link rel="self"` when it was confirmed.
    pub canonical_url: Option<Url>,
    /// `itunes:new-feed-url` when the feed announces a move.
    pub moved_to: Option<Url>,
    /// Podcast website.
    pub website: Option<Url>,
    /// Title.
    pub title: Option<String>,
    /// Author.
    pub author: Option<String>,
    /// Description (truncated).
    pub description: Option<String>,
    /// Artwork.
    pub artwork: Option<Url>,
    /// `podcast:guid`.
    pub podcast_guid: Option<String>,
    /// Language.
    pub language: Option<String>,
    /// Items seen in the feed body.
    pub item_count: usize,
    /// Items with media.
    pub items_with_media: usize,
    /// Newest item date.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub newest_item: Option<OffsetDateTime>,
    /// `podcast:locked`.
    pub locked: Option<bool>,
    /// Feed syntax.
    pub feed_kind: FeedKind,
    /// Steps taken.
    pub provenance: Vec<ResolutionStep>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
    /// When the feed was verified.
    #[serde(with = "time::serde::rfc3339")]
    pub verified_at: OffsetDateTime,
}

/// Why resolution failed.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ResolveError {
    /// The input is not a URL (callers may search for it instead).
    #[error("`{input}` is not a URL")]
    NotAUrl {
        /// The input.
        input: String,
    },
    /// The URL returned something that is neither a feed nor HTML.
    #[error("{url} is not a feed: {detail}")]
    NotAFeed {
        /// URL.
        url: Url,
        /// Detail.
        detail: String,
    },
    /// HTML was fetched but no podcast feed could be found from it.
    #[error("no podcast feed found for {url} (tried {})", tried.len())]
    NoFeedLinkFound {
        /// Page URL.
        url: Url,
        /// Candidate feed URLs that were tried.
        tried: Vec<Url>,
    },
    /// A feed was found but it does not look like a podcast feed.
    #[error("{url} is a feed but {reason}")]
    FeedInvalid {
        /// Feed URL.
        url: Url,
        /// Reason.
        reason: String,
    },
    /// The server answered with an error status.
    #[error("{url} answered with http {status}")]
    HttpStatus {
        /// URL.
        url: Url,
        /// Status.
        status: u16,
    },
    /// Network failure.
    #[error("network error for {url}: {detail}")]
    Network {
        /// URL.
        url: Url,
        /// Error kind from `uguisu-http`.
        error_kind: String,
        /// Detail.
        detail: String,
    },
    /// Refused by the SSRF policy.
    #[error("{url} blocked by network policy: {detail}")]
    BlockedByPolicy {
        /// URL.
        url: Url,
        /// Detail.
        detail: String,
    },
    /// A provider needed for a directory page is unavailable.
    #[error("provider {provider} unavailable: {detail}")]
    ProviderUnavailable {
        /// Provider.
        provider: String,
        /// Detail.
        detail: String,
    },
    /// The platform publishes no RSS feed.
    #[error("{platform} does not publish RSS feeds")]
    NoFeedAvailable {
        /// Platform name.
        platform: String,
        /// Suggested search term, when one can be derived.
        hint: Option<String>,
    },
    /// The request or time budget was exhausted.
    #[error("resolution budget exhausted after {requests} requests")]
    BudgetExceeded {
        /// Requests made.
        requests: usize,
    },
    /// Cancelled.
    #[error("cancelled")]
    Cancelled,
}

impl ResolveError {
    /// Stable identifier.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotAUrl { .. } => "not_a_url",
            Self::NotAFeed { .. } => "not_a_feed",
            Self::NoFeedLinkFound { .. } => "no_feed_link_found",
            Self::FeedInvalid { .. } => "feed_invalid",
            Self::HttpStatus { .. } => "http_status",
            Self::Network { .. } => "network",
            Self::BlockedByPolicy { .. } => "blocked_by_policy",
            Self::ProviderUnavailable { .. } => "provider_unavailable",
            Self::NoFeedAvailable { .. } => "no_feed_available",
            Self::BudgetExceeded { .. } => "budget_exceeded",
            Self::Cancelled => "cancelled",
        }
    }

    /// What the user can do next.
    pub fn suggestion(&self) -> &'static str {
        match self {
            Self::NotAUrl { .. } => "search for the podcast by name or paste its RSS URL",
            Self::NotAFeed { .. } | Self::NoFeedLinkFound { .. } => {
                "paste the RSS URL directly (look for an RSS link on the podcast's website)"
            }
            Self::FeedInvalid { .. } => {
                "this feed has no episodes with media; check that it is the podcast feed"
            }
            Self::HttpStatus { .. } | Self::Network { .. } => "check the URL and retry later",
            Self::BlockedByPolicy { .. } => {
                "private or local addresses are refused; allow the host explicitly if this is intended"
            }
            Self::ProviderUnavailable { .. } => "retry later or paste the RSS URL directly",
            Self::NoFeedAvailable { .. } => "search for the podcast by name to find its RSS feed",
            Self::BudgetExceeded { .. } => "paste the RSS URL directly",
            Self::Cancelled => "retry",
        }
    }
}

/// A failed resolution with the steps that led there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolveFailure {
    /// The error.
    pub error: ResolveError,
    /// Steps taken.
    pub provenance: Vec<ResolutionStep>,
}

/// Resolves inputs to verified feeds.
#[derive(Debug, Clone)]
pub struct Resolver {
    client: HttpClient,
    registry: Option<Arc<ProviderRegistry>>,
    config: ResolverConfig,
}

struct Session {
    started: Instant,
    requests: usize,
    steps: Vec<ResolutionStep>,
    cancel: CancellationToken,
    tried: Vec<Url>,
}

impl Session {
    fn step(&mut self, kind: StepKind, url: Option<&Url>, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::debug!(step = ?kind, url = url.map(|u| tracing::field::display(redact::urls(u.as_str()))), ok, detail = %redact::urls(&detail), "feed resolution step");
        self.steps.push(ResolutionStep {
            kind,
            url: url.cloned(),
            ok,
            detail,
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
    }

    fn fail(self, error: ResolveError) -> ResolveFailure {
        tracing::info!(kind = error.kind(), error = %redact::urls(&error.to_string()), "feed resolution failed");
        ResolveFailure {
            error,
            provenance: self.steps,
        }
    }
}

enum Fetched {
    Feed(Box<FeedProbe>, Url),
    Html(String, Url),
}

impl Resolver {
    /// Creates a resolver. The registry enables directory-page lookups.
    pub fn new(
        client: HttpClient,
        registry: Option<Arc<ProviderRegistry>>,
        config: ResolverConfig,
    ) -> Self {
        Self {
            client,
            registry,
            config,
        }
    }

    /// Resolves free text: URLs are fetched, anything else is `NotAUrl`.
    pub async fn resolve(
        &self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<ResolvedFeed, ResolveFailure> {
        let mut s = Session {
            started: Instant::now(),
            requests: 0,
            steps: Vec::new(),
            cancel,
            tried: Vec::new(),
        };
        let query = NormalizedQuery::parse(input);
        let Some(url) = query.url().cloned() else {
            s.step(StepKind::Classify, None, false, "input is not a URL");
            return Err(s.fail(ResolveError::NotAUrl {
                input: input.trim().to_owned(),
            }));
        };
        tracing::info!(url = %redact::urls(url.as_str()), "feed resolution started");
        let class = classify(&url);
        s.step(StepKind::Classify, Some(&url), true, format!("{class:?}"));
        let result = match class {
            UrlClass::SpotifyShow { .. } => Err(ResolveError::NoFeedAvailable {
                platform: "Spotify".into(),
                hint: None,
            }),
            UrlClass::YouTube => Err(ResolveError::NoFeedAvailable {
                platform: "YouTube".into(),
                hint: None,
            }),
            UrlClass::FyydPage { .. } => Err(ResolveError::ProviderUnavailable {
                provider: "fyyd".into(),
                detail: "the fyyd provider is not implemented yet".into(),
            }),
            UrlClass::ApplePage { id, .. } => {
                self.via_provider(
                    &mut s,
                    ProviderId::APPLE,
                    ProviderRef::ItunesId(id),
                    Some(id),
                    input,
                )
                .await
            }
            UrlClass::PodcastIndexPage { id } => {
                self.via_provider(
                    &mut s,
                    ProviderId::PODCAST_INDEX,
                    ProviderRef::Id(id.to_string()),
                    None,
                    input,
                )
                .await
            }
            UrlClass::Generic => self.resolve_url(&mut s, &url, input).await,
        };
        match result {
            Ok(feed) => {
                tracing::info!(feed_url = %redact::urls(feed.feed_url.as_str()), requests = s.requests, "feed resolution succeeded");
                Ok(feed)
            }
            Err(e) => Err(s.fail(e)),
        }
    }

    /// Resolves a selected candidate: its feed URL first, then provider
    /// lookups by cross-provider ids, then its website.
    pub async fn resolve_candidate(
        &self,
        candidate: &PodcastCandidate,
        cancel: CancellationToken,
    ) -> Result<ResolvedFeed, ResolveFailure> {
        let mut s = Session {
            started: Instant::now(),
            requests: 0,
            steps: Vec::new(),
            cancel,
            tried: Vec::new(),
        };
        let input = candidate.title.clone();
        s.step(
            StepKind::Classify,
            candidate.feed_url.as_ref(),
            true,
            format!(
                "candidate `{}` from {:?}",
                candidate.title,
                candidate.providers()
            ),
        );
        let mut last_err: Option<ResolveError> = None;
        if let Some(feed) = &candidate.feed_url {
            match self.validate_feed_url(&mut s, feed, &input).await {
                Ok(f) => return Ok(f),
                Err(e) => last_err = Some(e),
            }
        }
        if let Some(itunes) = candidate.itunes_id {
            for provider in [ProviderId::PODCAST_INDEX, ProviderId::APPLE] {
                if self
                    .registry
                    .as_ref()
                    .is_some_and(|r| r.enabled().contains(&provider))
                {
                    match self
                        .via_provider(
                            &mut s,
                            provider,
                            ProviderRef::ItunesId(itunes),
                            Some(itunes),
                            &input,
                        )
                        .await
                    {
                        Ok(f) => return Ok(f),
                        Err(e) => last_err = Some(e),
                    }
                }
            }
        }
        if let Some(site) = &candidate.website {
            match self.resolve_url(&mut s, site, &input).await {
                Ok(f) => return Ok(f),
                Err(e) => last_err = Some(e),
            }
        }
        let err = last_err.unwrap_or_else(|| ResolveError::NoFeedLinkFound {
            url: candidate
                .website
                .clone()
                .unwrap_or_else(|| Url::parse("about:blank").unwrap_or_else(|_| unreachable!())),
            tried: s.tried.clone(),
        });
        Err(s.fail(err))
    }

    async fn via_provider(
        &self,
        s: &mut Session,
        provider: ProviderId,
        reference: ProviderRef,
        itunes_id: Option<u64>,
        input: &str,
    ) -> Result<ResolvedFeed, ResolveError> {
        let Some(registry) = &self.registry else {
            return Err(ResolveError::ProviderUnavailable {
                provider: provider.to_string(),
                detail: "no provider registry configured".into(),
            });
        };
        let mut order = vec![provider];
        // Podcast Index can also answer Apple ids and vice versa.
        if let Some(id) = itunes_id {
            let _ = id;
            for alt in [ProviderId::PODCAST_INDEX, ProviderId::APPLE] {
                if !order.contains(&alt) {
                    order.push(alt);
                }
            }
        }
        let mut last = ResolveError::ProviderUnavailable {
            provider: provider.to_string(),
            detail: "not enabled".into(),
        };
        for p in order {
            if !registry.enabled().contains(&p) {
                s.step(
                    StepKind::ProviderLookup,
                    None,
                    false,
                    format!("{p} not enabled"),
                );
                continue;
            }
            let reference = match (&reference, itunes_id) {
                (ProviderRef::Id(_), Some(id)) if p != provider => ProviderRef::ItunesId(id),
                _ => reference.clone(),
            };
            let ctx = ProviderContext {
                cancel: s.cancel.clone(),
                deadline: Some(s.started + self.config.time_budget),
                limit: 1,
                country: None,
            };
            let call = registry.lookup(p, &reference, &ctx, true).await;
            s.requests += usize::from(!call.from_cache);
            match call.result {
                Ok(Some(c)) => {
                    s.step(
                        StepKind::ProviderLookup,
                        c.feed_url.as_ref(),
                        c.feed_url.is_some(),
                        format!("{p} knows `{}`", c.title),
                    );
                    if let Some(feed) = &c.feed_url {
                        return self.validate_feed_url(s, feed, input).await;
                    }
                    last = ResolveError::NoFeedLinkFound {
                        url: c.website.clone().unwrap_or_else(|| {
                            Url::parse("https://podcasts.apple.com/")
                                .unwrap_or_else(|_| unreachable!())
                        }),
                        tried: Vec::new(),
                    };
                }
                Ok(None) => {
                    s.step(
                        StepKind::ProviderLookup,
                        None,
                        false,
                        format!("{p}: not found"),
                    );
                    last = ResolveError::NoFeedLinkFound {
                        url: Url::parse(&format!("https://{p}.invalid/"))
                            .unwrap_or_else(|_| unreachable!()),
                        tried: Vec::new(),
                    };
                }
                Err(e) => {
                    s.step(StepKind::ProviderLookup, None, false, format!("{p}: {e}"));
                    last = match e {
                        ProviderError::Cancelled => ResolveError::Cancelled,
                        other => ResolveError::ProviderUnavailable {
                            provider: p.to_string(),
                            detail: other.to_string(),
                        },
                    };
                }
            }
        }
        Err(last)
    }

    async fn resolve_url(
        &self,
        s: &mut Session,
        url: &Url,
        input: &str,
    ) -> Result<ResolvedFeed, ResolveError> {
        match self.fetch(s, url).await? {
            Fetched::Feed(probe, final_url) => self.finish(s, *probe, final_url, input).await,
            Fetched::Html(body, page_url) => self.resolve_html(s, &page_url, &body, input).await,
        }
    }

    async fn validate_feed_url(
        &self,
        s: &mut Session,
        url: &Url,
        input: &str,
    ) -> Result<ResolvedFeed, ResolveError> {
        match self.fetch(s, url).await? {
            Fetched::Feed(probe, final_url) => self.finish(s, *probe, final_url, input).await,
            Fetched::Html(body, page_url) => {
                s.step(
                    StepKind::Sniff,
                    Some(&page_url),
                    false,
                    "feed url returned HTML; trying autodiscovery",
                );
                self.resolve_html(s, &page_url, &body, input).await
            }
        }
    }

    async fn resolve_html(
        &self,
        s: &mut Session,
        page_url: &Url,
        body: &str,
        input: &str,
    ) -> Result<ResolvedFeed, ResolveError> {
        let links = html::discover_feed_links(page_url, body);
        s.step(
            StepKind::Autodiscovery,
            Some(page_url),
            !links.is_empty(),
            format!("{} feed link(s) in HTML", links.len()),
        );
        let mut candidates: Vec<(StepKind, Url)> = links
            .into_iter()
            .map(|l| (StepKind::Autodiscovery, l.url))
            .collect();
        for u in patterns::platform_feed_candidates(page_url) {
            candidates.push((StepKind::PlatformPattern, u));
        }
        if self.config.try_well_known_paths && candidates.is_empty() {
            for u in patterns::well_known_candidates(page_url) {
                candidates.push((StepKind::WellKnownPath, u));
            }
        }
        let mut last_reason: Option<ResolveError> = None;
        for (kind, url) in candidates {
            if s.tried.contains(&url) {
                continue;
            }
            if s.requests >= self.config.max_requests
                || s.started.elapsed() > self.config.time_budget
            {
                return Err(ResolveError::BudgetExceeded {
                    requests: s.requests,
                });
            }
            s.tried.push(url.clone());
            match self.fetch(s, &url).await {
                Ok(Fetched::Feed(probe, final_url)) => {
                    if probe.looks_like_podcast() {
                        s.step(kind, Some(&url), true, "candidate is a podcast feed");
                        return self.finish(s, *probe, final_url, input).await;
                    }
                    s.step(kind, Some(&url), false, "feed without media items");
                    last_reason = Some(ResolveError::FeedInvalid {
                        url: url.clone(),
                        reason: "it has no items with media".into(),
                    });
                }
                Ok(Fetched::Html(..)) => {
                    s.step(kind, Some(&url), false, "candidate returned HTML");
                }
                Err(ResolveError::Cancelled) => return Err(ResolveError::Cancelled),
                Err(ResolveError::BlockedByPolicy { url, detail }) => {
                    s.step(kind, Some(&url), false, format!("blocked: {detail}"));
                    last_reason = Some(ResolveError::BlockedByPolicy { url, detail });
                }
                Err(e) => {
                    s.step(kind, Some(&url), false, e.to_string());
                }
            }
        }
        match last_reason {
            Some(e @ ResolveError::FeedInvalid { .. }) if s.tried.len() == 1 => Err(e),
            _ => Err(ResolveError::NoFeedLinkFound {
                url: page_url.clone(),
                tried: s.tried.clone(),
            }),
        }
    }

    /// Fetches a URL (counting against the budget) and sniffs the body.
    async fn fetch(&self, s: &mut Session, url: &Url) -> Result<Fetched, ResolveError> {
        if s.cancel.is_cancelled() {
            return Err(ResolveError::Cancelled);
        }
        if s.requests >= self.config.max_requests || s.started.elapsed() > self.config.time_budget {
            return Err(ResolveError::BudgetExceeded {
                requests: s.requests,
            });
        }
        s.requests += 1;
        let opts = GetOptions {
            cancel: Some(s.cancel.clone()),
            headers: vec![(
                HeaderName::from_static("accept"),
                HeaderValue::from_static(
                    "application/rss+xml, application/atom+xml, application/xml;q=0.9, text/xml;q=0.9, text/html;q=0.8, */*;q=0.1",
                ),
            )],
            ..GetOptions::default()
        };
        let resp: Response = match self.client.get_with(url, &opts).await {
            Ok(r) => r,
            Err(HttpError::Cancelled) => return Err(ResolveError::Cancelled),
            Err(HttpError::Policy(v)) => {
                s.step(
                    StepKind::Fetch,
                    Some(url),
                    false,
                    format!("blocked by policy: {v}"),
                );
                return Err(ResolveError::BlockedByPolicy {
                    url: url.clone(),
                    detail: v.to_string(),
                });
            }
            Err(e) => {
                s.step(StepKind::Fetch, Some(url), false, e.to_string());
                return Err(ResolveError::Network {
                    url: url.clone(),
                    error_kind: e.kind().to_owned(),
                    detail: e.to_string(),
                });
            }
        };
        let final_url = resp.url.clone();
        let detail = if resp.redirects.is_empty() {
            format!("http {}", resp.status.as_u16())
        } else {
            format!(
                "http {} after {} redirect(s) → {final_url}",
                resp.status.as_u16(),
                resp.redirects.len()
            )
        };
        s.step(StepKind::Fetch, Some(url), resp.status.is_success(), detail);
        if !resp.status.is_success() {
            return Err(ResolveError::HttpStatus {
                url: url.clone(),
                status: resp.status.as_u16(),
            });
        }
        match uguisu_feed::probe(&resp.body) {
            Ok(probe) => {
                s.step(
                    StepKind::Sniff,
                    Some(&final_url),
                    true,
                    format!(
                        "{:?} feed, {} items, {} with media",
                        probe.kind, probe.item_count, probe.items_with_enclosure
                    ),
                );
                Ok(Fetched::Feed(Box::new(probe), final_url))
            }
            Err(ProbeError::NotXml {
                looks_like_html: true,
            }) => {
                s.step(StepKind::Sniff, Some(&final_url), true, "HTML page");
                Ok(Fetched::Html(
                    String::from_utf8_lossy(&resp.body).into_owned(),
                    final_url,
                ))
            }
            Err(e) => {
                s.step(StepKind::Sniff, Some(&final_url), false, e.to_string());
                Err(ResolveError::NotAFeed {
                    url: final_url,
                    detail: e.to_string(),
                })
            }
        }
    }

    /// Validates a probed feed and assembles the result (canonical URL, HTTPS upgrade).
    #[allow(clippy::too_many_lines)] // validation, upgrade and canonicalization are one sequence
    async fn finish(
        &self,
        s: &mut Session,
        probe: FeedProbe,
        mut feed_url: Url,
        input: &str,
    ) -> Result<ResolvedFeed, ResolveError> {
        if !probe.looks_like_podcast() {
            s.step(
                StepKind::Validate,
                Some(&feed_url),
                false,
                "no items with media",
            );
            return Err(ResolveError::FeedInvalid {
                url: feed_url,
                reason: "it has no items with media".into(),
            });
        }
        s.step(
            StepKind::Validate,
            Some(&feed_url),
            true,
            format!("podcast feed `{}`", probe.title.clone().unwrap_or_default()),
        );
        let mut warnings = probe.warnings.clone();
        let mut canonical_url = None;

        if self.config.https_upgrade && feed_url.scheme() == "http" {
            let mut https = feed_url.clone();
            if https.set_scheme("https").is_ok() && s.requests < self.config.max_requests {
                match self.fetch(s, &https).await {
                    Ok(Fetched::Feed(p2, final_https))
                        if p2.looks_like_podcast() && p2.title == probe.title =>
                    {
                        s.step(
                            StepKind::HttpsUpgrade,
                            Some(&final_https),
                            true,
                            "https variant serves the same feed",
                        );
                        feed_url = final_https;
                    }
                    Ok(_) => s.step(
                        StepKind::HttpsUpgrade,
                        Some(&https),
                        false,
                        "https variant differs; keeping http",
                    ),
                    Err(ResolveError::Cancelled) => return Err(ResolveError::Cancelled),
                    Err(e) => s.step(StepKind::HttpsUpgrade, Some(&https), false, e.to_string()),
                }
            }
        }

        if let Some(self_link) = &probe.self_link
            && self_link != &feed_url
        {
            let same_host = self_link.host_str() == feed_url.host_str();
            if !same_host {
                warnings.push(format!(
                    "self link {self_link} points to another host; not used as canonical"
                ));
                s.step(
                    StepKind::Canonicalize,
                    Some(self_link),
                    false,
                    "self link on another host",
                );
            } else if self.config.confirm_self_link && s.requests < self.config.max_requests {
                match self.fetch(s, self_link).await {
                    Ok(Fetched::Feed(p2, _))
                        if p2.looks_like_podcast() && p2.title == probe.title =>
                    {
                        s.step(
                            StepKind::Canonicalize,
                            Some(self_link),
                            true,
                            "self link confirmed",
                        );
                        canonical_url = Some(self_link.clone());
                    }
                    Ok(_) => s.step(
                        StepKind::Canonicalize,
                        Some(self_link),
                        false,
                        "self link serves different content",
                    ),
                    Err(ResolveError::Cancelled) => return Err(ResolveError::Cancelled),
                    Err(e) => s.step(
                        StepKind::Canonicalize,
                        Some(self_link),
                        false,
                        e.to_string(),
                    ),
                }
            } else {
                canonical_url = Some(self_link.clone());
                s.step(
                    StepKind::Canonicalize,
                    Some(self_link),
                    true,
                    "self link recorded without confirmation",
                );
            }
        }
        // `itunes:new-feed-url` frequently repeats the feed's own URL (seen live
        // on Megaphone, Simplecast, Transistor and TWiT feeds); only a different
        // target is a move.
        let moved_to = probe
            .new_feed_url
            .clone()
            .filter(|moved| moved != &feed_url);
        if let Some(moved) = &moved_to {
            warnings.push(format!("feed announces a new location: {moved}"));
        }

        Ok(ResolvedFeed {
            input: input.to_owned(),
            feed_url,
            canonical_url,
            moved_to,
            website: probe.link.clone(),
            title: probe.title.clone(),
            author: probe.author.clone(),
            description: probe.description.clone(),
            artwork: probe.image.clone(),
            podcast_guid: probe.podcast_guid.clone(),
            language: probe.language.clone(),
            item_count: probe.item_count,
            items_with_media: probe.items_with_enclosure,
            newest_item: probe.newest_item,
            locked: probe.locked,
            feed_kind: probe.kind.unwrap_or(FeedKind::Rss2),
            provenance: std::mem::take(&mut s.steps),
            warnings,
            verified_at: OffsetDateTime::now_utc(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use uguisu_http::{NetworkPolicy, Profile};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const FEED: &str =
        include_str!("../../../../tests/fixtures/feeds/probe/rss_itunes_podcast.xml");
    const BLOG: &str = include_str!("../../../../tests/fixtures/feeds/probe/rss_no_enclosures.xml");

    fn resolver(server: &MockServer) -> Resolver {
        let _ = server;
        let client = HttpClient::with_policy(
            Profile::Feed,
            NetworkPolicy::strict().allow_private_hosts(["127.0.0.1"]),
        )
        .unwrap();
        Resolver::new(
            client,
            None,
            ResolverConfig {
                https_upgrade: false,
                ..ResolverConfig::default()
            },
        )
    }

    async fn mount(server: &MockServer, p: &str, status: u16, ct: &str, body: &str) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("content-type", ct)
                    .set_body_string(body),
            )
            .mount(server)
            .await;
    }

    fn page(name: &str, base: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/websites/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("__BASE__", base)
    }

    fn feed_with_self(base: &str) -> String {
        FEED.replace(
            "https://feeds.example-diaries.test/rss",
            &format!("{base}/canonical.xml"),
        )
        .replace(
            "https://feeds.example-diaries.test/v2/rss",
            &format!("{base}/v2.xml"),
        )
    }

    #[tokio::test]
    async fn a_direct_feed_resolves_with_provenance() {
        let server = MockServer::start().await;
        let base = server.uri();
        mount(
            &server,
            "/feed.xml",
            200,
            "application/rss+xml",
            &feed_with_self(&base),
        )
        .await;
        mount(
            &server,
            "/canonical.xml",
            200,
            "application/rss+xml",
            &feed_with_self(&base),
        )
        .await;
        let r = resolver(&server)
            .resolve(&format!("{base}/feed.xml"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.title.as_deref(), Some("Example Diaries"));
        assert_eq!(r.feed_url.path(), "/feed.xml");
        assert_eq!(
            r.canonical_url.as_ref().map(Url::path),
            Some("/canonical.xml")
        );
        assert_eq!(r.moved_to.as_ref().map(Url::path), Some("/v2.xml"));
        assert_eq!((r.item_count, r.items_with_media), (3, 3));
        assert_eq!(
            r.podcast_guid.as_deref(),
            Some("917393e3-1b1e-5cef-ace4-edaa54e1f810")
        );
        assert!(
            r.provenance
                .iter()
                .any(|s| s.kind == StepKind::Canonicalize && s.ok)
        );
        assert!(r.warnings.iter().any(|w| w.contains("new location")));
    }

    #[tokio::test]
    async fn no_media_is_invalid() {
        let server = MockServer::start().await;
        let base = server.uri();
        mount(&server, "/blog.xml", 200, "application/rss+xml", BLOG).await;
        let err = resolver(&server)
            .resolve(&format!("{base}/blog.xml"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, ResolveError::FeedInvalid { .. }),
            "{err:?}"
        );
        assert!(!err.provenance.is_empty());

        mount(
            &server,
            "/page",
            200,
            "text/html",
            &page("no_links.html", &base),
        )
        .await;
        let err = resolver(&server)
            .resolve(&format!("{base}/page"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, ResolveError::NoFeedLinkFound { ref tried, .. } if tried.len() == patterns::WELL_KNOWN_PATHS.len()),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn website_autodiscovery_absolute_relative_and_priority() {
        let server = MockServer::start().await;
        let base = server.uri();
        mount(
            &server,
            "/abs",
            200,
            "text/html",
            &page("link_absolute.html", &base),
        )
        .await;
        mount(&server, "/feeds/show.xml", 200, "application/rss+xml", FEED).await;
        let r = resolver(&server)
            .resolve(&format!("{base}/abs"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.feed_url.path(), "/feeds/show.xml");
        assert!(
            r.provenance
                .iter()
                .any(|s| s.kind == StepKind::Autodiscovery && s.ok)
        );

        mount(
            &server,
            "/rel",
            200,
            "text/html",
            &page("link_relative_base.html", &base),
        )
        .await;
        mount(
            &server,
            "/site/rss/podcast.xml",
            200,
            "application/rss+xml",
            FEED,
        )
        .await;
        let r = resolver(&server)
            .resolve(&format!("{base}/rel"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.feed_url.path(), "/site/rss/podcast.xml");

        mount(
            &server,
            "/multi",
            200,
            "text/html",
            &page("link_multiple.html", &base),
        )
        .await;
        mount(&server, "/podcast.rss", 200, "application/rss+xml", FEED).await;
        mount(&server, "/comments/feed", 200, "application/rss+xml", BLOG).await;
        let r = resolver(&server)
            .resolve(&format!("{base}/multi"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.feed_url.path(), "/podcast.rss");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|req| req.url.path() != "/comments/feed"),
            "podcast-titled feed is tried first"
        );
    }

    #[tokio::test]
    async fn well_known_paths_and_budget() {
        let server = MockServer::start().await;
        let base = server.uri();
        mount(
            &server,
            "/nolinks",
            200,
            "text/html",
            &page("no_links.html", &base),
        )
        .await;
        mount(&server, "/feed.xml", 200, "application/rss+xml", FEED).await;
        let r = resolver(&server)
            .resolve(&format!("{base}/nolinks"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.feed_url.path(), "/feed.xml");
        assert!(
            r.provenance
                .iter()
                .any(|s| s.kind == StepKind::WellKnownPath && s.ok)
        );

        mount(
            &server,
            "/allhtml",
            200,
            "text/html",
            &page("links_all_html.html", &base),
        )
        .await;
        for i in 1..=9 {
            mount(
                &server,
                &format!("/h{i}"),
                200,
                "text/html",
                "<html><body>not a feed</body></html>",
            )
            .await;
        }
        let err = resolver(&server)
            .resolve(&format!("{base}/allhtml"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, ResolveError::BudgetExceeded { requests: 8 }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn redirects_statuses_policy_and_classification_errors() {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/moved"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/feed.xml"))
            .mount(&server)
            .await;
        mount(&server, "/feed.xml", 200, "application/rss+xml", FEED).await;
        let r = resolver(&server)
            .resolve(&format!("{base}/moved"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            r.feed_url.path(),
            "/feed.xml",
            "final url after redirect is the feed url"
        );

        mount(&server, "/missing", 404, "text/plain", "nope").await;
        let err = resolver(&server)
            .resolve(&format!("{base}/missing"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(
            err.error,
            ResolveError::HttpStatus { status: 404, .. }
        ));

        Mock::given(method("GET"))
            .and(path("/private"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://10.0.0.1/feed"),
            )
            .mount(&server)
            .await;
        let err = resolver(&server)
            .resolve(&format!("{base}/private"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, ResolveError::BlockedByPolicy { .. }),
            "{err:?}"
        );

        mount(
            &server,
            "/binary",
            200,
            "application/octet-stream",
            "\u{0}\u{1}\u{2}",
        )
        .await;
        let err = resolver(&server)
            .resolve(&format!("{base}/binary"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err.error, ResolveError::NotAFeed { .. }),
            "{err:?}"
        );

        let res = resolver(&server);
        assert!(matches!(
            res.resolve("darknet diaries", CancellationToken::new())
                .await
                .unwrap_err()
                .error,
            ResolveError::NotAUrl { .. }
        ));
        assert!(matches!(
            res.resolve(
                "https://open.spotify.com/show/abc",
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .error,
            ResolveError::NoFeedAvailable { .. }
        ));
        assert!(matches!(
            res.resolve("https://fyyd.de/podcast/x/1", CancellationToken::new())
                .await
                .unwrap_err()
                .error,
            ResolveError::ProviderUnavailable { .. }
        ));
        let apple = res
            .resolve(
                "https://podcasts.apple.com/us/podcast/x/id1",
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(apple.error, ResolveError::ProviderUnavailable { .. }),
            "no registry configured: {apple:?}"
        );
    }

    #[tokio::test]
    async fn candidate_resolution_prefers_feed_then_website() {
        let server = MockServer::start().await;
        let base = server.uri();
        mount(&server, "/feed.xml", 200, "application/rss+xml", FEED).await;
        mount(
            &server,
            "/site",
            200,
            "text/html",
            &page("link_absolute.html", &base),
        )
        .await;
        mount(&server, "/feeds/show.xml", 200, "application/rss+xml", FEED).await;
        let identity = crate::candidate::ProviderIdentity {
            provider: ProviderId::APPLE,
            provider_ref: "1".into(),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        };
        let mut c = PodcastCandidate::new("Example Diaries", identity);
        c.feed_url = Some(Url::parse(&format!("{base}/feed.xml")).unwrap());
        let r = resolver(&server)
            .resolve_candidate(&c, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.feed_url.path(), "/feed.xml");
        c.feed_url = None;
        c.website = Some(Url::parse(&format!("{base}/site")).unwrap());
        let r = resolver(&server)
            .resolve_candidate(&c, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.feed_url.path(), "/feeds/show.xml");
        c.website = None;
        assert!(
            resolver(&server)
                .resolve_candidate(&c, CancellationToken::new())
                .await
                .is_err()
        );
    }

    #[test]
    fn error_kinds_and_suggestions_are_stable() {
        let e = ResolveError::NotAUrl { input: "x".into() };
        assert_eq!(e.kind(), "not_a_url");
        assert!(!e.suggestion().is_empty());
        let json = serde_json::to_string(&ResolveError::NoFeedAvailable {
            platform: "Spotify".into(),
            hint: None,
        })
        .unwrap();
        assert!(json.contains("\"kind\":\"no_feed_available\""));
    }
}
