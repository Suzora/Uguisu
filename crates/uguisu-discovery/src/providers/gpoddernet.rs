//! gpodder.net directory.
//!
//! Source: `gpodder/mygpo` → `doc/api/reference/directory.rst`.
//! `GET /search.json?q=…&scale_logo=…` (no auth) returns
//! `url, title, description, subscribers, logo_url, scaled_logo_url,
//! website, mygpo_link`; `GET /api/2/data/podcast.json?url=…` looks a feed
//! up (404 when unknown). No rate limit is published; the service is
//! community-run, so Uguisu keeps one request per second.

use async_trait::async_trait;
use serde::Deserialize;
use uguisu_core::config::GpodderNetConfig;
use uguisu_core::provider::ProviderId;
use uguisu_http::{HttpClient, ThrottleConfig, Url};

use super::{
    build_url, cache_hint, check_status, get, now, opt_str, opt_url, parse_json,
    positional_confidence, truncate,
};
use crate::candidate::{PodcastCandidate, Popularity, ProviderIdentity};
use crate::provider::{
    Capabilities, DiscoveryProvider, ProviderContext, ProviderError, ProviderInfo, ProviderRef,
    ProviderResponse,
};
use crate::query::NormalizedQuery;

/// Documentation URL.
pub const DOCS_URL: &str =
    "https://gpoddernet.readthedocs.io/en/latest/api/reference/directory.html";
/// Attribution shown with results.
pub const ATTRIBUTION: &str = "Directory data from gpodder.net";
/// Subscriber count that maps to popularity 1.0 (log scale).
const POPULARITY_CEILING: f64 = 100_000.0;

/// The gpodder.net provider.
#[derive(Debug, Clone)]
pub struct GpodderNetProvider {
    client: HttpClient,
    base_url: Url,
}

#[derive(Debug, Deserialize)]
struct Podcast {
    url: Option<String>,
    title: Option<String>,
    author: Option<String>,
    description: Option<String>,
    subscribers: Option<u64>,
    logo_url: Option<String>,
    scaled_logo_url: Option<String>,
    website: Option<String>,
    mygpo_link: Option<String>,
}

impl GpodderNetProvider {
    /// Creates the provider.
    pub fn new(client: HttpClient, cfg: &GpodderNetConfig) -> Result<Self, ProviderError> {
        let base_url = Url::parse(&format!("{}/", cfg.base_url.trim_end_matches('/')))
            .map_err(|e| ProviderError::InvalidResponse(format!("bad base url: {e}")))?;
        Ok(Self { client, base_url })
    }

    /// Log-scaled subscriber count in 0..1.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // counts are far below 2^52; f32 is enough for a score
    pub fn normalize_subscribers(subscribers: u64) -> f32 {
        let v = ((1.0 + subscribers as f64).ln() / (1.0 + POPULARITY_CEILING).ln()).clamp(0.0, 1.0);
        v as f32
    }

    #[allow(clippy::cast_precision_loss)] // subscriber counts fit easily
    fn map(p: &Podcast, position: usize) -> Option<PodcastCandidate> {
        let feed_url = opt_url(p.url.as_deref())?;
        let title = opt_str(p.title.as_deref()).unwrap_or_else(|| feed_url.to_string());
        let identity = ProviderIdentity {
            provider: ProviderId::GPODDER_NET,
            provider_ref: feed_url.to_string(),
            confidence: positional_confidence(position),
            url: opt_url(p.mygpo_link.as_deref()),
            fetched_at: now(),
        };
        let mut c = PodcastCandidate::new(title, identity);
        c.author = opt_str(p.author.as_deref());
        c.description = opt_str(p.description.as_deref());
        c.artwork =
            opt_url(p.scaled_logo_url.as_deref()).or_else(|| opt_url(p.logo_url.as_deref()));
        c.website = opt_url(p.website.as_deref());
        c.feed_url = Some(feed_url);
        if let Some(subs) = p.subscribers {
            c.popularity.push(Popularity {
                provider: ProviderId::GPODDER_NET,
                raw: subs as f64,
                normalized: Self::normalize_subscribers(subs),
                label: "subscribers".to_owned(),
            });
        }
        c.attribute_all_to(ProviderId::GPODDER_NET);
        Some(c)
    }
}

#[async_trait]
impl DiscoveryProvider for GpodderNetProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::GPODDER_NET,
            name: "gpodder.net",
            attribution: Some(ATTRIBUTION),
            docs_url: DOCS_URL,
            capabilities: Capabilities {
                search: true,
                lookup_by_feed_url: true,
                popularity: true,
                ..Capabilities::default()
            },
            requires_credentials: false,
            throttle: ThrottleConfig::per_minute(60, 2),
            trust: 0.6,
        }
    }

    async fn search(
        &self,
        query: &NormalizedQuery,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        let url = build_url(
            &self.base_url,
            "search.json",
            &[("q", query.raw.as_str()), ("scale_logo", "256")],
        )?;
        let resp = get(&self.client, &url, ctx, Vec::new()).await?;
        check_status(&resp)?;
        let list: Vec<Podcast> = parse_json(&resp)?;
        let candidates = list
            .into_iter()
            .enumerate()
            .filter_map(|(i, p)| Self::map(&p, i))
            .collect();
        Ok(ProviderResponse {
            value: truncate(candidates, ctx.limit),
            cache_max_age: cache_hint(&resp),
        })
    }

    async fn lookup(
        &self,
        reference: &ProviderRef,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Option<PodcastCandidate>>, ProviderError> {
        let feed = match reference {
            ProviderRef::FeedUrl(u) => u.to_string(),
            ProviderRef::Id(s) => s.clone(),
            other => {
                return Err(ProviderError::Unsupported(format!(
                    "gpodder.net lookup by {other:?}"
                )));
            }
        };
        let url = build_url(
            &self.base_url,
            "api/2/data/podcast.json",
            &[("url", feed.as_str())],
        )?;
        let resp = get(&self.client, &url, ctx, Vec::new()).await?;
        if resp.status.as_u16() == 404 {
            return Ok(ProviderResponse {
                value: None,
                cache_max_age: cache_hint(&resp),
            });
        }
        check_status(&resp)?;
        let p: Podcast = parse_json(&resp)?;
        Ok(ProviderResponse {
            value: Self::map(&p, 0),
            cache_max_age: cache_hint(&resp),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use uguisu_http::{NetworkPolicy, Profile};
    use wiremock::MockServer;

    use super::*;
    use crate::testing::Fixture;

    fn provider(server: &MockServer) -> GpodderNetProvider {
        let client = HttpClient::with_policy(Profile::Discovery, NetworkPolicy::trusted()).unwrap();
        GpodderNetProvider::new(
            client,
            &GpodderNetConfig {
                enabled: true,
                base_url: server.uri(),
            },
        )
        .unwrap()
    }

    async fn search(
        case: &str,
        term: &str,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        let server = MockServer::start().await;
        Fixture::load("gpoddernet", case)
            .unwrap()
            .mount(&server)
            .await;
        provider(&server)
            .search(&NormalizedQuery::parse(term), &ProviderContext::default())
            .await
    }

    #[tokio::test]
    async fn maps_search_results_with_popularity() {
        let c = search("search_floss", "floss weekly").await.unwrap().value;
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].title, "FLOSS Weekly");
        assert_eq!(c[0].author.as_deref(), Some("Leo Laporte"));
        assert_eq!(
            c[0].feed_url.as_ref().map(Url::as_str),
            Some("http://leo.am/podcasts/floss")
        );
        assert_eq!(
            c[0].website.as_ref().map(Url::as_str),
            Some("http://twit.tv/")
        );
        assert_eq!(
            c[0].identities[0].url.as_ref().map(Url::as_str),
            Some("http://gpodder.net/podcast/12925")
        );
        assert_eq!(c[0].popularity[0].raw, 1138.0);
        assert!(c[0].popularity[0].normalized > c[1].popularity[0].normalized);
        assert_eq!(
            c[0].provenance.get("feed_url"),
            Some(&ProviderId::GPODDER_NET)
        );
    }

    #[test]
    fn subscriber_normalization_is_monotonic_and_bounded() {
        let a = GpodderNetProvider::normalize_subscribers(0);
        let b = GpodderNetProvider::normalize_subscribers(100);
        let c = GpodderNetProvider::normalize_subscribers(100_000);
        let d = GpodderNetProvider::normalize_subscribers(10_000_000);
        assert_eq!(a, 0.0);
        assert!(a < b && b < c);
        assert!((c - 1.0).abs() < 1e-6 && d <= 1.0);
    }

    #[tokio::test]
    async fn empty_malformed_and_lookup() {
        assert!(
            search("search_empty", "zzqqxxnothing")
                .await
                .unwrap()
                .value
                .is_empty()
        );
        assert!(matches!(
            search("malformed", "malformed").await.unwrap_err(),
            ProviderError::InvalidResponse(_)
        ));
        let server = MockServer::start().await;
        Fixture::load("gpoddernet", "podcast_by_url")
            .unwrap()
            .mount(&server)
            .await;
        Fixture::load("gpoddernet", "podcast_by_url_404")
            .unwrap()
            .mount(&server)
            .await;
        let p = provider(&server);
        let ctx = ProviderContext::default();
        let found = p
            .lookup(
                &ProviderRef::FeedUrl(
                    Url::parse("http://feeds.feedburner.com/coverville").unwrap(),
                ),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(found.value.unwrap().title, "Coverville");
        let missing = p
            .lookup(
                &ProviderRef::FeedUrl(Url::parse("http://nowhere.test/feed").unwrap()),
                &ctx,
            )
            .await
            .unwrap();
        assert!(missing.value.is_none());
        assert!(matches!(
            p.lookup(&ProviderRef::ItunesId(1), &ctx).await.unwrap_err(),
            ProviderError::Unsupported(_)
        ));
    }
}
