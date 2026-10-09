//! The provider-independent candidate model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::provider::ProviderId;
use url::Url;

/// Where a provider knows the podcast from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderIdentity {
    /// The provider.
    pub provider: ProviderId,
    /// The provider's own identifier (Apple `collectionId`, Podcast Index feed `id`, gpodder feed URL…).
    pub provider_ref: String,
    /// The provider's confidence that this record matches the query (0..1).
    pub confidence: f32,
    /// A directory page for the podcast at this provider, when one exists.
    pub url: Option<Url>,
    /// When the record was fetched.
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
}

/// A popularity signal as supplied by one provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Popularity {
    /// Provider that supplied it.
    pub provider: ProviderId,
    /// Raw value (subscribers, rank…).
    pub raw: f64,
    /// Provider-specific normalization to 0..1.
    pub normalized: f32,
    /// What the raw value means (`subscribers`, `rank`).
    pub label: String,
}

/// Feed health hints reported by directories (never authoritative).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FeedHealthHints {
    /// Directory marks the feed as dead.
    pub dead: Option<bool>,
    /// `podcast:locked` as seen by the directory.
    pub locked: Option<bool>,
    /// Last time the directory saw the feed change.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_update: Option<OffsetDateTime>,
    /// Last HTTP status the directory got from the feed.
    pub http_status: Option<u16>,
}

/// A podcast as seen by one or more providers, normalized.
///
/// Only information the provider actually supplied is set; nothing is
/// invented. `provenance` records which provider supplied each field so a
/// merged candidate can still explain itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastCandidate {
    /// Show title.
    pub title: String,
    /// Author / host.
    pub author: Option<String>,
    /// Publisher or owner name.
    pub publisher: Option<String>,
    /// Description, plain text.
    pub description: Option<String>,
    /// Artwork URL.
    pub artwork: Option<Url>,
    /// Language tag.
    pub language: Option<String>,
    /// Category names.
    pub categories: Vec<String>,
    /// Podcast website.
    pub website: Option<Url>,
    /// Feed URL (`None` means the provider knows the show but not its feed).
    pub feed_url: Option<Url>,
    /// Episode count as reported.
    pub episode_count: Option<u32>,
    /// Newest episode date as reported.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_published: Option<OffsetDateTime>,
    /// Explicit flag as reported.
    pub explicit: Option<bool>,
    /// Apple/iTunes collection id (cross-provider identifier).
    pub itunes_id: Option<u64>,
    /// Podcasting 2.0 `podcast:guid` (cross-provider identifier).
    pub podcast_guid: Option<String>,
    /// Every provider that contributed to this candidate.
    pub identities: Vec<ProviderIdentity>,
    /// Popularity signals, one per supplying provider.
    pub popularity: Vec<Popularity>,
    /// Health hints.
    pub health: FeedHealthHints,
    /// Field name → provider that supplied the value.
    pub provenance: BTreeMap<String, ProviderId>,
}

impl PodcastCandidate {
    /// Creates a candidate with a title and one identity; everything else unset.
    pub fn new(title: impl Into<String>, identity: ProviderIdentity) -> Self {
        Self {
            title: title.into(),
            author: None,
            publisher: None,
            description: None,
            artwork: None,
            language: None,
            categories: Vec::new(),
            website: None,
            feed_url: None,
            episode_count: None,
            last_published: None,
            explicit: None,
            itunes_id: None,
            podcast_guid: None,
            identities: vec![identity],
            popularity: Vec::new(),
            health: FeedHealthHints::default(),
            provenance: BTreeMap::new(),
        }
    }

    /// Records `provider` as the source of every field that is currently set.
    /// Providers call this once after filling a candidate.
    pub fn attribute_all_to(&mut self, provider: ProviderId) {
        let mut set = |name: &str, present: bool| {
            if present {
                self.provenance.insert(name.to_owned(), provider);
            }
        };
        set("title", true);
        set("author", self.author.is_some());
        set("publisher", self.publisher.is_some());
        set("description", self.description.is_some());
        set("artwork", self.artwork.is_some());
        set("language", self.language.is_some());
        set("categories", !self.categories.is_empty());
        set("website", self.website.is_some());
        set("feed_url", self.feed_url.is_some());
        set("episode_count", self.episode_count.is_some());
        set("last_published", self.last_published.is_some());
        set("explicit", self.explicit.is_some());
        set("itunes_id", self.itunes_id.is_some());
        set("podcast_guid", self.podcast_guid.is_some());
    }

    /// The provider of the first identity (the one that created the candidate).
    pub fn primary_provider(&self) -> Option<ProviderId> {
        self.identities.first().map(|i| i.provider)
    }

    /// Distinct providers, in identity order.
    pub fn providers(&self) -> Vec<ProviderId> {
        let mut out: Vec<ProviderId> = Vec::new();
        for id in &self.identities {
            if !out.contains(&id.provider) {
                out.push(id.provider);
            }
        }
        out
    }

    /// Whether the candidate has a feed URL.
    pub const fn has_feed(&self) -> bool {
        self.feed_url.is_some()
    }

    /// Highest normalized popularity across providers.
    pub fn popularity_score(&self) -> Option<f32> {
        self.popularity
            .iter()
            .map(|p| p.normalized)
            .fold(None, |acc, v| Some(acc.map_or(v, |a: f32| a.max(v))))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn identity(provider: ProviderId) -> ProviderIdentity {
        ProviderIdentity {
            provider,
            provider_ref: "1".into(),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn provenance_only_covers_set_fields() {
        let mut c = PodcastCandidate::new("T", identity(ProviderId::APPLE));
        c.author = Some("A".into());
        c.attribute_all_to(ProviderId::APPLE);
        assert_eq!(c.provenance.get("author"), Some(&ProviderId::APPLE));
        assert_eq!(c.provenance.get("feed_url"), None);
        assert_eq!(c.primary_provider(), Some(ProviderId::APPLE));
        assert!(!c.has_feed());
    }

    #[test]
    fn serializes_round_trip() {
        let mut c = PodcastCandidate::new("T", identity(ProviderId::GPODDER_NET));
        c.popularity.push(Popularity {
            provider: ProviderId::GPODDER_NET,
            raw: 10.0,
            normalized: 0.3,
            label: "subscribers".into(),
        });
        let json = serde_json::to_string(&c).unwrap();
        let back: PodcastCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.popularity_score(), Some(0.3));
    }
}
