//! Deciding which episode a foreign file is, deterministically (ADR 0025).
//!
//! The rule the whole module is built around: **a wrong import is worse
//! than an unresolved one.** A file placed under the wrong episode is a
//! quiet, permanent error that nobody notices until they play it; a file
//! left unmatched is a line in a report that a user can act on. So there
//! is no "best guess" outcome. A candidate is either matched with room to
//! spare, or it is reported.
//!
//! Scoring is a pure function with no I/O and no randomness, and ties
//! break on a total order, so the same directory produces the same
//! verdicts on every run and on every machine.
//!
//! Signals are weighted and then **renormalized over the ones actually
//! available on both sides**. Without that, a feed that carries no season
//! numbers would cap every score below the threshold and nothing would
//! ever import.

// Scoring is float arithmetic over bounded quantities: every signal is
// clamped to [0, 1] before it becomes a percentage, the token counts are
// the words in one title, and the sizes come from a file system. None of
// these casts can lose anything that changes a verdict, and writing them
// as fallible conversions would bury the weight table in error handling.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use time::Date;
use uguisu_core::ids::EpisodeId;

use super::Candidate;
use super::normalize::{fold_title, numeric_tokens};

/// Weight of the title, in percent of the available total.
pub const W_TITLE: u32 = 40;
/// Weight of the publication date.
pub const W_DATE: u32 = 25;
/// Weight of the season and episode numbers.
pub const W_NUMBER: u32 = 15;
/// Weight of the duration.
pub const W_DURATION: u32 = 12;
/// Weight of the declared size.
pub const W_SIZE: u32 = 8;

/// How far ahead of the runner-up a match must be, in percent.
///
/// Two episodes that both explain a file equally well mean the file does
/// not identify one of them, however high the raw score is.
pub const MARGIN: u32 = 10;

/// Below this, a candidate is not worth reporting as a near miss.
pub const AMBIGUOUS_FLOOR: u32 = 55;

/// Title similarity below this counts as no similarity at all, so a long
/// common suffix cannot drag an unrelated title over the line.
const TITLE_FLOOR: f32 = 0.55;

/// What a disagreement in the numbers costs the title score.
///
/// `part 1` and `part 2` score about 0.97 to any string metric. Comparing
/// the digits separately is what keeps them apart.
const NUMERIC_PENALTY: f32 = 0.6;

/// Days apart before two dates are treated as contradicting each other.
const DATE_TOLERANCE_DAYS: i64 = 3;

/// The highest total a pair with contradicting dates may reach.
///
/// Below every usable threshold, so a date contradiction can always be
/// seen in the report but can never produce a match.
const DATE_VETO_CAP: u32 = 84;

/// What Uguisu knows about one episode, for matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeFacts {
    /// The episode.
    pub id: EpisodeId,
    /// Its title, as the feed gave it.
    pub title: String,
    /// Publication date, when the feed carried a usable one.
    pub published: Option<Date>,
    /// Season number.
    pub season: Option<u32>,
    /// Episode number.
    pub number: Option<u32>,
    /// Duration in seconds.
    pub duration_secs: Option<u32>,
    /// The enclosure's declared length. Often wrong, hence the small
    /// weight.
    pub enclosure_bytes: Option<u64>,
}

/// One signal's contribution, kept so a verdict can explain itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    /// Which signal.
    pub signal: &'static str,
    /// How well it agreed, in percent.
    pub score: u32,
    /// What it was worth.
    pub weight: u32,
}

/// How confident one pairing is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// The episode this is about.
    pub episode_id: EpisodeId,
    /// Total confidence in percent, renormalized over available signals.
    pub confidence: u32,
    /// What went into it.
    pub reasons: Vec<Reason>,
}

/// What an import decided about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// One episode, clearly ahead of every other.
    Matched {
        /// The episode.
        episode_id: EpisodeId,
        /// Confidence in percent.
        confidence: u32,
        /// How it was decided.
        matched_by: MatchedBy,
        /// The runner-up's confidence, when there was one.
        runner_up: Option<u32>,
        /// What went into the decision.
        reasons: Vec<Reason>,
    },
    /// More than one episode explains the file about equally well.
    /// Never imported.
    Ambiguous {
        /// The contenders, best first, at most five.
        top: Vec<Score>,
        /// Why it could not be decided.
        detail: &'static str,
    },
    /// Nothing explains the file well enough.
    Unmatched {
        /// The best score there was, for the report.
        best: Option<Score>,
    },
}

impl Verdict {
    /// The stable word the CLI, the API and the events use.
    #[must_use]
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Matched { .. } => "matched",
            Self::Ambiguous { .. } => "ambiguous",
            Self::Unmatched { .. } => "unmatched",
        }
    }

    /// The episode to import into, if there is one.
    #[must_use]
    pub const fn episode_id(&self) -> Option<EpisodeId> {
        match self {
            Self::Matched { episode_id, .. } => Some(*episode_id),
            _ => None,
        }
    }
}

/// How a match was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    /// The bytes are already known to Uguisu.
    ContentHash,
    /// An Uguisu sidecar beside the file named the episode, and its
    /// identity key agreed.
    Sidecar,
    /// The file's own tags carry an episode GUID that one episode has.
    EmbeddedGuid,
    /// The other tool's own database named the episode: Podgrab's record
    /// of the file carries its GUID or its enclosure.
    SourceDatabase,
    /// The weighted signals.
    Scored,
}

impl MatchedBy {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContentHash => "content_hash",
            Self::Sidecar => "sidecar",
            Self::EmbeddedGuid => "embedded_guid",
            Self::SourceDatabase => "source_database",
            Self::Scored => "scored",
        }
    }
}

fn percent(value: f32) -> u32 {
    // Saturating on both ends: a weight table change must never produce a
    // confidence above 100 or an underflow below 0.
    let scaled = (value * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        scaled as u32
    }
}

/// How alike two titles are, in percent.
///
/// The same comparison the scorer uses, exposed because an import also has
/// to decide which *podcast* a directory belongs to before it can score
/// anything inside it - and using a second, subtly different comparison
/// there would make the two disagree.
#[must_use]
pub fn title_similarity(a: &str, b: &str) -> u32 {
    percent(title_score(a, b))
}

/// Title similarity, with the numbers in the title compared separately.
fn title_score(candidate: &str, episode: &str) -> f32 {
    let a = fold_title(candidate);
    let b = fold_title(episode);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut score = if a == b {
        1.0
    } else {
        let jaro = strsim::jaro_winkler(&a, &b) as f32;
        let tokens = token_set_ratio(&a, &b);
        jaro.max(tokens)
    };
    if score < TITLE_FLOOR {
        return 0.0;
    }
    let (na, nb) = (numeric_tokens(&a), numeric_tokens(&b));
    if !na.is_empty() && !nb.is_empty() && na != nb {
        score *= NUMERIC_PENALTY;
    }
    score
}

/// How much of the smaller word set the two titles share.
fn token_set_ratio(a: &str, b: &str) -> f32 {
    use std::collections::BTreeSet;
    let sa: BTreeSet<&str> = a.split(' ').filter(|t| !t.is_empty()).collect();
    let sb: BTreeSet<&str> = b.split(' ').filter(|t| !t.is_empty()).collect();
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let shared = sa.intersection(&sb).count();
    let smaller = sa.len().min(sb.len());
    shared as f32 / smaller as f32
}

fn date_score(a: Date, b: Date) -> f32 {
    let days = (a.to_julian_day() - b.to_julian_day()).abs();
    match i64::from(days) {
        0 => 1.0,
        // One day is a time zone, not a different episode.
        1 => 0.85,
        2..=DATE_TOLERANCE_DAYS => 0.5,
        _ => 0.0,
    }
}

fn number_score(candidate: &Candidate, facts: &EpisodeFacts) -> Option<f32> {
    match (candidate.number, facts.number) {
        (Some(a), Some(b)) if a == b => Some(match (candidate.season, facts.season) {
            (Some(x), Some(y)) if x == y => 1.0,
            (Some(_), Some(_)) => 0.0,
            // One side simply does not say which season it is.
            _ => 0.8,
        }),
        (Some(_), Some(_)) => Some(0.0),
        _ => None,
    }
}

fn duration_score(a: u32, b: u32) -> f32 {
    // Within 2% or 30 seconds is the same recording: re-encoding and a
    // re-cut advert break move a duration by about that much.
    let tolerance = (b as f32 * 0.02).max(30.0);
    (1.0 - (a.abs_diff(b) as f32 / tolerance).min(1.0)).max(0.0)
}

fn size_score(a: u64, b: u64) -> f32 {
    if a == b {
        return 1.0;
    }
    // `enclosure_bytes` is what the feed declared, which is often stale or
    // simply wrong, so the tolerance is wide and the weight is small.
    let tolerance = b as f32 * 0.05;
    if tolerance <= 0.0 {
        return 0.0;
    }
    (1.0 - (a.abs_diff(b) as f32 / tolerance).min(1.0)).max(0.0)
}

/// Scores one file against one episode.
#[must_use]
pub fn score(candidate: &Candidate, facts: &EpisodeFacts) -> Score {
    let mut reasons = Vec::new();
    let mut weighted = 0.0f32;
    let mut available = 0u32;
    let mut add = |signal: &'static str, weight: u32, value: f32| {
        reasons.push(Reason {
            signal,
            score: percent(value),
            weight,
        });
        weighted += value * weight as f32;
        available += weight;
    };

    if let Some(title) = candidate.title.as_deref() {
        add("title", W_TITLE, title_score(title, &facts.title));
    }
    let mut date_contradicts = false;
    if let (Some(a), Some(b)) = (candidate.published, facts.published) {
        let value = date_score(a, b);
        date_contradicts = value == 0.0;
        add("date", W_DATE, value);
    }
    if let Some(value) = number_score(candidate, facts) {
        add("number", W_NUMBER, value);
    }
    if let (Some(a), Some(b)) = (candidate.duration_secs, facts.duration_secs) {
        add("duration", W_DURATION, duration_score(a, b));
    }
    if let Some(b) = facts.enclosure_bytes {
        add("size", W_SIZE, size_score(candidate.size_bytes, b));
    }

    let confidence = if available == 0 {
        0
    } else {
        percent(weighted / available as f32)
    };
    // Two dates that disagree by more than a time zone are a statement
    // that these are different recordings. It caps the total rather than
    // scoring zero, so the near miss is still visible in the report.
    let confidence = if date_contradicts {
        confidence.min(DATE_VETO_CAP)
    } else {
        confidence
    };
    Score {
        episode_id: facts.id,
        confidence,
        reasons,
    }
}

/// Picks the episode a file belongs to, or refuses to.
///
/// `threshold` is the configured confidence in percent. A match needs both
/// that and [`MARGIN`] over the runner-up; anything else is reported.
///
/// The fallback titles are tried in order only when the title itself does
/// not match clearly, and the first clear match wins; otherwise the title's
/// own verdict stands. So a fallback can rescue a file, never take one
/// away from a match it already had (ADR 0050).
#[must_use]
pub fn classify(candidate: &Candidate, episodes: &[EpisodeFacts], threshold: u32) -> Verdict {
    if let Some(matched) = candidate.exact_match {
        return Verdict::Matched {
            episode_id: matched.0,
            confidence: 100,
            matched_by: matched.1,
            runner_up: None,
            reasons: Vec::new(),
        };
    }
    let verdict = classify_scored(candidate, episodes, threshold);
    if matches!(verdict, Verdict::Matched { .. }) {
        return verdict;
    }
    for title in &candidate.fallback_titles {
        let reading = Candidate {
            title: Some(title.clone()),
            fallback_titles: Vec::new(),
            ..candidate.clone()
        };
        let alternative = classify_scored(&reading, episodes, threshold);
        if matches!(alternative, Verdict::Matched { .. }) {
            return alternative;
        }
    }
    verdict
}

fn classify_scored(candidate: &Candidate, episodes: &[EpisodeFacts], threshold: u32) -> Verdict {
    let mut scores: Vec<Score> = episodes.iter().map(|e| score(candidate, e)).collect();
    // A total order, so shuffling the candidate list cannot change a
    // verdict: confidence first, then the identifier.
    scores.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.episode_id.cmp(&b.episode_id))
    });
    let Some(best) = scores.first().cloned() else {
        return Verdict::Unmatched { best: None };
    };
    let runner_up = scores.get(1).map(|s| s.confidence);

    if best.confidence < AMBIGUOUS_FLOOR {
        return Verdict::Unmatched { best: Some(best) };
    }
    let clear = runner_up.is_none_or(|second| best.confidence.saturating_sub(second) >= MARGIN);
    if best.confidence >= threshold && clear {
        return Verdict::Matched {
            episode_id: best.episode_id,
            confidence: best.confidence,
            matched_by: MatchedBy::Scored,
            runner_up,
            reasons: best.reasons,
        };
    }
    scores.truncate(5);
    Verdict::Ambiguous {
        top: scores,
        detail: if best.confidence >= threshold {
            "two episodes explain the file equally well"
        } else {
            "no episode explains the file well enough"
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::Month;

    use super::*;
    use crate::import::Candidate;

    const THRESHOLD: u32 = 85;

    fn day(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    fn facts(title: &str, date: Option<Date>) -> EpisodeFacts {
        EpisodeFacts {
            id: EpisodeId::new(),
            title: title.to_owned(),
            published: date,
            season: None,
            number: None,
            duration_secs: Some(1800),
            enclosure_bytes: Some(1_000_000),
        }
    }

    fn candidate(title: &str, date: Option<Date>) -> Candidate {
        Candidate {
            title: Some(title.to_owned()),
            published: date,
            duration_secs: Some(1800),
            size_bytes: 1_000_000,
            ..Candidate::empty()
        }
    }

    #[test]
    fn a_named_file_matches_throughout() {
        use crate::import::{GenericFormat, SourceFormat};
        use crate::path::RelativePath;
        use crate::scan::ScannedFile;

        let e = facts("Folge 12: Grüße aus Köln", Some(day(2024, 1, 5)));
        let file = ScannedFile {
            relative: RelativePath::parse("Show/2024-01-05 - Folge 12 Grüße aus Köln.mp3").unwrap(),
            absolute: std::path::PathBuf::new(),
            size_bytes: 1_000_000,
            mtime_unix: None,
            head: b"ID3\x04............".to_vec(),
        };
        let mut c = GenericFormat.observe(&file);
        c.duration_secs = Some(1800);
        assert_eq!(
            c.title.as_deref(),
            Some("Folge 12 Grüße aus Köln"),
            "the downloader's date prefix is not part of the title"
        );
        let v = classify(&c, std::slice::from_ref(&e), THRESHOLD);
        assert_eq!(v.episode_id(), Some(e.id), "{v:?}");
        assert_eq!(v.state(), "matched");
    }

    #[test]
    fn a_respelled_title_still_finds_its_episode() {
        // `Gruesse aus Koeln` for `Grüße aus Köln`. Folding alone does not
        // unify the two - respelling `oe` as `ö` would need per-language
        // rules that damage every other language - but the character
        // metric carries it, which is exactly the job it is there for.
        let e = facts("Folge 12: Grüße aus Köln", Some(day(2024, 1, 5)));
        let c = candidate("Folge 12 Gruesse aus Koeln", Some(day(2024, 1, 5)));
        let v = classify(&c, std::slice::from_ref(&e), THRESHOLD);
        assert_eq!(v.episode_id(), Some(e.id), "{v:?}");
    }

    #[test]
    fn one_differing_word_is_separated() {
        // Character similarity alone rates these about 0.93, which is the
        // known weakness of any string metric. The margin rule is what
        // keeps it honest: when both are candidates, one of them has to
        // be clearly ahead or neither is chosen.
        let mueller = facts("Der Fall Müller", Some(day(2024, 1, 5)));
        let meier = facts("Der Fall Meier", Some(day(2024, 3, 20)));

        let dated = candidate("Der Fall Müller", Some(day(2024, 1, 5)));
        assert_eq!(
            classify(&dated, &[mueller.clone(), meier.clone()], THRESHOLD).episode_id(),
            Some(mueller.id),
            "the date tells them apart"
        );

        let mut blind = dated.clone();
        blind.published = None;
        blind.duration_secs = None;
        let v = classify(&blind, &[mueller, meier], THRESHOLD);
        assert_eq!(
            v.state(),
            "ambiguous",
            "with nothing but the title to go on, neither is chosen: {v:?}"
        );
    }

    #[test]
    fn one_title_two_dates_never_matches() {
        // A rerun published a year later, titled exactly like the original.
        let original = facts("Die Jahresrückschau", Some(day(2023, 12, 30)));
        let rerun = facts("Die Jahresrückschau", Some(day(2024, 12, 30)));
        let c = candidate("Die Jahresrückschau", Some(day(2023, 12, 30)));
        let v = classify(&c, &[original.clone(), rerun.clone()], THRESHOLD);
        assert_eq!(
            v.episode_id(),
            Some(original.id),
            "the date decides between them: {v:?}"
        );

        // With no date on the file, the two are indistinguishable.
        let mut blind = c.clone();
        blind.published = None;
        let v = classify(&blind, &[original, rerun], THRESHOLD);
        assert_eq!(v.state(), "ambiguous", "{v:?}");
        assert_eq!(v.episode_id(), None, "an ambiguous file is never imported");
    }

    #[test]
    fn two_near_identical_titles_stay_apart() {
        // The classic fuzzy-matching failure: `jaro_winkler` rates these
        // about 0.97, and picking either one is a coin toss.
        for (a, b, wanted) in [
            ("Der Fall, Teil 1", "Der Fall, Teil 2", "Teil 1"),
            ("Episode 10", "Episode 100", "Episode 10"),
        ] {
            let first = facts(a, Some(day(2024, 1, 5)));
            let second = facts(b, Some(day(2024, 1, 12)));
            let c = candidate(a, None);
            let v = classify(&c, &[first.clone(), second], THRESHOLD);
            assert_eq!(
                v.episode_id(),
                Some(first.id),
                "`{a}` must not be confused with `{b}` ({wanted}): {v:?}"
            );
        }
    }

    #[test]
    fn a_contradicting_date_blocks_a_match() {
        let e = facts("Folge 12", Some(day(2024, 1, 5)));
        let c = candidate("Folge 12", Some(day(2024, 6, 5)));
        let s = score(&c, &e);
        assert!(
            s.confidence <= DATE_VETO_CAP,
            "everything else agreeing must not override the date: {s:?}"
        );
        assert!(classify(&c, &[e], THRESHOLD).episode_id().is_none());
    }

    #[test]
    fn a_feed_without_numbers_still_matches() {
        // Renormalization: the season and number signal is simply absent
        // here, and must not cap the achievable score.
        let e = EpisodeFacts {
            season: None,
            number: None,
            duration_secs: None,
            enclosure_bytes: None,
            ..facts("Folge 12: Grüße aus Köln", Some(day(2024, 1, 5)))
        };
        let c = Candidate {
            duration_secs: None,
            ..candidate("Folge 12: Grüße aus Köln", Some(day(2024, 1, 5)))
        };
        let s = score(&c, &e);
        assert_eq!(
            s.confidence, 100,
            "only title and date were available, and both agreed: {s:?}"
        );
        assert_eq!(s.reasons.len(), 2);
    }

    #[test]
    fn nothing_similar_is_reported_rather_than_forced() {
        let e = facts("Ein ganz anderes Thema", Some(day(2020, 3, 3)));
        let c = candidate("Folge 12: Grüße aus Köln", Some(day(2024, 1, 5)));
        let v = classify(&c, &[e], THRESHOLD);
        assert_eq!(v.state(), "unmatched", "{v:?}");
        assert!(matches!(v, Verdict::Unmatched { best: Some(_) }));
        assert_eq!(classify(&c, &[], THRESHOLD).state(), "unmatched");
    }

    #[test]
    fn the_verdict_is_order_independent() {
        let all: Vec<EpisodeFacts> = (1..=6)
            .map(|i| {
                facts(
                    &format!("Folge {i}"),
                    Some(day(2024, 1, u8::try_from(i).unwrap())),
                )
            })
            .collect();
        let c = candidate("Folge 4", Some(day(2024, 1, 4)));
        let forward = classify(&c, &all, THRESHOLD);
        let mut reversed = all.clone();
        reversed.reverse();
        assert_eq!(forward, classify(&c, &reversed, THRESHOLD));
        assert_eq!(forward.episode_id(), Some(all[3].id));
    }

    #[test]
    fn a_known_hash_short_circuits_scoring() {
        let e = facts("Etwas völlig anderes", None);
        let mut c = candidate("Nicht einmal ähnlich", None);
        c.exact_match = Some((e.id, MatchedBy::ContentHash));
        let v = classify(&c, std::slice::from_ref(&e), THRESHOLD);
        assert_eq!(
            v,
            Verdict::Matched {
                episode_id: e.id,
                confidence: 100,
                matched_by: MatchedBy::ContentHash,
                runner_up: None,
                reasons: vec![],
            },
            "bytes Uguisu already holds need no scoring"
        );
        assert_eq!(MatchedBy::Sidecar.as_str(), "sidecar");
    }

    #[test]
    fn fallback_title_matches_after_primary() {
        // Podgrab's counter in front of a numbered title: the numbers
        // disagree, so the name alone does not match.
        let wanted = facts("Folge 12: Der Einbruch", None);
        let other = facts("Folge 11: Die Spur", None);
        let mut c = candidate("13-folge-12-der-einbruch", None);
        let episodes = [wanted.clone(), other];
        assert_eq!(classify(&c, &episodes, THRESHOLD).episode_id(), None);
        c.fallback_titles = vec!["unrelated".to_owned(), "folge-12-der-einbruch".to_owned()];
        assert_eq!(
            classify(&c, &episodes, THRESHOLD).episode_id(),
            Some(wanted.id)
        );
    }

    #[test]
    fn primary_match_ignores_fallbacks() {
        // The fallback alone would fit both episodes equally well.
        let wanted = facts("Folge 13: Der Einbruch", Some(day(2024, 1, 5)));
        let other = facts("Folge 14: Der Einbruch", Some(day(2024, 1, 5)));
        let mut c = candidate("folge-13-der-einbruch", Some(day(2024, 1, 5)));
        c.fallback_titles = vec!["der-einbruch".to_owned()];
        assert_eq!(
            classify(&c, &[wanted.clone(), other], THRESHOLD).episode_id(),
            Some(wanted.id),
            "the title's own clear match stands"
        );
    }
}
