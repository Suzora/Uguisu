//! A feed's bytes through the parser, the normalizer and the identity
//! cascade, as a refresh takes them: none of them may panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use time::OffsetDateTime;
use uguisu_core::config::FeedLimits;
use uguisu_core::ids::EpisodeId;
use uguisu_feed::identity::{comparable_hash, resolve_identities, signals};
use uguisu_feed::normalize::{normalize_channel, normalize_item};

/// Normalizing compares dates with now; a fixed one makes a finding replay.
const NOW: OffsetDateTime = time::macros::datetime!(2026-10-10 12:00 UTC);

fuzz_target!(|data: &[u8]| {
    let Ok(parsed) = uguisu_feed::parse(data, &FeedLimits::default()) else {
        return;
    };
    let _ = normalize_channel(&parsed.channel);
    let mut identities = Vec::with_capacity(parsed.items.len());
    for (n, item) in (0u128..).zip(&parsed.items) {
        let normalized = normalize_item(item, EpisodeId::from_ulid(ulid::Ulid(n)), NOW);
        let _ = comparable_hash(&normalized);
        identities.push(signals(item, &normalized));
    }
    let _ = resolve_identities(&identities);
});
