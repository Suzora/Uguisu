//! Podcast search across directory providers.
//!
//! A search only ever produces feed URLs; the feed URL stays the single canonical input of the
//! archive. A provider is a variant of [`Provider`] plus one function that fetches and one that
//! parses.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;

const ITUNES_TIMEOUT: Duration = Duration::from_secs(15);
// fyyd reports answering within a second, but whole requests have been measured at 15 to
// 30 seconds, so the limit is generous.
const FYYD_TIMEOUT: Duration = Duration::from_secs(45);
const RESULTS_PER_PROVIDER: &str = "20";

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub title: String,
    pub author: Option<String>,
    pub feed_url: String,
    pub image_url: Option<String>,
    pub provider: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Itunes,
    Fyyd,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Itunes, Provider::Fyyd];

    /// Name shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            Provider::Itunes => "iTunes",
            Provider::Fyyd => "fyyd",
        }
    }

    /// Stable identifier used in URLs.
    pub fn key(self) -> &'static str {
        match self {
            Provider::Itunes => "itunes",
            Provider::Fyyd => "fyyd",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// Queries one provider. Providers are queried separately so that a slow directory never holds
/// back the results of a fast one.
pub async fn search_provider(
    client: &reqwest::Client,
    provider: Provider,
    query: &str,
) -> Result<Vec<SearchHit>> {
    match provider {
        Provider::Itunes => itunes(client, query).await,
        Provider::Fyyd => fyyd(client, query).await,
    }
}

/// Feed URLs differ between directories in scheme, case of the host and trailing slashes.
fn dedup_key(feed_url: &str) -> String {
    let rest = feed_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(feed_url);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    format!(
        "{}/{}",
        host.to_ascii_lowercase(),
        path.trim_end_matches('/')
    )
}

const CACHE_TTL: Duration = Duration::from_secs(600);
const CACHE_ENTRIES: usize = 64;

/// Remembers recent results per provider and query.
///
/// It serves two purposes: repeated searches do not hit the directories again (they rate-limit),
/// and a feed that one provider already returned for a query is not shown a second time when
/// another provider answers later.
#[derive(Default)]
pub struct SearchCache {
    entries: Vec<CacheEntry>,
}

struct CacheEntry {
    provider: Provider,
    query: String,
    stored: Instant,
    hits: Vec<SearchHit>,
}

fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl SearchCache {
    pub fn get(&mut self, provider: Provider, query: &str) -> Option<Vec<SearchHit>> {
        let query = normalize_query(query);
        self.entries.retain(|e| e.stored.elapsed() < CACHE_TTL);
        self.entries
            .iter()
            .find(|e| e.provider == provider && e.query == query)
            .map(|e| e.hits.clone())
    }

    /// Stores a provider's results and returns them without the feeds that another provider
    /// already delivered for the same query. Whoever answers first keeps the hit.
    pub fn store(
        &mut self,
        provider: Provider,
        query: &str,
        hits: Vec<SearchHit>,
    ) -> Vec<SearchHit> {
        let query = normalize_query(query);
        self.entries.retain(|e| {
            e.stored.elapsed() < CACHE_TTL && !(e.provider == provider && e.query == query)
        });
        let taken: HashSet<String> = self
            .entries
            .iter()
            .filter(|e| e.query == query)
            .flat_map(|e| e.hits.iter().map(|h| dedup_key(&h.feed_url)))
            .collect();
        let mut seen = taken;
        let hits: Vec<SearchHit> = hits
            .into_iter()
            .filter(|h| seen.insert(dedup_key(&h.feed_url)))
            .collect();
        if self.entries.len() >= CACHE_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(CacheEntry {
            provider,
            query,
            stored: Instant::now(),
            hits: hits.clone(),
        });
        hits
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ── iTunes ─────────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ItunesResponse {
    #[serde(default)]
    results: Vec<ItunesResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesResult {
    collection_name: Option<String>,
    artist_name: Option<String>,
    feed_url: Option<String>,
    artwork_url600: Option<String>,
    artwork_url100: Option<String>,
}

async fn itunes(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>> {
    let body = client
        .get("https://itunes.apple.com/search")
        .query(&[
            ("media", "podcast"),
            ("limit", RESULTS_PER_PROVIDER),
            ("term", query),
        ])
        .timeout(ITUNES_TIMEOUT)
        .send()
        .await
        .context("iTunes search is unreachable")?
        .error_for_status()
        .context("iTunes search returned an error")?
        .bytes()
        .await
        .context("iTunes search was interrupted")?;
    parse_itunes(&body)
}

fn parse_itunes(body: &[u8]) -> Result<Vec<SearchHit>> {
    let response: ItunesResponse =
        serde_json::from_slice(body).context("iTunes search returned an unexpected answer")?;
    Ok(response
        .results
        .into_iter()
        .filter_map(|r| {
            Some(SearchHit {
                title: non_empty(r.collection_name)?,
                author: non_empty(r.artist_name),
                feed_url: non_empty(r.feed_url)?,
                image_url: non_empty(r.artwork_url600).or(non_empty(r.artwork_url100)),
                provider: "iTunes",
            })
        })
        .collect())
}

// ── fyyd ───────────────────────────────────────────────────────────────────────────────────
// API documentation: https://codeberg.org/eazy/fyyd-api (version 0.2, no key needed for search).

#[derive(Deserialize)]
struct FyydResponse {
    #[serde(default)]
    data: Vec<FyydPodcast>,
}

#[derive(Deserialize)]
struct FyydPodcast {
    title: Option<String>,
    #[serde(rename = "xmlURL")]
    xml_url: Option<String>,
    #[serde(rename = "layoutImageURL")]
    layout_image_url: Option<String>,
    #[serde(rename = "imgURL")]
    img_url: Option<String>,
    author: Option<String>,
    // Popularity within fyyd. Read leniently: an unexpected type must not fail the search.
    rank: Option<serde_json::Value>,
}

async fn fyyd(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>> {
    let body = client
        .get("https://api.fyyd.de/0.2/search/podcast")
        // `term` searches the whole podcast and finds more than `title`, which matches the
        // title only literally ("freakshow" does not find "Freak Show").
        .query(&[("term", query), ("count", RESULTS_PER_PROVIDER)])
        .timeout(FYYD_TIMEOUT)
        .send()
        .await
        .context("fyyd search is unreachable")?
        .error_for_status()
        .context("fyyd search returned an error")?
        .bytes()
        .await
        .context("fyyd search was interrupted")?;
    parse_fyyd(&body, query)
}

/// Letters and digits only, lowercased: "Freak Show" and "freakshow" compare equal.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// fyyd returns full-text matches in no useful order. Podcasts whose title contains the query
/// come first, the more popular ones (by fyyd's rank) before the less popular.
fn parse_fyyd(body: &[u8], query: &str) -> Result<Vec<SearchHit>> {
    let response: FyydResponse =
        serde_json::from_slice(body).context("fyyd search returned an unexpected answer")?;
    let needle = squash(query);
    let mut ranked: Vec<(bool, f64, SearchHit)> = response
        .data
        .into_iter()
        .filter_map(|p| {
            let title = non_empty(p.title)?;
            let in_title = !needle.is_empty() && squash(&title).contains(&needle);
            let rank = p.rank.as_ref().and_then(|r| r.as_f64()).unwrap_or(0.0);
            let hit = SearchHit {
                title,
                author: non_empty(p.author),
                feed_url: non_empty(p.xml_url)?,
                // The image hosted by fyyd is preferred: it is served over https and resized.
                image_url: non_empty(p.layout_image_url).or(non_empty(p.img_url)),
                provider: "fyyd",
            };
            Some((in_title, rank, hit))
        })
        .collect();
    // Stable sort: equal entries keep the order fyyd gave them.
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)));
    Ok(ranked.into_iter().map(|(_, _, hit)| hit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(title: &str, feed_url: &str, provider: &'static str) -> SearchHit {
        SearchHit {
            title: title.into(),
            author: None,
            feed_url: feed_url.into(),
            image_url: None,
            provider,
        }
    }

    #[test]
    fn parses_itunes() {
        let body = br#"{"resultCount":2,"results":[
            {"collectionName":"Radiolab","artistName":"WNYC Studios",
             "feedUrl":"https://feeds.example/radiolab","artworkUrl100":"https://img.example/100.jpg",
             "artworkUrl600":"https://img.example/600.jpg","trackCount":500},
            {"collectionName":"No feed","artistName":"Nobody"}]}"#;
        let hits = parse_itunes(body).unwrap();
        assert_eq!(hits.len(), 1, "results without a feed URL are useless");
        assert_eq!(hits[0].title, "Radiolab");
        assert_eq!(hits[0].author.as_deref(), Some("WNYC Studios"));
        assert_eq!(
            hits[0].image_url.as_deref(),
            Some("https://img.example/600.jpg")
        );
    }

    #[test]
    fn parses_fyyd() {
        // Shape and order of a real answer to `term=freakshow` (November 2026), shortened.
        let body = br##"{"status":1,"msg":"ok","meta":{"paging":{"count":5,"page":0,"first_page":0,
            "last_page":0,"next_page":null,"prev_page":null},"API_INFO":{"API_VERSION":"0.2"},
            "SERVER":"192.0.2.1","duration":0},"data":[
            {"title":"Charles Band's Full Moon Freakshow","id":82022,
             "xmlURL":"https:\/\/anchor.fm\/s\/93f097fc\/podcast\/rss",
             "imgURL":"https:\/\/cdn.example\/original.jpg","status":200,
             "layoutImageURL":"https:\/\/img-1.fyyd.de\/pd\/layout\/82022.jpg",
             "language":"en","categories":[205,208],"rank":2,"subtitle":"Long text",
             "episode_count":37,"iflags":null,"paymentURL":null,"author":"Charles Band"},
            {"title":"SmartH\u00fctte","id":90243,"xmlURL":"https:\/\/feeds.transistor.fm\/smarthuette",
             "layoutImageURL":"https:\/\/img-1.fyyd.de\/pd\/layout\/90243.jpg","categories":[203],
             "rank":8,"iflags":"99","author":"Andrej Friesen, Thomas Wiebe"},
            {"title":"Freak Show","id":85,"xmlURL":"https:\/\/feeds.metaebene.me\/freakshow\/mp3",
             "layoutImageURL":"https:\/\/img-1.fyyd.de\/pd\/layout\/85.jpg","categories":[203,181],
             "rank":292,"author":"Metaebene Personal Media - Tim Pritlove"},
            {"title":"No rank","id":7,"xmlURL":"http:\/\/second.example\/feed",
             "imgURL":"http:\/\/second.example\/cover.jpg","categories":38,"rank":"n\/a","author":""},
            {"title":"Broken","id":1}]}"##;
        let hits = parse_fyyd(body, "freakshow").unwrap();
        let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Freak Show",
                "Charles Band's Full Moon Freakshow",
                "SmartH\u{fc}tte",
                "No rank"
            ],
            "title matches first, then by rank"
        );
        assert_eq!(hits[0].feed_url, "https://feeds.metaebene.me/freakshow/mp3");
        assert_eq!(
            hits[0].author.as_deref(),
            Some("Metaebene Personal Media - Tim Pritlove")
        );
        assert_eq!(
            hits[0].image_url.as_deref(),
            Some("https://img-1.fyyd.de/pd/layout/85.jpg")
        );
        assert_eq!(hits[3].author, None);
        assert_eq!(
            hits[3].image_url.as_deref(),
            Some("http://second.example/cover.jpg")
        );
        assert!(
            parse_fyyd(br#"{"status":0,"msg":"error"}"#, "x")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cache_deduplicates_across_providers() {
        let mut cache = SearchCache::default();
        assert!(cache.get(Provider::Itunes, "birds").is_none());
        let first = cache.store(
            Provider::Itunes,
            "Birds",
            vec![
                hit("A", "https://Feeds.Example/a/", "iTunes"),
                hit("B", "https://feeds.example/b", "iTunes"),
                hit("B twice", "https://feeds.example/b", "iTunes"),
            ],
        );
        assert_eq!(first.len(), 2);
        let second = cache.store(
            Provider::Fyyd,
            "  birds ",
            vec![
                hit("A again", "http://feeds.example/a", "fyyd"),
                hit("C", "https://feeds.example/c", "fyyd"),
            ],
        );
        let titles: Vec<&str> = second.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(
            titles,
            ["C"],
            "a feed another provider already showed is dropped"
        );
        assert_eq!(cache.get(Provider::Fyyd, "BIRDS").unwrap().len(), 1);
        assert_eq!(cache.get(Provider::Itunes, "birds").unwrap().len(), 2);
        // A different query is unaffected.
        let other = cache.store(
            Provider::Fyyd,
            "owls",
            vec![hit("A", "https://feeds.example/a", "fyyd")],
        );
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn provider_keys_round_trip() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_key(provider.key()), Some(provider));
        }
        assert_eq!(Provider::from_key("nope"), None);
    }
}
