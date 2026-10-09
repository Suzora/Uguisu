//! Feed URL patterns for hosting platforms whose feed location can be
//! derived from the show page URL without crawling. Only patterns that are
//! fully determined by the URL are listed; platforms that need an opaque
//! numeric id (Anchor, Simplecast, SoundCloud) rely on HTML autodiscovery.
//!
//! Patterns are candidates, never trusted: every derived URL is fetched and
//! validated like any other feed.

use url::Url;

/// Derives candidate feed URLs for known hosting platforms.
pub fn platform_feed_candidates(page: &Url) -> Vec<Url> {
    let host = page.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let segs: Vec<&str> = page
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let mut out: Vec<String> = Vec::new();

    let subdomain_of = |suffix: &str| -> Option<String> {
        host.strip_suffix(suffix)
            .map(|s| s.trim_end_matches('.').to_owned())
            .filter(|s| !s.is_empty() && !s.contains('.'))
    };

    if let Some(show) = subdomain_of(".libsyn.com") {
        out.push(format!("https://{show}.libsyn.com/rss"));
    }
    if host == "buzzsprout.com"
        && let Some(id) = segs
            .first()
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
    {
        out.push(format!("https://feeds.buzzsprout.com/{id}.rss"));
    }
    if let Some(slug) = subdomain_of(".transistor.fm").filter(|s| s != "feeds" && s != "share") {
        out.push(format!("https://feeds.transistor.fm/{slug}"));
    }
    if host == "share.transistor.fm"
        && segs.first() == Some(&"s")
        && let Some(slug) = segs.get(1)
    {
        out.push(format!("https://feeds.transistor.fm/{slug}"));
    }
    if let Some(slug) = subdomain_of(".podbean.com").filter(|s| s != "feed") {
        out.push(format!("https://feed.podbean.com/{slug}/feed.xml"));
    }
    if let Some(slug) = subdomain_of(".captivate.fm").filter(|s| s != "feeds") {
        out.push(format!("https://feeds.captivate.fm/{slug}/"));
    }
    if host == "rss.com"
        && segs.first() == Some(&"podcasts")
        && let Some(slug) = segs.get(1)
    {
        out.push(format!("https://media.rss.com/{slug}/feed.xml"));
    }
    if let Some(slug) = subdomain_of(".podigee.io") {
        out.push(format!("https://{slug}.podigee.io/feed/mp3"));
    }
    if host == "shows.acast.com"
        && let Some(slug) = segs.first()
    {
        out.push(format!("https://feeds.acast.com/public/shows/{slug}"));
    }
    if host == "spreaker.com"
        && segs.first() == Some(&"show")
        && let Some(id) = segs
            .get(1)
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
    {
        out.push(format!("https://www.spreaker.com/show/{id}/episodes/feed"));
    }
    if host == "audioboom.com"
        && segs.first() == Some(&"channels")
        && let Some(id) = segs.get(1).map(|s| s.trim_end_matches(".rss"))
    {
        out.push(format!("https://audioboom.com/channels/{id}.rss"));
    }
    if let Some(slug) = subdomain_of(".megaphone.fm").filter(|s| s != "feeds" && s != "playlist") {
        out.push(format!("https://feeds.megaphone.fm/{slug}"));
    }

    out.into_iter()
        .filter_map(|s| Url::parse(&s).ok())
        .collect()
}

/// Well-known feed paths tried on a site when nothing else worked.
pub const WELL_KNOWN_PATHS: [&str; 7] = [
    "/feed/podcast/",
    "/feed",
    "/rss",
    "/feed.xml",
    "/rss.xml",
    "/podcast.rss",
    "/podcast/feed",
];

/// Candidate URLs from the well-known paths at the site root.
pub fn well_known_candidates(page: &Url) -> Vec<Url> {
    let mut root = page.clone();
    root.set_path("/");
    root.set_query(None);
    root.set_fragment(None);
    WELL_KNOWN_PATHS
        .iter()
        .filter_map(|p| root.join(p).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn first(s: &str) -> Option<String> {
        platform_feed_candidates(&Url::parse(s).unwrap())
            .first()
            .map(Url::to_string)
    }

    #[test]
    fn derives_known_platform_feeds() {
        assert_eq!(
            first("https://myshow.libsyn.com/website").as_deref(),
            Some("https://myshow.libsyn.com/rss")
        );
        assert_eq!(
            first("https://www.buzzsprout.com/123456").as_deref(),
            Some("https://feeds.buzzsprout.com/123456.rss")
        );
        assert_eq!(
            first("https://myshow.transistor.fm/episodes").as_deref(),
            Some("https://feeds.transistor.fm/myshow")
        );
        assert_eq!(
            first("https://share.transistor.fm/s/abc123").as_deref(),
            Some("https://feeds.transistor.fm/abc123")
        );
        assert_eq!(
            first("https://myshow.podbean.com/").as_deref(),
            Some("https://feed.podbean.com/myshow/feed.xml")
        );
        assert_eq!(
            first("https://myshow.captivate.fm/listen").as_deref(),
            Some("https://feeds.captivate.fm/myshow/")
        );
        assert_eq!(
            first("https://rss.com/podcasts/myshow/").as_deref(),
            Some("https://media.rss.com/myshow/feed.xml")
        );
        assert_eq!(
            first("https://myshow.podigee.io/").as_deref(),
            Some("https://myshow.podigee.io/feed/mp3")
        );
        assert_eq!(
            first("https://shows.acast.com/myshow/episodes").as_deref(),
            Some("https://feeds.acast.com/public/shows/myshow")
        );
        assert_eq!(
            first("https://www.spreaker.com/show/1234567").as_deref(),
            Some("https://www.spreaker.com/show/1234567/episodes/feed")
        );
        assert_eq!(
            first("https://audioboom.com/channels/5012345").as_deref(),
            Some("https://audioboom.com/channels/5012345.rss")
        );
        assert_eq!(
            first("https://darknet.megaphone.fm/").as_deref(),
            Some("https://feeds.megaphone.fm/darknet")
        );
    }

    #[test]
    fn ignores_unknown_hosts_and_feed_subdomains() {
        assert!(
            platform_feed_candidates(&Url::parse("https://example.com/podcast").unwrap())
                .is_empty()
        );
        assert!(
            platform_feed_candidates(&Url::parse("https://feeds.transistor.fm/x").unwrap())
                .is_empty()
        );
        assert!(
            platform_feed_candidates(&Url::parse("https://a.b.libsyn.com/").unwrap()).is_empty(),
            "nested subdomains are not shows"
        );
    }

    #[test]
    fn well_known_paths_are_rooted() {
        let c = well_known_candidates(&Url::parse("https://example.com/some/page?x=1").unwrap());
        assert_eq!(c.len(), WELL_KNOWN_PATHS.len());
        assert_eq!(c[0].as_str(), "https://example.com/feed/podcast/");
        assert_eq!(c[1].as_str(), "https://example.com/feed");
    }
}
