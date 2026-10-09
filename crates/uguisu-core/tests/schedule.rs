//! Properties of the refresh schedule (ADR 0027).
//!
//! The unit tests in `schedule.rs` use real identifiers, which exercise
//! the identifier generator but only a corner of the value space. These
//! walk the whole of it: any 128-bit identifier, any interval up to a
//! year, any position of `now` relative to the previous due time.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use proptest::prelude::*;
use time::OffsetDateTime;
use uguisu_core::PodcastId;
use uguisu_core::schedule::{catch_up_spread, plan_next, slot_permille, spread};

fn id(raw: u128) -> PodcastId {
    PodcastId::from_ulid(ulid::Ulid(raw))
}

fn epoch() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

proptest! {
    /// The same podcast always lands in the same slot. Nothing about the
    /// schedule may depend on the clock, the process or the machine — a
    /// scheduling bug has to be reproducible from the database alone.
    #[test]
    fn a_slot_depends_on_the_id(raw: u128) {
        let slot = slot_permille(id(raw));
        prop_assert!(slot < 1000);
        prop_assert_eq!(slot, slot_permille(id(raw)));
    }

    /// The spread is one-sided and bounded: at most a tenth of the
    /// interval normally, at most a whole one when catching up, and never
    /// negative — a fetch may be later than the interval asks, never
    /// earlier than `Cache-Control: max-age` allowed.
    #[test]
    fn a_spread_is_one_sided_and_bounded(raw: u128, secs in 1_u64..=31_536_000) {
        let base = Duration::from_secs(secs);
        let ordinary = spread(id(raw), base);
        let catching_up = catch_up_spread(id(raw), base);
        prop_assert!(ordinary <= base / 10);
        prop_assert!(catching_up <= base);
        prop_assert!(catching_up >= ordinary);
    }

    /// Whatever the history, the next fetch is at least one interval away
    /// and at most two. The floor is what keeps a spread from becoming a
    /// second refresh loop; the ceiling is what keeps a catch-up from
    /// parking a podcast for a day.
    #[test]
    fn a_planned_time_stays_bounded(
        raw: u128,
        secs in 60_u64..=604_800,
        behind_secs in -604_800_i64..=604_800,
        ever_fetched: bool,
    ) {
        let base = Duration::from_secs(secs);
        let now = epoch();
        let previous = ever_fetched.then(|| now + time::Duration::seconds(behind_secs));
        let next = plan_next(id(raw), previous, now, base);
        let span = time::Duration::try_from(base).unwrap();
        prop_assert!(next >= now + span);
        prop_assert!(next <= now + span * 2);
        prop_assert_eq!(next, plan_next(id(raw), previous, now, base));
    }

    /// A thousand podcasts fill the interval instead of clustering. The
    /// bound is deliberately loose — this asserts that the distribution is
    /// a spread, not that it is uniform.
    #[test]
    fn a_library_spreads_across_the_interval(seed: u64) {
        let mut buckets = [0_u32; 10];
        for n in 0..1000_u128 {
            // Consecutive identifiers, which is the worst case: the
            // generator produces them for podcasts added together.
            let slot = slot_permille(id(u128::from(seed) + n));
            buckets[usize::try_from(slot / 100).unwrap()] += 1;
        }
        for (tenth, count) in buckets.iter().enumerate() {
            prop_assert!(
                (30..=200).contains(count),
                "tenth {tenth} holds {count} of 1000"
            );
        }
    }
}
