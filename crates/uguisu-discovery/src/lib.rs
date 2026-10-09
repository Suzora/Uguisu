//! Podcast discovery and feed resolution.
//!
//! Turns a human query or a pasted URL into ranked, deduplicated
//! [`PodcastCandidate`]s and, on selection, a verified feed. Providers
//! (Apple, Podcast Index, gpodder.net) implement [`DiscoveryProvider`] and
//! never leak their response formats; ranking is explainable; merges below
//! strong confidence are surfaced as ambiguity rather than applied.
//!
//! This crate has **no dependency on storage or the archive**: a failing
//! provider can never affect an existing subscription.
//!
//! Pipeline (see `docs/DISCOVERY.md`):
//!
//! ```text
//! query → NormalizedQuery → ProviderRegistry fan-out → Vec<PodcastCandidate>
//!       → dedup → merge → rank → SearchResponse
//! selection / URL → Resolver → ResolvedFeed
//! ```

pub mod assemble;
pub mod cache;
pub mod candidate;
pub mod dedup;
pub mod error;
pub mod fuzzy;
pub mod merge;
pub mod provider;
pub mod providers;
pub mod query;
pub mod rank;
pub mod registry;
pub mod resolve;
pub mod search;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use assemble::{AssembleError, Discovery, assemble, assemble_with_cache_store};
pub use cache::{CacheStats, DiscoveryCache};
pub use candidate::{FeedHealthHints, PodcastCandidate, Popularity, ProviderIdentity};
pub use dedup::{AmbiguityNote, DedupGroup, DedupThresholds, MergeReason};
pub use error::SearchOutcome;
pub use provider::{
    Capabilities, DiscoveryProvider, ProviderContext, ProviderError, ProviderInfo, ProviderRef,
    ProviderResponse,
};
pub use query::{NormalizedQuery, QueryKind, fold_text, tokenize};
pub use rank::{RankContext, RankedCandidate, RankingExplanation, RankingWeights, Signal};
pub use registry::{
    CircuitState, ManagedCall, ProviderHealth, ProviderRegistry, ProviderStatus, RegistryConfig,
};
pub use resolve::{
    ResolutionStep, ResolveError, ResolveFailure, ResolvedFeed, Resolver, ResolverConfig, StepKind,
};
pub use search::{
    ProviderCallStatus, ProviderOutcome, RESPONSE_SCHEMA, RelaxedQuery, SearchEngine,
    SearchRequest, SearchResponse, SearchTiming,
};
pub use uguisu_core::provider::ProviderId;
