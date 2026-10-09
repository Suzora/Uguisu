//! HTML feed autodiscovery (`<link rel="alternate" type="application/rss+xml">`).

use scraper::{Html, Selector};
use url::Url;

/// A feed link found in a page, ordered by how likely it is the podcast feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedLink {
    /// Absolute URL.
    pub url: Url,
    /// `type` attribute, lowercased.
    pub mime: String,
    /// `title` attribute.
    pub title: Option<String>,
    /// Heuristic priority (lower is better).
    pub priority: u8,
}

const FEED_TYPES: [&str; 5] = [
    "application/rss+xml",
    "application/atom+xml",
    "application/xml",
    "text/xml",
    "application/rdf+xml",
];

/// Extracts feed links, resolves them against the page URL (honouring
/// `<base href>`), drops duplicates and sorts by priority: links whose
/// title mentions "podcast" first, RSS before Atom before generic XML,
/// comment feeds last.
pub fn discover_feed_links(page_url: &Url, html: &str) -> Vec<FeedLink> {
    let doc = Html::parse_document(html);
    let base_sel = Selector::parse("base[href]").ok();
    let base = base_sel
        .as_ref()
        .and_then(|s| doc.select(s).next())
        .and_then(|b| b.value().attr("href"))
        .and_then(|h| page_url.join(h.trim()).ok())
        .unwrap_or_else(|| page_url.clone());

    let Ok(link_sel) = Selector::parse("link[rel][href]") else {
        return Vec::new();
    };
    let mut out: Vec<FeedLink> = Vec::new();
    for el in doc.select(&link_sel) {
        let v = el.value();
        let rel = v.attr("rel").unwrap_or_default().to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "alternate") {
            continue;
        }
        let mime = v
            .attr("type")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let mime = mime.split(';').next().unwrap_or_default().trim().to_owned();
        if !FEED_TYPES.contains(&mime.as_str()) {
            continue;
        }
        let Some(href) = v.attr("href") else { continue };
        let Ok(url) = base.join(href.trim()) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let title = v
            .attr("title")
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        let lower_title = title.as_deref().unwrap_or_default().to_ascii_lowercase();
        let path = url.path().to_ascii_lowercase();
        let is_comments = lower_title.contains("comment")
            || path.contains("/comments/")
            || path.ends_with("/comments/feed");
        let mut priority: u8 = match mime.as_str() {
            "application/rss+xml" => 20,
            "application/atom+xml" => 30,
            _ => 40,
        };
        if lower_title.contains("podcast") || path.contains("podcast") {
            priority -= 10;
        }
        if is_comments {
            priority = 90;
        }
        if out.iter().any(|f| f.url == url) {
            continue;
        }
        out.push(FeedLink {
            url,
            mime,
            title,
            priority,
        });
    }
    out.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.url.as_str().cmp(b.url.as_str()))
    });
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn page() -> Url {
        Url::parse("https://example.com/show/page.html").unwrap()
    }

    #[test]
    fn resolves_relative_and_base_hrefs() {
        let html = r#"<html><head><base href="/site/"><link rel="alternate" type="application/rss+xml" href="rss/podcast.xml"></head></html>"#;
        let links = discover_feed_links(&page(), html);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].url.as_str(),
            "https://example.com/site/rss/podcast.xml"
        );
        let html = r#"<link rel="alternate" type="application/rss+xml" href="../feed.xml">"#;
        assert_eq!(
            discover_feed_links(&page(), html)[0].url.as_str(),
            "https://example.com/feed.xml"
        );
    }

    #[test]
    fn prioritizes_podcast_titled_rss_and_demotes_comments() {
        let html = include_str!("../../../../tests/fixtures/websites/link_multiple.html")
            .replace("__BASE__", "https://example.com");
        let links = discover_feed_links(&page(), &html);
        let paths: Vec<&str> = links.iter().map(|l| l.url.path()).collect();
        assert_eq!(paths, vec!["/podcast.rss", "/atom.xml", "/comments/feed"]);
        assert_eq!(links[0].title.as_deref(), Some("Multi Podcast"));
    }

    #[test]
    fn ignores_non_feeds_and_duplicates() {
        let html = r#"<link rel="stylesheet" href="/a.css"><link rel="alternate" type="text/html" href="/x"><link rel="alternate" type="application/rss+xml" href="ftp://example.com/f"><link rel="alternate" type="application/rss+xml; charset=utf-8" href="/f.xml"><link rel="ALTERNATE" type="application/rss+xml" href="/f.xml">"#;
        let links = discover_feed_links(&page(), html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url.path(), "/f.xml");
        assert!(discover_feed_links(&page(), "<html><body>nothing</body></html>").is_empty());
    }
}
