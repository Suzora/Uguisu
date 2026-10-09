//! When a podcast is refreshed next (ADR 0027).
//!
//! The arithmetic has no clock of its own and no randomness: the same
//! library, refreshed on the same day, produces the same schedule on every
//! machine. That is what makes a scheduling bug reproducible instead of
//! anecdotal.
//!
//! The problem this solves is the herd. A hundred podcasts added in one
//! sitting, all on an hourly interval, would be fetched in the same second
//! for as long as the library exists — a burst against a handful of CDNs,
//! a burst through one SQLite writer, and a load profile that looks like
//! an outage from the outside. Spreading them needs no randomness, only a
//! number that differs per podcast and never changes: the identifier.
//!
//! [`SchedulerControl`] sits here too, as [`crate::download::DownloadControl`]
//! sits beside the download vocabulary: it is what the scheduler *is*, not
//! how it counts.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::PodcastId;

/// Whether automatic refreshing is running, and since when it is not.
///
/// Deliberately not the download queue's control row: a full disk pauses
/// transfers by itself, and an operator who stops transfers is not asking
/// the library to go stale.
///
/// `paused_reason` is free text rather than an enum, unlike the download
/// queue's fixed vocabulary: the queue pauses itself for reasons the
/// engine knows, while this is paused by a person, whose reason is theirs
/// to write. `scheduler.paused` carries the same string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SchedulerControl {
    /// Whether the scheduler starts refreshes.
    pub paused: bool,
    /// Why, when somebody said.
    pub paused_reason: Option<String>,
    /// Since when.
    #[serde(with = "time::serde::rfc3339::option")]
    pub paused_at: Option<OffsetDateTime>,
    /// When the maintenance pass last ran. This is what lets a restart
    /// work out when the next one is due instead of running it on boot.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_maintenance_at: Option<OffsetDateTime>,
    /// Last change to this row.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Widest one-sided spread added to an ordinary interval, in per mille.
///
/// One-sided, 0…+10 % (ADR 0027): subtracting could schedule a fetch
/// *earlier* than the origin's `Cache-Control: max-age` allows, and a spread
/// must not quietly undo a header Uguisu honours on purpose.
pub const SPREAD_PERMILLE: u64 = 100;

/// Widest spread for a podcast that is already overdue, in per mille.
///
/// A row that is overdue by more than one interval — a restart after a
/// week off, or a podcast that has never been fetched — is rescheduled
/// across a whole interval rather than a tenth of one. A week of downtime
/// otherwise drains into a six-minute window and re-synchronises the
/// library into exactly the herd the spread exists to prevent.
pub const CATCH_UP_PERMILLE: u64 = 1000;

/// The podcast's fixed position in the interval, in per mille (0…999).
///
/// Derived from the ULID's random tail, not from the whole identifier:
/// the leading 48 bits are a millisecond timestamp, so podcasts added in
/// one sitting would otherwise share a slot — the same reasoning ADR 0022
/// applies to path suffixes.
///
/// The tail is mixed rather than taken modulo directly, because
/// [`crate::ids`] generates monotonically within a millisecond: two
/// podcasts added in the same millisecond get tails that differ by one,
/// and a plain `% 1000` would put them in adjacent slots. The mixer is
/// SplitMix64's finalizer — a few constants, no dependency, and
/// avalanching enough that consecutive inputs land far apart.
#[must_use]
pub fn slot_permille(id: PodcastId) -> u64 {
    let tail = id.ulid().random();
    // The tail is 80 bits: its low half and its high half both fit a u64
    // by construction, so neither conversion can fail. Folding them
    // together keeps the whole tail in play.
    let low = u64::try_from(tail & u128::from(u64::MAX)).unwrap_or_default();
    let high = u64::try_from(tail >> 64).unwrap_or_default();
    mix(low ^ high) % 1000
}

/// SplitMix64's finalizer.
const fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The podcast's share of an ordinary interval: 0…10 % of `base`.
#[must_use]
pub fn spread(id: PodcastId, base: Duration) -> Duration {
    fraction(base, slot_permille(id) * SPREAD_PERMILLE, 1_000_000)
}

/// The podcast's share of a whole interval, for a podcast that is behind:
/// 0…100 % of `base`.
#[must_use]
pub fn catch_up_spread(id: PodcastId, base: Duration) -> Duration {
    fraction(base, slot_permille(id) * CATCH_UP_PERMILLE, 1_000_000)
}

/// `base * numerator / denominator`, saturating rather than panicking.
fn fraction(base: Duration, numerator: u64, denominator: u64) -> Duration {
    let nanos = base.as_nanos() * u128::from(numerator) / u128::from(denominator);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// Whether the podcast is behind rather than merely due.
///
/// "Never fetched" counts as behind, which is also how the due query
/// orders it: a `NULL` next-refresh time sorts first.
#[must_use]
pub fn is_catching_up(
    previous_due: Option<OffsetDateTime>,
    now: OffsetDateTime,
    base: Duration,
) -> bool {
    let Some(previous) = previous_due else {
        return true;
    };
    let Ok(base) = time::Duration::try_from(base) else {
        return false;
    };
    now - previous > base
}

/// When this podcast should next be refreshed.
///
/// `now + base + spread`, where the spread is the podcast's own share of
/// the interval — a tenth of it normally, a whole one when the podcast is
/// catching up. Called at the three places a refresh writes
/// `next_refresh_at` and nowhere else: the scheduler never rewrites due
/// times, so a restart costs no writes at all and a hand-set time keeps
/// meaning what it says.
#[must_use]
pub fn plan_next(
    id: PodcastId,
    previous_due: Option<OffsetDateTime>,
    now: OffsetDateTime,
    base: Duration,
) -> OffsetDateTime {
    let extra = if is_catching_up(previous_due, now, base) {
        catch_up_spread(id, base)
    } else {
        spread(id, base)
    };
    add(now, base.saturating_add(extra))
}

/// When a podcast that has just been put back in the schedule should be
/// looked at.
///
/// Inside the next interval rather than after it: somebody who resumes a
/// podcast means "carry on with this one", not "wait an hour first".
/// Spread all the same, because resuming two hundred podcasts at once is
/// exactly the herd the rest of this module exists to prevent.
#[must_use]
pub fn resume_at(id: PodcastId, now: OffsetDateTime, base: Duration) -> OffsetDateTime {
    add(now, catch_up_spread(id, base))
}

/// `at + delta`, clamped to the representable range instead of panicking.
///
/// Public because the maintenance tick needs the same arithmetic on a
/// plain interval, and a scheduler that panics on a configured value is
/// worse than one that schedules the end of time.
#[must_use]
pub fn after(at: OffsetDateTime, delta: Duration) -> OffsetDateTime {
    add(at, delta)
}

fn add(at: OffsetDateTime, delta: Duration) -> OffsetDateTime {
    time::Duration::try_from(delta)
        .ok()
        .and_then(|d| at.checked_add(d))
        .unwrap_or(OffsetDateTime::new_utc(
            time::Date::MAX,
            time::Time::MIDNIGHT,
        ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn at(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
    }

    #[test]
    fn slots_are_stable_and_spread() {
        let id = PodcastId::new();
        assert_eq!(slot_permille(id), slot_permille(id));
        let mut deciles = BTreeMap::new();
        for _ in 0..2000 {
            let slot = slot_permille(PodcastId::new());
            assert!(slot < 1000);
            *deciles.entry(slot / 100).or_insert(0_u32) += 1;
        }
        assert_eq!(deciles.len(), 10, "every tenth of the interval is used");
        for (decile, count) in &deciles {
            assert!(
                (60..=400).contains(count),
                "decile {decile} holds {count} of 2000 — that is not a spread"
            );
        }
    }

    #[test]
    fn one_millisecond_shares_no_slot() {
        // The generator increments the random tail within a millisecond, so
        // a plain `tail % 1000` would put a batch import into adjacent
        // slots — the herd this module exists to break up.
        let ids: Vec<PodcastId> = (0..64).map(|_| PodcastId::new()).collect();
        let slots: Vec<u64> = ids.iter().map(|id| slot_permille(*id)).collect();
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert!(
            sorted.len() > 48,
            "64 ids produced only {} distinct slots: {slots:?}",
            sorted.len()
        );
        let adjacent = slots
            .windows(2)
            .filter(|w| w[0].abs_diff(w[1]) <= 1)
            .count();
        assert!(
            adjacent < 8,
            "{adjacent} consecutive ids landed side by side"
        );
    }

    #[test]
    fn a_spread_never_exceeds_a_tenth() {
        for _ in 0..500 {
            let id = PodcastId::new();
            let s = spread(id, HOUR);
            assert!(s <= HOUR / 10, "{s:?}");
            assert!(catch_up_spread(id, HOUR) <= HOUR);
            assert!(catch_up_spread(id, HOUR) >= s);
        }
        assert_eq!(spread(PodcastId::new(), Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn a_planned_time_follows_the_interval() {
        let id = PodcastId::new();
        let now = at("2026-09-21T12:00:00Z");
        let next = plan_next(id, Some(now), now, HOUR);
        assert!(next >= now + time::Duration::HOUR);
        assert!(next <= now + time::Duration::minutes(66));
    }

    #[test]
    fn overdue_widens_the_spread() {
        let now = at("2026-09-21T12:00:00Z");
        // One id whose slot is high enough that the two modes differ.
        let id = (0..10_000)
            .map(|_| PodcastId::new())
            .find(|id| slot_permille(*id) > 200)
            .unwrap();
        let on_time = plan_next(id, Some(now - time::Duration::minutes(1)), now, HOUR);
        let behind = plan_next(id, Some(now - time::Duration::days(7)), now, HOUR);
        let fresh = plan_next(id, None, now, HOUR);
        assert!(behind > on_time);
        assert_eq!(behind, fresh, "never fetched is maximally overdue");
        assert!(is_catching_up(None, now, HOUR));
        assert!(!is_catching_up(Some(now), now, HOUR));
        assert!(!is_catching_up(
            Some(now - time::Duration::minutes(59)),
            now,
            HOUR
        ));
        assert!(is_catching_up(
            Some(now - time::Duration::minutes(61)),
            now,
            HOUR
        ));
    }

    #[test]
    fn resuming_lands_inside_the_interval() {
        let now = at("2026-09-21T12:00:00Z");
        for _ in 0..200 {
            let when = resume_at(PodcastId::new(), now, HOUR);
            assert!(when >= now);
            assert!(when <= now + time::Duration::HOUR);
        }
    }

    #[test]
    fn an_absurd_interval_clamps_instead_of_panicking() {
        let id = PodcastId::new();
        let huge = Duration::from_secs(u64::MAX / 2);
        let _ = spread(id, huge);
        let _ = catch_up_spread(id, huge);
        let next = plan_next(id, None, at("2026-09-21T12:00:00Z"), huge);
        assert!(next.year() >= 9999);
    }
}
