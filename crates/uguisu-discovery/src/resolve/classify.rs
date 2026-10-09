//! Classifies pasted URLs: directory pages that need a provider lookup,
//! platforms that publish no feed, or ordinary URLs to fetch.

use url::Url;

/// What a URL points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlClass {
    /// An Apple Podcasts page; `id` is the collection id, `country` the storefront.
    ApplePage {
        /// Collection id.
        id: u64,
        /// Storefront country code, when present.
        country: Option<String>,
    },
    /// A podcastindex.org page; `id` is the Podcast Index feed id.
    PodcastIndexPage {
        /// Feed id.
        id: u64,
    },
    /// A Spotify show page; Spotify exposes no RSS feed.
    SpotifyShow {
        /// Show id.
        id: String,
    },
    /// A fyyd.de page; the fyyd provider is deferred.
    FyydPage {
        /// Podcast id.
        id: u64,
    },
    /// A YouTube channel or playlist; not a podcast feed source.
    YouTube,
    /// Anything else: fetch it and look at what comes back.
    Generic,
}

/// Classifies a URL.
pub fn classify(url: &Url) -> UrlClass {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();

    match host.as_str() {
        "podcasts.apple.com" | "itunes.apple.com" | "music.apple.com" => {
            if let Some(id) = segments
                .iter()
                .rev()
                .find_map(|s| s.strip_prefix("id").and_then(|n| n.parse::<u64>().ok()))
            {
                let country = segments
                    .first()
                    .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()))
                    .map(|s| s.to_ascii_uppercase());
                return UrlClass::ApplePage { id, country };
            }
            // Old style lookup URLs carry the id in the query.
            if let Some(id) = url
                .query_pairs()
                .find(|(k, _)| k == "id")
                .and_then(|(_, v)| v.parse::<u64>().ok())
            {
                return UrlClass::ApplePage { id, country: None };
            }
        }
        "podcastindex.org" => {
            if segments.first() == Some(&"podcast")
                && let Some(id) = segments.get(1).and_then(|s| s.parse::<u64>().ok())
            {
                return UrlClass::PodcastIndexPage { id };
            }
        }
        "open.spotify.com" => {
            if segments.first() == Some(&"show")
                && let Some(id) = segments.get(1)
            {
                return UrlClass::SpotifyShow {
                    id: (*id).to_owned(),
                };
            }
        }
        "fyyd.de" => {
            if segments.first() == Some(&"podcast")
                && let Some(id) = segments.iter().rev().find_map(|s| s.parse::<u64>().ok())
            {
                return UrlClass::FyydPage { id };
            }
        }
        "youtube.com" | "m.youtube.com" | "youtu.be" | "music.youtube.com" => {
            return UrlClass::YouTube;
        }
        _ => {}
    }
    UrlClass::Generic
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn c(s: &str) -> UrlClass {
        classify(&Url::parse(s).unwrap())
    }

    #[test]
    fn apple_pages() {
        assert_eq!(
            c("https://podcasts.apple.com/us/podcast/darknet-diaries/id1296350485"),
            UrlClass::ApplePage {
                id: 1_296_350_485,
                country: Some("US".into())
            }
        );
        assert_eq!(
            c("https://podcasts.apple.com/podcast/id123?i=456"),
            UrlClass::ApplePage {
                id: 123,
                country: None
            }
        );
        assert_eq!(
            c("https://itunes.apple.com/lookup?id=999"),
            UrlClass::ApplePage {
                id: 999,
                country: None
            }
        );
        assert_eq!(c("https://podcasts.apple.com/us/browse"), UrlClass::Generic);
    }

    #[test]
    fn other_directories_and_platforms() {
        assert_eq!(
            c("https://podcastindex.org/podcast/75075"),
            UrlClass::PodcastIndexPage { id: 75075 }
        );
        assert_eq!(
            c("https://open.spotify.com/show/4XPl3uEEL9hvqMkoZrzbx5?si=x"),
            UrlClass::SpotifyShow {
                id: "4XPl3uEEL9hvqMkoZrzbx5".into()
            }
        );
        assert_eq!(
            c("https://fyyd.de/podcast/darknet-diaries/12345"),
            UrlClass::FyydPage { id: 12345 }
        );
        assert_eq!(c("https://www.youtube.com/@channel"), UrlClass::YouTube);
        assert_eq!(c("https://darknetdiaries.com/"), UrlClass::Generic);
        assert_eq!(
            c("https://feeds.megaphone.fm/darknetdiaries"),
            UrlClass::Generic
        );
    }
}
