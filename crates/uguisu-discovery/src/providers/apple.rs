//! Apple Podcasts via the iTunes Search API.
//!
//! Source: developer.apple.com, *iTunes Search API* (Constructing Searches,
//! Understanding Search Results). No key; "approximately 20 calls per
//! minute" per IP; `lang` accepts only `en_us` and `ja_jp`; `feedUrl` is
//! present for most podcasts but not for Apple-hosted/subscription shows.
//!
//! Live-verified 2026-09-17 (`tests/fixtures/discovery/live/apple`): bodies
//! arrive as `text/javascript`, `Cache-Control: max-age=86400` on searches,
//! `genres` always includes "Podcasts", `country` is a storefront code
//! ("USA"), the search is not fuzzy ("darknet diariez" → 0 results), and a
//! burst of 45 requests in 20 s was not throttled — the 20/min throttle is
//! kept as documented courtesy.

use async_trait::async_trait;
use serde::Deserialize;
use uguisu_core::config::AppleConfig;
use uguisu_core::provider::ProviderId;
use uguisu_http::{HttpClient, ThrottleConfig, Url};

use super::{
    build_url, cache_hint, check_status, get, now, opt_str, opt_url, parse_json,
    positional_confidence, truncate,
};
use crate::candidate::{PodcastCandidate, ProviderIdentity};
use crate::provider::{
    Capabilities, DiscoveryProvider, ProviderContext, ProviderError, ProviderInfo, ProviderRef,
    ProviderResponse,
};
use crate::query::NormalizedQuery;

/// Documentation URL.
pub const DOCS_URL: &str = "https://developer.apple.com/library/archive/documentation/AudioVideo/Conceptual/iTuneSearchAPI/";

/// The Apple provider.
#[derive(Debug, Clone)]
pub struct AppleProvider {
    client: HttpClient,
    base_url: Url,
    country: String,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    #[serde(default)]
    result_count: u64,
    #[serde(default)]
    results: Vec<AppleResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppleResult {
    wrapper_type: Option<String>,
    kind: Option<String>,
    collection_id: Option<u64>,
    track_id: Option<u64>,
    artist_name: Option<String>,
    collection_name: Option<String>,
    track_name: Option<String>,
    collection_view_url: Option<String>,
    feed_url: Option<String>,
    artwork_url600: Option<String>,
    artwork_url100: Option<String>,
    release_date: Option<String>,
    collection_explicitness: Option<String>,
    track_count: Option<u32>,
    #[serde(default)]
    genres: Vec<String>,
}

impl AppleProvider {
    /// Creates the provider from configuration.
    pub fn new(client: HttpClient, cfg: &AppleConfig) -> Result<Self, ProviderError> {
        let base_url = Url::parse(&format!("{}/", cfg.base_url.trim_end_matches('/')))
            .map_err(|e| ProviderError::InvalidResponse(format!("bad base url: {e}")))?;
        Ok(Self {
            client,
            base_url,
            country: cfg.country.clone(),
            lang: cfg.lang.clone(),
        })
    }

    fn map(r: AppleResult, position: usize) -> Option<PodcastCandidate> {
        let is_podcast = r.kind.as_deref() == Some("podcast")
            || (r.kind.is_none()
                && r.wrapper_type.as_deref() == Some("track")
                && r.feed_url.is_some());
        if !is_podcast {
            return None;
        }
        let id = r.collection_id.or(r.track_id)?;
        let title =
            opt_str(r.collection_name.as_deref()).or_else(|| opt_str(r.track_name.as_deref()))?;
        let identity = ProviderIdentity {
            provider: ProviderId::APPLE,
            provider_ref: id.to_string(),
            confidence: positional_confidence(position),
            url: opt_url(r.collection_view_url.as_deref()),
            fetched_at: now(),
        };
        let mut c = PodcastCandidate::new(title, identity);
        c.author = opt_str(r.artist_name.as_deref());
        c.feed_url = opt_url(r.feed_url.as_deref());
        c.artwork =
            opt_url(r.artwork_url600.as_deref()).or_else(|| opt_url(r.artwork_url100.as_deref()));
        c.categories = r.genres.into_iter().filter(|g| g != "Podcasts").collect();
        c.episode_count = r.track_count;
        c.last_published = r.release_date.as_deref().and_then(uguisu_feed::parse_date);
        c.explicit = r
            .collection_explicitness
            .as_deref()
            .map(|e| e.eq_ignore_ascii_case("explicit"));
        c.itunes_id = Some(id);
        // `country` is the storefront (ISO 3166-1 alpha-3, e.g. "USA"), not a
        // language; Apple exposes no language field, so `language` stays unset.
        c.attribute_all_to(ProviderId::APPLE);
        Some(c)
    }

    async fn fetch(
        &self,
        url: Url,
        ctx: &ProviderContext,
    ) -> Result<(Vec<PodcastCandidate>, Option<std::time::Duration>), ProviderError> {
        let resp = get(&self.client, &url, ctx, Vec::new()).await?;
        if resp.status.as_u16() == 403 {
            // Apple answers 403 when the per-minute quota is exceeded.
            return Err(ProviderError::RateLimited {
                retry_after: uguisu_http::retry_after(&resp.headers),
            });
        }
        check_status(&resp)?;
        let env: Envelope = parse_json(&resp)?;
        tracing::debug!(
            provider = "apple",
            result_count = env.result_count,
            "apple response parsed"
        );
        let candidates = env
            .results
            .into_iter()
            .enumerate()
            .filter_map(|(i, r)| Self::map(r, i))
            .collect();
        Ok((candidates, cache_hint(&resp)))
    }

    fn country<'a>(&'a self, ctx: &'a ProviderContext) -> &'a str {
        ctx.country.as_deref().unwrap_or(&self.country)
    }
}

#[async_trait]
impl DiscoveryProvider for AppleProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::APPLE,
            name: "Apple Podcasts",
            attribution: Some("Search results from Apple Podcasts (iTunes Search API)"),
            docs_url: DOCS_URL,
            capabilities: Capabilities {
                search: true,
                lookup_by_id: true,
                lookup_by_itunes_id: true,
                ..Capabilities::default()
            },
            requires_credentials: false,
            throttle: ThrottleConfig::per_minute(20, 2),
            trust: 0.8,
        }
    }

    async fn search(
        &self,
        query: &NormalizedQuery,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        let limit = ctx.limit.clamp(1, 200).to_string();
        let mut params = vec![
            ("term", query.raw.as_str()),
            ("country", self.country(ctx)),
            ("media", "podcast"),
            ("entity", "podcast"),
            ("limit", limit.as_str()),
        ];
        if let Some(lang) = &self.lang {
            params.push(("lang", lang.as_str()));
        }
        let url = build_url(&self.base_url, "search", &params)?;
        let (candidates, cache_max_age) = self.fetch(url, ctx).await?;
        Ok(ProviderResponse {
            value: truncate(candidates, ctx.limit),
            cache_max_age,
        })
    }

    async fn lookup(
        &self,
        reference: &ProviderRef,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Option<PodcastCandidate>>, ProviderError> {
        let id = match reference {
            ProviderRef::Id(s) => s.clone(),
            ProviderRef::ItunesId(n) => n.to_string(),
            other => {
                return Err(ProviderError::Unsupported(format!(
                    "apple lookup by {other:?}"
                )));
            }
        };
        let url = build_url(
            &self.base_url,
            "lookup",
            &[
                ("id", id.as_str()),
                ("entity", "podcast"),
                ("country", self.country(ctx)),
            ],
        )?;
        let (mut candidates, cache_max_age) = self.fetch(url, ctx).await?;
        let first = if candidates.is_empty() {
            None
        } else {
            Some(candidates.remove(0))
        };
        Ok(ProviderResponse {
            value: first,
            cache_max_age,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use uguisu_http::{NetworkPolicy, Profile};
    use wiremock::MockServer;

    use super::*;
    use crate::testing::Fixture;

    fn provider(server: &MockServer) -> AppleProvider {
        let client = HttpClient::with_policy(Profile::Discovery, NetworkPolicy::trusted()).unwrap();
        let cfg = AppleConfig {
            enabled: true,
            country: "US".into(),
            lang: None,
            base_url: server.uri(),
        };
        AppleProvider::new(client, &cfg).unwrap()
    }

    async fn search(
        case: &str,
        term: &str,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        let server = MockServer::start().await;
        Fixture::load("apple", case).unwrap().mount(&server).await;
        provider(&server)
            .search(&NormalizedQuery::parse(term), &ProviderContext::default())
            .await
    }

    #[tokio::test]
    async fn maps_search_results() {
        let resp = search("search_darknet", "darknet diaries").await.unwrap();
        let c = &resp.value;
        assert_eq!(c.len(), 3);
        let top = &c[0];
        assert_eq!(top.title, "Darknet Diaries");
        assert_eq!(top.author.as_deref(), Some("Jack Rhysider"));
        assert_eq!(
            top.feed_url.as_ref().map(Url::as_str),
            Some("https://feeds.megaphone.fm/darknetdiaries")
        );
        assert!(top.artwork.as_ref().unwrap().as_str().contains("600x600"));
        assert_eq!(
            top.categories,
            vec!["Technology", "True Crime"],
            "'Podcasts' genre dropped"
        );
        assert_eq!(top.episode_count, Some(180));
        assert_eq!(top.itunes_id, Some(1_296_350_485));
        assert_eq!(top.explicit, Some(false));
        assert!(top.last_published.is_some());
        assert_eq!(top.identities[0].provider, ProviderId::APPLE);
        assert_eq!(top.identities[0].provider_ref, "1296350485");
        assert!(top.identities[0].confidence > c[1].identities[0].confidence);
        assert_eq!(top.provenance.get("feed_url"), Some(&ProviderId::APPLE));
        assert!(!top.provenance.contains_key("podcast_guid"));
    }

    #[tokio::test]
    async fn empty_and_missing_feed_url() {
        assert!(
            search("search_empty", "zzqqxxnothing")
                .await
                .unwrap()
                .value
                .is_empty()
        );
        let resp = search("search_missing_feedurl", "apple exclusive")
            .await
            .unwrap();
        assert_eq!(resp.value.len(), 1);
        assert!(!resp.value[0].has_feed());
        assert_eq!(resp.value[0].title, "Apple Exclusive Show");
    }

    #[tokio::test]
    async fn rate_limit_403_and_malformed() {
        assert!(matches!(
            search("error_403", "ratelimited").await.unwrap_err(),
            ProviderError::RateLimited { .. }
        ));
        assert!(matches!(
            search("malformed", "malformed").await.unwrap_err(),
            ProviderError::InvalidResponse(_)
        ));
    }

    #[tokio::test]
    async fn lookup_by_id_and_unsupported_refs() {
        let server = MockServer::start().await;
        Fixture::load("apple", "lookup_id")
            .unwrap()
            .mount(&server)
            .await;
        let p = provider(&server);
        let ctx = ProviderContext::default();
        let found = p
            .lookup(&ProviderRef::ItunesId(1_296_350_485), &ctx)
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(found.title, "Darknet Diaries");
        let err = p
            .lookup(&ProviderRef::PodcastGuid("x".into()), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported(_)));
    }

    #[tokio::test]
    async fn sends_documented_parameters() {
        let server = MockServer::start().await;
        Fixture::load("apple", "search_empty")
            .unwrap()
            .mount(&server)
            .await;
        let client = HttpClient::with_policy(Profile::Discovery, NetworkPolicy::trusted()).unwrap();
        let cfg = AppleConfig {
            enabled: true,
            country: "DE".into(),
            lang: Some("en_us".into()),
            base_url: server.uri(),
        };
        let p = AppleProvider::new(client, &cfg).unwrap();
        let ctx = ProviderContext {
            limit: 7,
            ..ProviderContext::default()
        };
        p.search(&NormalizedQuery::parse("zzqqxxnothing"), &ctx)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let q = req.url.query().unwrap();
        assert!(
            q.contains("country=DE")
                && q.contains("limit=7")
                && q.contains("lang=en_us")
                && q.contains("media=podcast"),
            "{q}"
        );
    }
}
