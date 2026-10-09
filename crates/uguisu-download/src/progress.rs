//! Progress accounting: throttled reporting and a smoothed transfer rate.
//!
//! Speed is an exponential moving average (α = 0.3) over samples taken at
//! most once per second, so a burst or a stall moves the number gradually
//! instead of flapping; the ETA divides the remaining bytes by that rate.

use std::time::{Duration, Instant};

use time::OffsetDateTime;
use uguisu_core::download::{ProgressSnapshot, percentage};

/// Smoothing factor of the rate average.
const ALPHA: f64 = 0.3;
/// Minimum spacing of rate samples.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// A report is due after the interval when at least this many bytes
/// arrived since the last report …
const MIN_DELTA_BYTES: u64 = 1024 * 1024;
/// … or at least this much of the total.
const MIN_DELTA_PERCENT: f64 = 1.0;

/// Tracks bytes over time for one attempt.
#[derive(Debug, Clone)]
pub struct ProgressMeter {
    interval: Duration,
    total: Option<u64>,
    bytes: u64,
    last_report_at: Instant,
    last_report_bytes: u64,
    sample_at: Instant,
    sample_bytes: u64,
    ewma_bps: Option<f64>,
    attempt_start: Instant,
    attempt_start_bytes: u64,
}

impl ProgressMeter {
    /// Starts at `bytes` already on disk (the resume offset).
    #[must_use]
    pub fn new(now: Instant, bytes: u64, total: Option<u64>, interval: Duration) -> Self {
        Self {
            interval,
            total,
            bytes,
            last_report_at: now,
            last_report_bytes: bytes,
            sample_at: now,
            sample_bytes: bytes,
            ewma_bps: None,
            attempt_start: now,
            attempt_start_bytes: bytes,
        }
    }

    /// Sets the total once the response declared it.
    pub const fn set_total(&mut self, total: Option<u64>) {
        self.total = total;
    }

    /// Bytes on disk.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Complete length when known.
    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }

    /// Records that the file now holds `bytes` (absolute) and updates the
    /// rate sample when a second has passed.
    #[allow(clippy::cast_precision_loss)]
    pub fn record(&mut self, now: Instant, bytes: u64) {
        self.bytes = bytes;
        let elapsed = now.saturating_duration_since(self.sample_at);
        if elapsed >= SAMPLE_INTERVAL {
            let delta = bytes.saturating_sub(self.sample_bytes) as f64;
            let rate = delta / elapsed.as_secs_f64();
            self.ewma_bps = Some(match self.ewma_bps {
                None => rate,
                Some(prev) => ALPHA * rate + (1.0 - ALPHA) * prev,
            });
            self.sample_at = now;
            self.sample_bytes = bytes;
        }
    }

    /// Whether a progress record/event is due: the interval passed and the
    /// delta since the last report is meaningful (1 MiB or 1 %).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn should_report(&self, now: Instant) -> bool {
        if now.saturating_duration_since(self.last_report_at) < self.interval {
            return false;
        }
        let delta = self.bytes.saturating_sub(self.last_report_bytes);
        if delta == 0 {
            return false;
        }
        if delta >= MIN_DELTA_BYTES {
            return true;
        }
        match self.total {
            Some(t) if t > 0 => (delta as f64) * 100.0 / (t as f64) >= MIN_DELTA_PERCENT,
            _ => false,
        }
    }

    /// Marks a report as sent.
    pub fn reported(&mut self, now: Instant) {
        self.last_report_at = now;
        self.last_report_bytes = self.bytes;
    }

    /// Smoothed rate in bytes per second (0 before the first sample).
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn speed_bps(&self) -> u64 {
        self.ewma_bps.map_or(0, |r| r.max(0.0) as u64)
    }

    /// Remaining seconds at the smoothed rate, when the total is known and
    /// the rate is positive.
    #[must_use]
    pub fn eta_secs(&self) -> Option<u64> {
        let total = self.total?;
        let rate = self.speed_bps();
        if rate == 0 {
            return None;
        }
        Some(total.saturating_sub(self.bytes).div_ceil(rate))
    }

    /// Average rate over the attempt so far, for the attempt log.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn average_bps(&self, now: Instant) -> Option<u64> {
        let secs = now
            .saturating_duration_since(self.attempt_start)
            .as_secs_f64();
        if secs <= 0.0 {
            return None;
        }
        Some(((self.bytes.saturating_sub(self.attempt_start_bytes) as f64) / secs) as u64)
    }

    /// Bytes received during this attempt.
    #[must_use]
    pub const fn attempt_bytes(&self) -> u64 {
        self.bytes.saturating_sub(self.attempt_start_bytes)
    }

    /// The current numbers.
    #[must_use]
    pub fn snapshot(&self, at: OffsetDateTime) -> ProgressSnapshot {
        ProgressSnapshot {
            bytes_downloaded: self.bytes,
            total_bytes: self.total,
            percentage: percentage(self.bytes, self.total),
            speed_bps: self.speed_bps(),
            eta_secs: self.eta_secs(),
            at,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn rate_is_smoothed_and_eta_follows() {
        let t0 = Instant::now();
        let mut m = ProgressMeter::new(t0, 0, Some(10_000_000), Duration::from_secs(1));
        assert_eq!(m.speed_bps(), 0);
        assert_eq!(m.eta_secs(), None);
        // 1 MB/s for one second.
        m.record(t0 + Duration::from_secs(1), 1_000_000);
        assert_eq!(m.speed_bps(), 1_000_000);
        assert_eq!(m.eta_secs(), Some(9));
        // A burst of 10 MB/s in the next second only moves the average by α.
        m.record(t0 + Duration::from_secs(2), 11_000_000);
        let s = m.speed_bps();
        assert!((3_600_000..=3_800_000).contains(&s), "{s}");
        // Sub-second updates do not resample.
        m.record(t0 + Duration::from_millis(2500), 11_100_000);
        assert_eq!(m.speed_bps(), s);
        assert_eq!(m.bytes(), 11_100_000);
        m.set_total(Some(11_100_000));
        assert_eq!(m.eta_secs(), Some(0));
        assert_eq!(m.average_bps(t0 + Duration::from_secs(2)), Some(5_550_000));
    }

    #[test]
    fn reports_are_throttled_by_time_and_delta() {
        let t0 = Instant::now();
        let mut m = ProgressMeter::new(t0, 0, Some(100 * 1024 * 1024), Duration::from_secs(1));
        m.record(t0 + Duration::from_millis(500), 5 * 1024 * 1024);
        assert!(
            !m.should_report(t0 + Duration::from_millis(500)),
            "interval not elapsed"
        );
        assert!(m.should_report(t0 + Duration::from_secs(1)));
        m.reported(t0 + Duration::from_secs(1));
        m.record(t0 + Duration::from_secs(3), 5 * 1024 * 1024 + 100);
        assert!(
            !m.should_report(t0 + Duration::from_secs(3)),
            "100 bytes is not meaningful"
        );
        m.record(t0 + Duration::from_secs(4), 6 * 1024 * 1024 + 100);
        assert!(m.should_report(t0 + Duration::from_secs(4)), "≥ 1 MiB");
        // Small file: 1 % of the total is enough.
        let mut small = ProgressMeter::new(t0, 0, Some(10_000), Duration::from_secs(1));
        small.record(t0 + Duration::from_secs(2), 150);
        assert!(small.should_report(t0 + Duration::from_secs(2)));
        // Unknown total and tiny delta: no report.
        let mut unknown = ProgressMeter::new(t0, 0, None, Duration::from_secs(1));
        unknown.record(t0 + Duration::from_secs(2), 150);
        assert!(!unknown.should_report(t0 + Duration::from_secs(2)));
        let snap = unknown.snapshot(OffsetDateTime::UNIX_EPOCH);
        assert_eq!(snap.percentage, None);
        assert_eq!(snap.bytes_downloaded, 150);
    }

    proptest! {
        /// Whatever the chunk sizes and timing, the meter accounts for
        /// exactly the bytes it was fed: the counter equals the resume
        /// offset plus every chunk, the attempt's own share excludes the
        /// resumed prefix, and the derived numbers stay consistent with it.
        #[test]
        fn accounting_matches_the_chunks_it_was_fed(
            offset in 0u64..5_000_000,
            chunks in proptest::collection::vec(1u64..1_000_000, 1..60),
            gaps_ms in proptest::collection::vec(0u64..3_000, 1..60),
            total_extra in 0u64..5_000_000,
        ) {
            let t0 = Instant::now();
            let sum: u64 = chunks.iter().sum();
            let total = offset + sum + total_extra;
            let mut m = ProgressMeter::new(t0, offset, Some(total), Duration::from_secs(1));
            let mut at = t0;
            let mut written = offset;
            for (i, chunk) in chunks.iter().enumerate() {
                at += Duration::from_millis(gaps_ms[i % gaps_ms.len()]);
                written += chunk;
                m.record(at, written);
                // The counter is always what was written, never more.
                prop_assert_eq!(m.bytes(), written);
                prop_assert_eq!(m.attempt_bytes(), written - offset);
                prop_assert!(m.bytes() <= total);
            }
            prop_assert_eq!(m.bytes(), offset + sum);
            prop_assert_eq!(m.attempt_bytes(), sum);

            let snap = m.snapshot(OffsetDateTime::UNIX_EPOCH);
            prop_assert_eq!(snap.bytes_downloaded, offset + sum);
            prop_assert_eq!(snap.total_bytes, Some(total));
            prop_assert_eq!(snap.speed_bps, m.speed_bps());
            // Percentage and ETA follow the counter, never run away.
            let pct = snap.percentage.unwrap_or(0.0);
            prop_assert!((0.0..=100.0).contains(&pct), "{pct}");
            if let Some(eta) = snap.eta_secs {
                let rate = m.speed_bps();
                prop_assert!(rate > 0);
                prop_assert_eq!(eta, (total - (offset + sum)).div_ceil(rate));
            }
            // A report is never due before the interval has passed.
            prop_assert!(!m.should_report(t0));
            // Once it is due and sent, nothing is due again without new bytes,
            // however much time passes.
            let later = at + Duration::from_secs(2);
            if m.should_report(later) {
                m.reported(later);
                prop_assert!(!m.should_report(later));
                prop_assert!(!m.should_report(later + Duration::from_secs(60)));
            }
        }

        /// A finished transfer reports 100 % and no remaining time.
        #[test]
        fn a_complete_transfer_reports_full(total in 1u64..10_000_000) {
            let t0 = Instant::now();
            let mut m = ProgressMeter::new(t0, 0, Some(total), Duration::from_secs(1));
            m.record(t0 + Duration::from_secs(1), total);
            let snap = m.snapshot(OffsetDateTime::UNIX_EPOCH);
            prop_assert_eq!(snap.bytes_downloaded, total);
            prop_assert_eq!(snap.percentage, Some(100.0));
            prop_assert_eq!(snap.eta_secs, Some(0));
        }
    }
}
