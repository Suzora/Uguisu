//! Podcast Index (podcastindex.org).
//!
//! Source: `Podcastindex-org/docs-api` OpenAPI spec and the Terms of
//! Service. Auth: `X-Auth-Key`, `X-Auth-Date` (unix seconds), and
//! `Authorization` = lowercase hex SHA-1 of `key + secret + date`; a
//! `User-Agent` is mandatory (sent by `uguisu-http`). Rate limits are
//! enforced at Podcast Index's discretion and not published — Uguisu stays
//! at one request per second. Cached copies may not outlive the
//! `Cache-Control` header (ToS §5) and attribution is required (ToS §7).

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use uguisu_core::config::PodcastIndexConfig;
use uguisu_core::provider::ProviderId;
use uguisu_core::secret::Secret;
use uguisu_http::{HeaderName, HeaderValue, HttpClient, ThrottleConfig, Url};

use super::{
    build_url, cache_hint, check_status, get, now, opt_str, opt_url, parse_json,
    positional_confidence, truncate, unix_ts,
};
use crate::candidate::{FeedHealthHints, PodcastCandidate, ProviderIdentity};
use crate::provider::{
    Capabilities, DiscoveryProvider, ProviderContext, ProviderError, ProviderInfo, ProviderRef,
    ProviderResponse,
};
use crate::query::NormalizedQuery;

/// Documentation URL.
pub const DOCS_URL: &str = "https://podcastindex-org.github.io/docs-api/";
/// Attribution required by the Terms of Service.
pub const ATTRIBUTION: &str = "Podcast data from Podcast Index (podcastindex.org)";

/// The Podcast Index provider.
#[derive(Debug, Clone)]
pub struct PodcastIndexProvider {
    client: HttpClient,
    base_url: Url,
    key: Option<Secret<String>>,
    secret: Option<Secret<String>>,
}

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    #[serde(default)]
    status: serde_json::Value,
    #[serde(default)]
    feeds: Vec<Feed>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LookupEnvelope {
    #[serde(default)]
    status: serde_json::Value,
    #[serde(default)]
    feed: serde_json::Value,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feed {
    id: Option<u64>,
    podcast_guid: Option<String>,
    title: Option<String>,
    url: Option<String>,
    original_url: Option<String>,
    link: Option<String>,
    description: Option<String>,
    author: Option<String>,
    owner_name: Option<String>,
    image: Option<String>,
    artwork: Option<String>,
    last_update_time: Option<i64>,
    last_http_status: Option<u16>,
    itunes_id: Option<u64>,
    language: Option<String>,
    explicit: Option<bool>,
    dead: Option<u8>,
    episode_count: Option<u32>,
    #[serde(default)]
    categories: serde_json::Value,
    locked: Option<u8>,
    newest_item_pubdate: Option<i64>,
}

impl PodcastIndexProvider {
    /// Creates the provider. Credentials may be absent; calls then fail with
    /// [`ProviderError::AuthRequired`] and the registry keeps it disabled.
    pub fn new(client: HttpClient, cfg: &PodcastIndexConfig) -> Result<Self, ProviderError> {
        let base_url = Url::parse(&format!("{}/", cfg.base_url.trim_end_matches('/')))
            .map_err(|e| ProviderError::InvalidResponse(format!("bad base url: {e}")))?;
        Ok(Self {
            client,
            base_url,
            key: cfg.key.clone(),
            secret: cfg.secret.clone(),
        })
    }

    /// Whether credentials are configured.
    pub fn has_credentials(&self) -> bool {
        self.key.is_some() && self.secret.is_some()
    }

    /// Computes the `Authorization` value for a key, secret and unix time.
    pub fn auth_hash(key: &str, secret: &str, unix_time: i64) -> String {
        let mut hasher = Sha1::new();
        hasher.update(key.as_bytes());
        hasher.update(secret.as_bytes());
        hasher.update(unix_time.to_string().as_bytes());
        hex::encode(hasher.finalize())
    }

    fn auth_headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, ProviderError> {
        let (Some(key), Some(secret)) = (&self.key, &self.secret) else {
            return Err(ProviderError::AuthRequired);
        };
        let unix_time = now().unix_timestamp();
        let hash = Self::auth_hash(key.expose(), secret.expose(), unix_time);
        let hv = |s: &str| {
            HeaderValue::from_str(s)
                .map_err(|e| ProviderError::InvalidResponse(format!("bad header: {e}")))
        };
        Ok(vec![
            (HeaderName::from_static("x-auth-key"), hv(key.expose())?),
            (
                HeaderName::from_static("x-auth-date"),
                hv(&unix_time.to_string())?,
            ),
            (HeaderName::from_static("authorization"), hv(&hash)?),
        ])
    }

    fn map(feed: Feed, position: usize) -> Option<PodcastCandidate> {
        let id = feed.id?;
        let title = opt_str(feed.title.as_deref())?;
        let identity = ProviderIdentity {
            provider: ProviderId::PODCAST_INDEX,
            provider_ref: id.to_string(),
            confidence: positional_confidence(position),
            url: Url::parse(&format!("https://podcastindex.org/podcast/{id}")).ok(),
            fetched_at: now(),
        };
        let mut c = PodcastCandidate::new(title, identity);
        c.author = opt_str(feed.author.as_deref());
        c.publisher = opt_str(feed.owner_name.as_deref());
        c.description = opt_str(feed.description.as_deref());
        c.artwork = opt_url(feed.artwork.as_deref()).or_else(|| opt_url(feed.image.as_deref()));
        c.language = opt_str(feed.language.as_deref());
        c.website = opt_url(feed.link.as_deref());
        c.feed_url = opt_url(feed.url.as_deref()).or_else(|| opt_url(feed.original_url.as_deref()));
        c.episode_count = feed.episode_count;
        c.last_published = unix_ts(feed.newest_item_pubdate);
        c.explicit = feed.explicit;
        c.itunes_id = feed.itunes_id;
        c.podcast_guid = opt_str(feed.podcast_guid.as_deref());
        c.categories = match feed.categories {
            serde_json::Value::Object(map) => {
                let sorted: BTreeMap<u64, String> = map
                    .into_iter()
                    .filter_map(|(k, v)| Some((k.parse::<u64>().ok()?, v.as_str()?.to_owned())))
                    .collect();
                sorted.into_values().collect()
            }
            _ => Vec::new(),
        };
        c.health = FeedHealthHints {
            dead: feed.dead.map(|d| d != 0),
            locked: feed.locked.map(|l| l != 0),
            last_update: unix_ts(feed.last_update_time),
            http_status: feed.last_http_status,
        };
        c.attribute_all_to(ProviderId::PODCAST_INDEX);
        Some(c)
    }

    fn status_ok(status: &serde_json::Value) -> bool {
        match status {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(s) => s.eq_ignore_ascii_case("true"),
            _ => true,
        }
    }

    async fn request(
        &self,
        url: Url,
        ctx: &ProviderContext,
    ) -> Result<uguisu_http::Response, ProviderError> {
        let headers = self.auth_headers()?;
        let resp = get(&self.client, &url, ctx, headers).await?;
        check_status(&resp)?;
        Ok(resp)
    }
}

#[async_trait]
impl DiscoveryProvider for PodcastIndexProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ProviderId::PODCAST_INDEX,
            name: "Podcast Index",
            attribution: Some(ATTRIBUTION),
            docs_url: DOCS_URL,
            capabilities: Capabilities {
                search: true,
                lookup_by_id: true,
                lookup_by_feed_url: true,
                lookup_by_itunes_id: true,
                lookup_by_guid: true,
                popularity: false,
            },
            requires_credentials: true,
            throttle: ThrottleConfig::per_minute(60, 2),
            trust: 0.9,
        }
    }

    async fn search(
        &self,
        query: &NormalizedQuery,
        ctx: &ProviderContext,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        let max = ctx.limit.clamp(1, 100).to_string();
        let url = build_url(
            &self.base_url,
            "search/byterm",
            &[
                ("q", query.raw.as_str()),
                ("max", max.as_str()),
                ("similar", "true"),
            ],
        )?;
        let resp = self.request(url, ctx).await?;
        let env: SearchEnvelope = parse_json(&resp)?;
        if !Self::status_ok(&env.status) {
            return Err(ProviderError::InvalidResponse(
                env.description.unwrap_or_else(|| "status false".to_owned()),
            ));
        }
        let candidates = env
            .feeds
            .into_iter()
            .enumerate()
            .filter_map(|(i, f)| Self::map(f, i))
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
        let (path, key, value) = match reference {
            ProviderRef::Id(id) => ("podcasts/byfeedid", "id", id.clone()),
            ProviderRef::FeedUrl(u) => ("podcasts/byfeedurl", "url", u.to_string()),
            ProviderRef::ItunesId(n) => ("podcasts/byitunesid", "id", n.to_string()),
            ProviderRef::PodcastGuid(g) => ("podcasts/byguid", "guid", g.clone()),
        };
        let url = build_url(&self.base_url, path, &[(key, value.as_str())])?;
        let resp = self.request(url, ctx).await?;
        let env: LookupEnvelope = parse_json(&resp)?;
        if !Self::status_ok(&env.status) {
            return Err(ProviderError::InvalidResponse(
                env.description.unwrap_or_else(|| "status false".to_owned()),
            ));
        }
        let feed: Option<Feed> = match env.feed {
            serde_json::Value::Object(_) => serde_json::from_value(env.feed)
                .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?,
            _ => None, // Podcast Index returns an empty array when nothing matches.
        };
        Ok(ProviderResponse {
            value: feed.and_then(|f| Self::map(f, 0)),
            cache_max_age: cache_hint(&resp),
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

    fn provider(server: &MockServer, with_creds: bool) -> PodcastIndexProvider {
        let client = HttpClient::with_policy(Profile::Discovery, NetworkPolicy::trusted()).unwrap();
        let cfg = PodcastIndexConfig {
            enabled: with_creds,
            key: with_creds.then(|| Secret::new("KEY".to_owned())),
            secret: with_creds.then(|| Secret::new("SECRET".to_owned())),
            base_url: server.uri(),
        };
        PodcastIndexProvider::new(client, &cfg).unwrap()
    }

    async fn search(
        case: &str,
        term: &str,
    ) -> Result<ProviderResponse<Vec<PodcastCandidate>>, ProviderError> {
        let server = MockServer::start().await;
        Fixture::load("podcastindex", case)
            .unwrap()
            .mount(&server)
            .await;
        provider(&server, true)
            .search(&NormalizedQuery::parse(term), &ProviderContext::default())
            .await
    }

    #[test]
    fn auth_hash_matches_spec_example() {
        // Example key/secret/date from the Podcast Index OpenAPI description.
        let hash = PodcastIndexProvider::auth_hash(
            "UXKCGDSYGUUEVQJSYDZH",
            "yzJe2eE7XV-3eY576dyRZ6wXyAbndh6LUrCZ8KN|",
            1_613_713_388,
        );
        assert_eq!(hash.len(), 40);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(hash, GOLDEN);
    }

    const GOLDEN: &str = "73a1fffed61c1d30d858beb1fc48f355386449d2";

    #[tokio::test]
    async fn maps_search_results_and_cache_hint() {
        let resp = search("search_byterm", "batman university").await.unwrap();
        assert_eq!(
            resp.cache_max_age,
            Some(std::time::Duration::from_secs(300))
        );
        let c = &resp.value;
        assert_eq!(c.len(), 2);
        let top = &c[0];
        assert_eq!(top.title, "Batman University");
        assert_eq!(top.author.as_deref(), Some("Tony Sindelar"));
        assert_eq!(top.publisher.as_deref(), Some("The Incomparable"));
        assert_eq!(
            top.feed_url.as_ref().map(Url::as_str),
            Some("https://feeds.theincomparable.com/batmanuniversity")
        );
        assert_eq!(
            top.website.as_ref().map(Url::as_str),
            Some("https://www.theincomparable.com/batmanuniversity/")
        );
        assert_eq!(
            top.podcast_guid.as_deref(),
            Some("9b024349-ccf0-5f69-a609-6b82873eab3c")
        );
        assert_eq!(top.itunes_id, Some(1_441_923_632));
        assert_eq!(top.categories, vec!["Tv", "Film", "Reviews"]);
        assert_eq!(top.episode_count, Some(19));
        assert_eq!(top.language.as_deref(), Some("en-us"));
        assert_eq!(top.health.dead, Some(false));
        assert_eq!(top.health.locked, Some(false));
        assert!(top.health.last_update.is_some());
        assert_eq!(
            top.last_published.map(time::OffsetDateTime::unix_timestamp),
            Some(1_546_399_813)
        );
        assert_eq!(top.identities[0].provider_ref, "75075");
        assert_eq!(
            top.identities[0].url.as_ref().map(Url::as_str),
            Some("https://podcastindex.org/podcast/75075")
        );
        assert_eq!(c[1].itunes_id, None);
    }

    #[tokio::test]
    async fn empty_errors_and_malformed() {
        assert!(
            search("search_empty", "zzqqxxnothing")
                .await
                .unwrap()
                .value
                .is_empty()
        );
        assert!(matches!(
            search("error_401", "unauthorized").await.unwrap_err(),
            ProviderError::AuthRejected
        ));
        assert!(matches!(
            search("error_400", "badrequest").await.unwrap_err(),
            ProviderError::InvalidResponse(_)
        ));
        assert!(matches!(
            search("malformed", "malformed").await.unwrap_err(),
            ProviderError::InvalidResponse(_)
        ));
    }

    #[tokio::test]
    async fn missing_credentials_fail_before_any_request() {
        let server = MockServer::start().await;
        let p = provider(&server, false);
        assert!(!p.has_credentials());
        let err = p
            .search(&NormalizedQuery::parse("x"), &ProviderContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sends_auth_headers_and_similar_flag() {
        let server = MockServer::start().await;
        Fixture::load("podcastindex", "search_empty")
            .unwrap()
            .mount(&server)
            .await;
        provider(&server, true)
            .search(
                &NormalizedQuery::parse("zzqqxxnothing"),
                &ProviderContext {
                    limit: 5,
                    ..ProviderContext::default()
                },
            )
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        assert_eq!(req.headers.get("x-auth-key").unwrap(), "KEY");
        let date: i64 = req
            .headers
            .get("x-auth-date")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        let expected = PodcastIndexProvider::auth_hash("KEY", "SECRET", date);
        assert_eq!(req.headers.get("authorization").unwrap(), expected.as_str());
        assert!(
            req.headers
                .get("user-agent")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Uguisu/")
        );
        let q = req.url.query().unwrap();
        assert!(q.contains("similar=true") && q.contains("max=5"), "{q}");
    }

    #[tokio::test]
    async fn lookups_by_feed_url_and_itunes_id() {
        let server = MockServer::start().await;
        Fixture::load("podcastindex", "byfeedurl")
            .unwrap()
            .mount(&server)
            .await;
        Fixture::load("podcastindex", "byitunesid")
            .unwrap()
            .mount(&server)
            .await;
        let p = provider(&server, true);
        let ctx = ProviderContext::default();
        let by_url = p
            .lookup(
                &ProviderRef::FeedUrl(
                    Url::parse("https://feeds.theincomparable.com/batmanuniversity").unwrap(),
                ),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(by_url.value.unwrap().title, "Batman University");
        let by_id = p
            .lookup(&ProviderRef::ItunesId(1_441_923_632), &ctx)
            .await
            .unwrap();
        assert_eq!(
            by_id.value.unwrap().podcast_guid.as_deref(),
            Some("9b024349-ccf0-5f69-a609-6b82873eab3c")
        );
    }
}
