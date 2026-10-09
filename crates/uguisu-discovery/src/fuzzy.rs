//! Fuzzy text similarity used by ranking and deduplication.
//!
//! All functions expect text that went through [`crate::query::fold_text`]
//! and [`crate::query::ascii_fold`] so that case, punctuation, diacritics
//! and whitespace differences are already gone.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // scores are f32 by design

use std::collections::BTreeSet;

use crate::query::{ascii_fold, fold_text};

/// Folds arbitrary text the same way queries are folded.
pub fn fold_for_match(s: &str) -> String {
    ascii_fold(&fold_text(s))
}

/// Jaro-Winkler similarity in 0..1 (typo tolerant, favours common prefixes).
pub fn jaro_winkler(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    strsim::jaro_winkler(a, b) as f32
}

/// Normalized Levenshtein similarity in 0..1.
pub fn ratio(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    strsim::normalized_levenshtein(a, b) as f32
}

fn token_set(s: &str) -> BTreeSet<&str> {
    s.split_whitespace().collect()
}

fn join(tokens: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    tokens
        .into_iter()
        .map(|t| t.as_ref().to_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Token-sort ratio: order-insensitive Levenshtein similarity.
pub fn token_sort_ratio(a: &str, b: &str) -> f32 {
    ratio(&join(token_set(a)), &join(token_set(b)))
}

/// Token-set ratio (fuzzywuzzy semantics): the shared tokens form a base
/// string that is compared against each side's full sorted token string, so
/// a query that is a subset of the title ("darknet" vs "darknet diaries")
/// scores 1.0 while extra or misspelled words lower the score gradually.
pub fn token_set_ratio(a: &str, b: &str) -> f32 {
    let ta = token_set(a);
    let tb = token_set(b);
    if ta.is_empty() || tb.is_empty() {
        return if ta.is_empty() && tb.is_empty() {
            1.0
        } else {
            0.0
        };
    }
    let shared: Vec<&str> = ta.intersection(&tb).copied().collect();
    let only_a: Vec<&str> = ta.difference(&tb).copied().collect();
    let only_b: Vec<&str> = tb.difference(&ta).copied().collect();
    let t0 = join(&shared);
    let t1 = join(shared.iter().chain(only_a.iter()));
    let t2 = join(shared.iter().chain(only_b.iter()));
    if !shared.is_empty() && (only_a.is_empty() || only_b.is_empty()) {
        return 1.0;
    }
    ratio(&t0, &t1).max(ratio(&t0, &t2)).max(ratio(&t1, &t2))
}

/// Best similarity between each query token and any candidate token,
/// averaged over the query tokens ("darknet diariez" vs "darknet diaries"
/// → high because both tokens have a close counterpart).
pub fn token_coverage(query: &str, candidate: &str) -> f32 {
    let q: Vec<&str> = query.split_whitespace().collect();
    if q.is_empty() {
        return 0.0;
    }
    let c: Vec<&str> = candidate.split_whitespace().collect();
    let total: f32 = q
        .iter()
        .map(|qt| {
            c.iter()
                .map(|ct| jaro_winkler(qt, ct))
                .fold(0.0_f32, f32::max)
        })
        .sum();
    total / q.len() as f32
}

/// Combined title similarity used for deduplication: the best of the
/// order-insensitive and typo-tolerant measures.
pub fn title_similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    token_sort_ratio(a, b).max(jaro_winkler(a, b))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    fn f(s: &str) -> String {
        fold_for_match(s)
    }

    #[test]
    fn token_set_ratio_handles_subsets_and_typos() {
        assert_eq!(token_set_ratio(&f("darknet"), &f("Darknet Diaries")), 1.0);
        assert_eq!(
            token_set_ratio(&f("dark net diaries"), &f("Darknet Diaries")),
            token_set_ratio("dark net diaries", "darknet diaries")
        );
        assert!(token_set_ratio(&f("darknet diariez"), &f("Darknet Diaries")) > 0.9);
        assert!(token_set_ratio(&f("darknet diaries"), &f("Diary of a CEO")) < 0.6);
        assert_eq!(token_set_ratio("", ""), 1.0);
        assert_eq!(token_set_ratio("a", ""), 0.0);
    }

    #[test]
    fn coverage_and_jaro_winkler_tolerate_misspellings() {
        assert!(token_coverage(&f("darknet diariez"), &f("Darknet Diaries")) > 0.95);
        assert!(token_coverage(&f("rhysider darknet"), &f("Darknet Diaries")) < 0.8);
        assert!(jaro_winkler("darknet diary", "darknet diaries") > 0.9);
        assert!(jaro_winkler("darknet", "the daily") < 0.7);
    }

    #[test]
    fn title_similarity_symmetric_and_bounded() {
        for (a, b) in [
            ("darknet diaries", "diaries darknet"),
            ("darknet diaries", "darknet diary"),
            ("the daily", "daily the"),
        ] {
            let s1 = title_similarity(a, b);
            let s2 = title_similarity(b, a);
            assert!((s1 - s2).abs() < 1e-6);
            assert!((0.0..=1.0).contains(&s1));
        }
        assert_eq!(title_similarity("x", "x"), 1.0);
    }
}
