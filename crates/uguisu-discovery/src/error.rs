//! Outcome taxonomy shared by CLI and API.

use serde::{Deserialize, Serialize};

/// How a search ended. Distinguishes "nothing matched" from "nothing could
/// be asked" so that the UI never collapses failures into "not found".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
// `uguisu-engine` has a `SearchOutcome` too, for the library search, and the
// OpenAPI document names a schema after the type's last path segment: without
// this the two would collide and one would silently replace the other.
#[schema(as = DiscoverySearchOutcome)]
#[serde(rename_all = "snake_case")]
pub enum SearchOutcome {
    /// At least one candidate was returned.
    Results,
    /// Every enabled provider answered and none had a match.
    NoResults,
    /// Every enabled provider failed, timed out or was skipped by its circuit breaker.
    AllProvidersFailed,
    /// No provider is enabled (configuration problem).
    NoProvidersEnabled,
}

impl SearchOutcome {
    /// Stable identifier for logs and exit codes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Results => "results",
            Self::NoResults => "no_results",
            Self::AllProvidersFailed => "all_providers_failed",
            Self::NoProvidersEnabled => "no_providers_enabled",
        }
    }
}
