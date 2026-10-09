//! Domain model and shared types for Uguisu.
//!
//! This crate is the foundation of the workspace and depends on no other
//! Uguisu crate. It holds the identifiers ([`ids`]), the domain model
//! ([`model`]), the feed-refresh vocabulary ([`feed`]), domain events
//! ([`events`]), the download vocabulary ([`download`]), the cross-crate
//! error ([`error`]), the refresh schedule ([`schedule`]), configuration
//! types and secrets that every other crate shares.
//!
//! See `docs/ARCHITECTURE.md` §2 and `docs/DATA_MODEL.md`.

pub mod archive;
pub mod auth;
pub mod config;
pub mod download;
pub mod error;
pub mod events;
pub mod feed;
pub mod ids;
pub mod model;
pub mod page;
pub mod provider;
pub mod redact;
pub mod schedule;
pub mod search;
pub mod secret;
pub mod settings;

pub use error::UguisuError;
pub use events::{Event, EventKind};
pub use ids::{
    ArchiveFileId, AttemptId, ChangeId, EnclosureId, EpisodeId, EventId, FetchId, Id, JobId,
    PodcastId, SourceId,
};

/// The Uguisu version, taken from the workspace package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The user agent Uguisu identifies itself with on the network.
///
/// The shared HTTP client (`uguisu-http`) is the only place that sends it.
pub const USER_AGENT: &str = concat!(
    "Uguisu/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/suzora/uguisu)"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_semver_like() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "VERSION should be MAJOR.MINOR.PATCH");
        for part in parts {
            assert!(
                part.parse::<u32>().is_ok(),
                "non-numeric version part: {part}"
            );
        }
    }

    #[test]
    fn user_agent_names_the_product() {
        assert!(USER_AGENT.starts_with("Uguisu/"));
        assert!(USER_AGENT.contains(VERSION));
    }
}
