//! Turning a file name and a feed title into something comparable
//! (ADR 0025).
//!
//! This is not `uguisu-discovery`'s folding and deliberately so. Search
//! folding is tuned for ranking a query against a catalogue; archive
//! folding needs rules that would make search worse — stripping a leading
//! `2024-01-05 ` or `013 - ` that a downloader put there, unifying the
//! dash and quote characters publishers use interchangeably, and reducing
//! accented text so that `Grüße` and `Gruesse` meet. Sharing one function
//! would mean every change here silently re-ranked search results.

use deunicode::deunicode;
use time::{Date, Month};
use unicode_normalization::UnicodeNormalization;

/// Folds a title to its comparable form.
///
/// Compatibility-normalized, transliterated to ASCII, lowercased, with
/// every run of punctuation or whitespace collapsed to one space. What
/// survives is words and digits, which is what a comparison is about.
#[must_use]
pub fn fold_title(raw: &str) -> String {
    let normalized: String = raw.nfkc().collect();
    let ascii = deunicode(&normalized).to_lowercase();
    let mut out = String::with_capacity(ascii.len());
    let mut pending_space = false;
    for c in ascii.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else {
            pending_space = true;
        }
    }
    out
}

/// The integers appearing in a folded title, in order.
///
/// `part 1` and `part 2` are all but identical to any string metric, so
/// the numbers in a title are compared separately and can veto a match
/// the letters would otherwise have made.
#[must_use]
pub fn numeric_tokens(folded: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in folded.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            current.push(c);
        } else if !current.is_empty() {
            if let Ok(n) = current.parse::<u64>() {
                out.push(n);
            }
            current.clear();
        }
    }
    out
}

/// Strips a leading date from a file name, returning it and the rest.
///
/// Recognises `2024-01-05`, `2024_01_05`, `2024.01.05` and `20240105`,
/// each optionally followed by a separator. Many downloaders prefix the
/// publication date; comparing it as part of the title would penalise
/// exactly the files that carry the most information.
#[must_use]
pub fn strip_leading_date(name: &str) -> (Option<Date>, &str) {
    let bytes = name.as_bytes();
    let digits_at = |i: usize, n: usize| -> Option<u32> {
        let slice = name.get(i..i + n)?;
        slice.bytes().all(|b| b.is_ascii_digit()).then_some(())?;
        slice.parse::<u32>().ok()
    };
    let sep_at = |i: usize| -> bool { matches!(bytes.get(i), Some(b'-' | b'_' | b'.')) };

    let (date, rest_at) = if bytes.len() >= 10 && sep_at(4) && sep_at(7) {
        (
            digits_at(0, 4).zip(digits_at(5, 2)).zip(digits_at(8, 2)),
            10,
        )
    } else if bytes.len() >= 8 {
        (digits_at(0, 4).zip(digits_at(4, 2)).zip(digits_at(6, 2)), 8)
    } else {
        (None, 0)
    };
    let Some(((year, month), day)) = date else {
        return (None, name);
    };
    let Some(parsed) = Month::try_from(u8::try_from(month).unwrap_or(0))
        .ok()
        .and_then(|m| {
            Date::from_calendar_date(i32::try_from(year).unwrap_or(0), m, u8::try_from(day).ok()?)
                .ok()
        })
    else {
        return (None, name);
    };
    let rest = name
        .get(rest_at..)
        .unwrap_or("")
        .trim_start_matches([' ', '-', '_', '.'])
        .trim();
    (Some(parsed), rest)
}

/// Strips a leading track number, returning it and the rest.
///
/// Recognises `013 - `, `13. `, `#13 ` and `13_`. A bare number with no
/// separator is left alone: `2020 in review` is a title, not track 2020.
#[must_use]
pub fn strip_track_prefix(name: &str) -> (Option<u32>, &str) {
    let trimmed = name.trim_start();
    let body = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let digits: String = body.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 5 {
        return (None, name);
    }
    let rest = body.get(digits.len()..).unwrap_or("");
    let separated = rest
        .strip_prefix(" - ")
        .or_else(|| rest.strip_prefix(". "))
        .or_else(|| rest.strip_prefix(" -"))
        .or_else(|| rest.strip_prefix('_'))
        .or_else(|| rest.strip_prefix(" | "))
        .or_else(|| trimmed.starts_with('#').then(|| rest.trim_start()));
    match (digits.parse::<u32>(), separated) {
        (Ok(n), Some(rest)) if !rest.trim().is_empty() => (Some(n), rest.trim()),
        _ => (None, name),
    }
}

/// Reads a season and episode marker such as `S01E02`, `s1e2` or `1x02`.
#[must_use]
pub fn season_episode(text: &str) -> (Option<u32>, Option<u32>) {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let read_number = |from: usize| -> (Option<u32>, usize) {
        let digits: String = lower
            .get(from..)
            .unwrap_or("")
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let len = digits.len();
        (digits.parse::<u32>().ok(), from + len)
    };
    for (i, window) in bytes.iter().enumerate() {
        if *window == b's' {
            let (season, after) = read_number(i + 1);
            if season.is_some() && bytes.get(after) == Some(&b'e') {
                let (episode, _) = read_number(after + 1);
                if episode.is_some() {
                    return (season, episode);
                }
            }
        }
        if *window == b'x' && i > 0 {
            let start = lower[..i]
                .rfind(|c: char| !c.is_ascii_digit())
                .map_or(0, |p| p + 1);
            if start < i {
                let season = lower.get(start..i).and_then(|s| s.parse::<u32>().ok());
                let (episode, _) = read_number(i + 1);
                if season.is_some() && episode.is_some() {
                    return (season, episode);
                }
            }
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn folding_makes_one_episode_look_alike() {
        // Punctuation, dash and quote variants, and the whitespace a
        // downloader leaves behind, all fold to one form.
        let target = "folge 12 grusse aus koln";
        for variant in [
            "Folge 12: Grüße aus Köln",
            "Folge 12 — Grüße aus Köln",
            "Folge 12 – Grüße aus Köln",
            "  folge   12 · grüße aus köln  ",
            "Folge 12 \u{2018}Grüße aus Köln\u{2019}",
            "Folge 12 (Grüße aus Köln)",
        ] {
            assert_eq!(fold_title(variant), target, "`{variant}`");
        }
        assert_eq!(fold_title("!!!"), "", "punctuation alone folds to nothing");
    }

    #[test]
    fn transliteration_reduces_without_respelling() {
        // `ö` becomes `o`, which is what makes `Köln` and `Koln` meet.
        assert_eq!(fold_title("Köln"), "koln");
        assert_eq!(fold_title("Koln"), "koln");
        // A German `oe` spelling is a different string and stays one:
        // respelling it would need per-language rules that would damage
        // every other language. Closing that gap is the string metric's
        // job, not folding's - see `matching`.
        assert_ne!(fold_title("Koeln"), fold_title("Köln"));
    }

    #[test]
    fn numbers_stay_apart_from_words() {
        assert_eq!(numeric_tokens(&fold_title("Part 1")), vec![1]);
        assert_eq!(numeric_tokens(&fold_title("Part 2")), vec![2]);
        assert_eq!(numeric_tokens(&fold_title("Episode 10")), vec![10]);
        assert_eq!(numeric_tokens(&fold_title("Episode 100")), vec![100]);
        assert_eq!(numeric_tokens(&fold_title("S01E02 Intro")), vec![1, 2]);
        assert!(numeric_tokens("no digits here").is_empty());
    }

    #[test]
    fn a_date_prefix_is_not_title() {
        let expected = Date::from_calendar_date(2024, Month::January, 5).unwrap();
        for name in [
            "2024-01-05 - Folge 1",
            "2024_01_05 Folge 1",
            "2024.01.05.Folge 1",
            "20240105 Folge 1",
        ] {
            let (date, rest) = strip_leading_date(name);
            assert_eq!(date, Some(expected), "{name}");
            assert_eq!(rest, "Folge 1", "{name}");
        }
        // Not a date: left exactly as it is, rather than half-eaten.
        for name in ["2024-13-05 Folge", "Folge 2024", "12345"] {
            let (date, rest) = strip_leading_date(name);
            assert_eq!(date, None, "{name}");
            assert_eq!(rest, name, "{name}");
        }
    }

    #[test]
    fn a_track_prefix_needs_a_separator() {
        assert_eq!(strip_track_prefix("013 - Folge"), (Some(13), "Folge"));
        assert_eq!(strip_track_prefix("13. Folge"), (Some(13), "Folge"));
        assert_eq!(strip_track_prefix("#13 Folge"), (Some(13), "Folge"));
        assert_eq!(strip_track_prefix("13_Folge"), (Some(13), "Folge"));
        // A number that is part of the title stays in it.
        assert_eq!(
            strip_track_prefix("2020 in review"),
            (None, "2020 in review")
        );
        assert_eq!(strip_track_prefix("13"), (None, "13"));
        assert_eq!(strip_track_prefix("Folge 13"), (None, "Folge 13"));
    }

    #[test]
    fn season_markers_read_every_form() {
        assert_eq!(season_episode("S01E02 - Intro"), (Some(1), Some(2)));
        assert_eq!(season_episode("show s3e12 title"), (Some(3), Some(12)));
        assert_eq!(season_episode("1x02 Intro"), (Some(1), Some(2)));
        assert_eq!(season_episode("Folge 12"), (None, None));
        assert_eq!(season_episode("Sesame Street"), (None, None));
    }
}
