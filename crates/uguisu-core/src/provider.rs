//! Identifiers for discovery providers, and the record of what a
//! resolution decided (ADR 0030).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{DiscoveryRecordId, PodcastId};

/// Identifies a discovery provider (directory, index or resolver).
///
/// Known providers are exposed as constants; the type is open so that
/// additional providers can be registered without touching this crate.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct ProviderId(&'static str);

impl ProviderId {
    /// Apple Podcasts directory via the iTunes Search API.
    pub const APPLE: Self = Self("apple");
    /// Podcast Index (podcastindex.org).
    pub const PODCAST_INDEX: Self = Self("podcastindex");
    /// gpodder.net directory.
    pub const GPODDER_NET: Self = Self("gpoddernet");
    /// fyyd.de directory (not implemented).
    pub const FYYD: Self = Self("fyyd");
    /// Direct RSS / website resolver (no third party involved).
    pub const WEBSITE: Self = Self("website");

    /// All providers that ship with Uguisu, in the order they are shown.
    pub const KNOWN: [Self; 5] = [
        Self::APPLE,
        Self::PODCAST_INDEX,
        Self::GPODDER_NET,
        Self::FYYD,
        Self::WEBSITE,
    ];

    /// Creates an identifier for a provider that is not built in.
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// The identifier as a string (stable; used in config, CLI and API).
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProviderId({})", self.0)
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Error returned when a string names no known provider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown provider `{0}` (known: apple, podcastindex, gpoddernet, fyyd, website)")]
pub struct UnknownProvider(pub String);

impl FromStr for ProviderId {
    type Err = UnknownProvider;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim().to_ascii_lowercase();
        Self::KNOWN
            .into_iter()
            .find(|p| p.0 == wanted || (p.0 == "podcastindex" && wanted == "podcast_index"))
            .ok_or(UnknownProvider(s.to_owned()))
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_known_ids_case_insensitively() {
        assert_eq!("Apple".parse::<ProviderId>().unwrap(), ProviderId::APPLE);
        assert_eq!(
            "podcast_index".parse::<ProviderId>().unwrap(),
            ProviderId::PODCAST_INDEX
        );
        assert!("spotify".parse::<ProviderId>().is_err());
    }

    #[test]
    fn serializes_as_plain_string() {
        let json = serde_json::to_string(&ProviderId::GPODDER_NET).unwrap();
        assert_eq!(json, "\"gpoddernet\"");
        let back: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ProviderId::GPODDER_NET);
    }
}

/// How a resolution ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionOutcome {
    /// A feed was verified.
    Resolved,
    /// Nothing usable was found.
    Unresolved,
    /// Something went wrong on the way (network, policy, a bad status).
    Failed,
}

impl ResolutionOutcome {
    /// Stable string form (also the stored value).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Unresolved => "unresolved",
            Self::Failed => "failed",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "resolved" => Some(Self::Resolved),
            "unresolved" => Some(Self::Unresolved),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// What a resolution decided, and why (ADR 0030).
///
/// **Provenance only.** Nothing reads these rows back as a feed source: a
/// search result does not become a podcast, and `search`, `resolve` and
/// `podcast add` stay three separate things. `podcast_id` is filled in
/// only when the user went on to add one, and survives that podcast's
/// deletion as a `NULL` rather than taking the record with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiscoveryRecord {
    /// Identifier.
    pub id: DiscoveryRecordId,
    /// What the user typed.
    pub input: String,
    /// Which provider answered, when one did.
    pub provider: Option<String>,
    /// The provider's own reference, when there was one.
    pub provider_ref: Option<String>,
    /// The feed that was decided on.
    pub feed_url: Option<String>,
    /// The podcast's website, when the resolution found one.
    pub website: Option<String>,
    /// How it ended.
    pub status: ResolutionOutcome,
    /// One line about why, when there is anything to say.
    pub detail: Option<String>,
    /// The steps taken, as the resolver reported them.
    pub steps: Vec<String>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
    /// The podcast this became, if the user added it.
    pub podcast_id: Option<PodcastId>,
    /// When the resolution ran.
    #[serde(with = "time::serde::rfc3339")]
    pub resolved_at: OffsetDateTime,
    /// When the row was written.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
