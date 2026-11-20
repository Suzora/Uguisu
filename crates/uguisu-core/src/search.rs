//! Podcast search across directory providers.
//!
//! A search only ever produces feed URLs; the feed URL stays the single canonical input of the
//! archive. A provider is one function that fetches and one that parses; both are listed in
//! [`search`].

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

const TIMEOUT: Duration = Duration::from_secs(15);
const RESULTS_PER_PROVIDER: &str = "20";

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub title: String,
    pub author: Option<String>,
    pub feed_url: String,
    pub image_url: Option<String>,
    pub provider: &'static str,
}

/// Queries all providers at the same time and merges the results.
/// A failing provider is logged and skipped; an error is returned only if all of them fail.
pub async fn search(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let (itunes, fyyd) = tokio::join!(itunes(client, query), fyyd(client, query));

    let mut lists = Vec::new();
    let mut last_error = None;
    for (provider, result) in [("iTunes", itunes), ("fyyd", fyyd)] {
        match result {
            Ok(hits) => lists.push(hits),
            Err(e) => {
                tracing::warn!("search provider {provider} failed: {e:#}");
                last_error = Some(e);
            }
        }
    }
    match (lists.is_empty(), last_error) {
        (true, Some(e)) => Err(e),
        _ => Ok(merge(lists)),
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

/// Interleaves the provider lists (best hit of each first) and drops duplicate feeds.
fn merge(lists: Vec<Vec<SearchHit>>) -> Vec<SearchHit> {
    let mut merged: Vec<SearchHit> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut iterators: Vec<_> = lists.into_iter().map(Vec::into_iter).collect();
    loop {
        let mut any = false;
        for iterator in &mut iterators {
            if let Some(hit) = iterator.next() {
                any = true;
                if seen.insert(dedup_key(&hit.feed_url)) {
                    merged.push(hit);
                }
            }
        }
        if !any {
            return merged;
        }
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
        .timeout(TIMEOUT)
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
    subtitle: Option<String>,
}

async fn fyyd(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>> {
    let body = client
        .get("https://api.fyyd.de/0.2/search/podcast")
        .query(&[("title", query), ("count", RESULTS_PER_PROVIDER)])
        .timeout(TIMEOUT)
        .send()
        .await
        .context("fyyd search is unreachable")?
        .error_for_status()
        .context("fyyd search returned an error")?
        .bytes()
        .await
        .context("fyyd search was interrupted")?;
    parse_fyyd(&body)
}

fn parse_fyyd(body: &[u8]) -> Result<Vec<SearchHit>> {
    let response: FyydResponse =
        serde_json::from_slice(body).context("fyyd search returned an unexpected answer")?;
    Ok(response
        .data
        .into_iter()
        .filter_map(|p| {
            Some(SearchHit {
                title: non_empty(p.title)?,
                // fyyd has no author field; the subtitle is the closest short description.
                author: non_empty(p.subtitle),
                feed_url: non_empty(p.xml_url)?,
                // The image hosted by fyyd is preferred: it is served over https and resized.
                image_url: non_empty(p.layout_image_url).or(non_empty(p.img_url)),
                provider: "fyyd",
            })
        })
        .collect())
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
        // Shape taken from the API documentation, including `categories` being a list or a number.
        let body = br#"{"status":1,"msg":"ok","meta":{"API_INFO":{"API_VERSION":0.2}},"data":[
            {"title":"Freak Show","id":85,"xmlURL":"https:\/\/feeds.metaebene.me\/freakshow\/m4a",
             "htmlURL":"http:\/\/freakshow.fm","imgURL":"http:\/\/freakshow.fm\/cover.jpg",
             "layoutImageURL":"https:\/\/img.fyyd.de\/pd\/layout\/85.jpg","language":"de",
             "categories":[52,61],"rank":32,"subtitle":"Menschen! Technik! Sensationen!"},
            {"title":"Second","id":36645,"xmlURL":"http:\/\/second.example\/feed",
             "imgURL":"http:\/\/second.example\/cover.jpg","categories":38,"subtitle":""},
            {"title":"Broken","id":1}]}"#;
        let hits = parse_fyyd(body).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].feed_url, "https://feeds.metaebene.me/freakshow/m4a");
        assert_eq!(
            hits[0].author.as_deref(),
            Some("Menschen! Technik! Sensationen!")
        );
        assert_eq!(
            hits[0].image_url.as_deref(),
            Some("https://img.fyyd.de/pd/layout/85.jpg")
        );
        assert_eq!(hits[1].author, None);
        assert_eq!(
            hits[1].image_url.as_deref(),
            Some("http://second.example/cover.jpg")
        );
        assert!(
            parse_fyyd(br#"{"status":0,"msg":"error"}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn merges_and_deduplicates() {
        let merged = merge(vec![
            vec![
                hit("A", "https://Feeds.Example/a/", "iTunes"),
                hit("B", "https://feeds.example/b", "iTunes"),
            ],
            vec![
                hit("C", "https://feeds.example/c", "fyyd"),
                hit("A again", "http://feeds.example/a", "fyyd"),
                hit("D", "https://feeds.example/d", "fyyd"),
            ],
        ]);
        let titles: Vec<&str> = merged.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, ["A", "C", "B", "D"]);
    }
}
