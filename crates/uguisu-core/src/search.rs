//! Podcast search across directory providers.
//!
//! A search only ever produces feed URLs; the feed URL stays the single canonical input of the
//! archive. New providers are added by writing one function and listing it in [`search`].

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub title: String,
    pub author: Option<String>,
    pub feed_url: String,
    pub image_url: Option<String>,
    pub provider: &'static str,
}

/// Queries all providers and merges the results, dropping duplicate feed URLs.
/// A failing provider is logged and skipped; an error is returned only if all of them fail.
pub async fn search(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let results = [("iTunes", itunes(client, query).await)];

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut last_error = None;
    let mut any_ok = false;
    for (provider, result) in results {
        match result {
            Ok(found) => {
                any_ok = true;
                for hit in found {
                    if !hits.iter().any(|h| h.feed_url == hit.feed_url) {
                        hits.push(hit);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("search provider {provider} failed: {e:#}");
                last_error = Some(e);
            }
        }
    }
    match (any_ok, last_error) {
        (false, Some(e)) => Err(e),
        _ => Ok(hits),
    }
}

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
    let response: ItunesResponse = client
        .get("https://itunes.apple.com/search")
        .query(&[("media", "podcast"), ("limit", "24"), ("term", query)])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("iTunes search is unreachable")?
        .error_for_status()
        .context("iTunes search returned an error")?
        .json()
        .await
        .context("iTunes search returned an unexpected answer")?;
    Ok(response
        .results
        .into_iter()
        .filter_map(|r| {
            Some(SearchHit {
                title: r.collection_name?,
                author: r.artist_name,
                feed_url: r.feed_url?,
                image_url: r.artwork_url600.or(r.artwork_url100),
                provider: "iTunes",
            })
        })
        .collect())
}
