//! Query normalization and tokenization.
//!
//! The same folding is applied to queries and to candidate titles/authors
//! so that fuzzy matching compares like with like.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use url::Url;

/// Whether the input is a free-text term or a URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "url")]
pub enum QueryKind {
    /// Free text to search for.
    Term,
    /// A URL (feed, website or directory page) to resolve instead of searching.
    Url(Url),
}

/// A normalized search input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NormalizedQuery {
    /// Input as typed (trimmed).
    pub raw: String,
    /// NFKC, case-folded, punctuation removed, whitespace collapsed; Unicode kept.
    pub folded: String,
    /// `folded` with diacritics and non-Latin scripts transliterated to ASCII.
    pub ascii: String,
    /// Tokens of `ascii`, in order, duplicates kept.
    pub tokens: Vec<String>,
    /// Term or URL.
    pub kind: QueryKind,
}

impl NormalizedQuery {
    /// Parses and normalizes user input.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        let kind = detect_url(raw).map_or(QueryKind::Term, QueryKind::Url);
        let folded = fold_text(raw);
        let ascii = ascii_fold(&folded);
        let tokens = tokenize(&ascii);
        Self {
            raw: raw.to_owned(),
            folded,
            ascii,
            tokens,
            kind,
        }
    }

    /// Whether the input is a URL.
    pub const fn is_url(&self) -> bool {
        matches!(self.kind, QueryKind::Url(_))
    }

    /// The URL, when the input is one.
    pub const fn url(&self) -> Option<&Url> {
        match &self.kind {
            QueryKind::Url(u) => Some(u),
            QueryKind::Term => None,
        }
    }

    /// Whether there is anything to search for.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && !self.is_url()
    }

    /// Distinct tokens.
    pub fn token_set(&self) -> BTreeSet<&str> {
        self.tokens.iter().map(String::as_str).collect()
    }

    /// Tokens sorted and joined, for order-insensitive comparison.
    pub fn sorted_tokens(&self) -> String {
        let mut t: Vec<&str> = self.token_set().into_iter().collect();
        t.sort_unstable();
        t.join(" ")
    }

    /// At most two looser forms of a term query, for a search that found
    /// nothing (ADR 0005): without its last word and without its first word,
    /// or a single word of four or more characters without its last one.
    /// Empty for a URL.
    pub fn relaxed(&self) -> Vec<Self> {
        if self.is_url() {
            return Vec::new();
        }
        // `folded` has no punctuation, so no variant can parse as a URL.
        let words: Vec<&str> = self.folded.split_whitespace().collect();
        let mut variants: Vec<String> = match words.as_slice() {
            [word] if word.chars().count() >= 4 => {
                let mut chars = word.chars();
                chars.next_back();
                vec![chars.as_str().to_owned()]
            }
            [] | [_] => Vec::new(),
            _ => vec![words[..words.len() - 1].join(" "), words[1..].join(" ")],
        };
        variants.dedup();
        variants
            .iter()
            .map(|v| Self::parse(v))
            .filter(|q| !q.is_empty())
            .collect()
    }
}

/// NFKC-normalizes, lowercases, turns punctuation into spaces and collapses
/// whitespace. Letters of every script are kept.
pub fn fold_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.nfkc() {
        let mapped: Vec<char> = if c.is_alphanumeric() {
            // `İ` lowercases to `i` plus a combining dot, which is not
            // alphanumeric and would become a space on the next fold.
            c.to_lowercase().filter(|m| m.is_alphanumeric()).collect()
        } else if c == '\'' || c == '\u{2019}' || c == '\u{2018}' {
            // Apostrophes join words: "rhysider's" → "rhysiders"
            continue;
        } else {
            vec![' ']
        };
        for m in mapped {
            if m == ' ' {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            } else {
                out.push(m);
                last_space = false;
            }
        }
    }
    // Dropping an apostrophe can make two combining marks adjacent, and the
    // next normalization would reorder them canonically. Normalize once more
    // so folding twice equals folding once (property-tested).
    out.trim().nfkc().collect()
}

/// Transliterates to ASCII (`ü` → `u`, `ß` → `ss`, Cyrillic → Latin) and
/// re-folds so tokens are stable.
pub fn ascii_fold(folded: &str) -> String {
    let t = deunicode::deunicode(folded);
    fold_text(&t)
}

/// Splits folded text into tokens.
pub fn tokenize(folded: &str) -> Vec<String> {
    folded.split_whitespace().map(str::to_owned).collect()
}

fn detect_url(raw: &str) -> Option<Url> {
    if raw.is_empty() || raw.contains(char::is_whitespace) {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let candidate = if lower.starts_with("http://") || lower.starts_with("https://") {
        raw.to_owned()
    } else if let Some(rest) = lower.strip_prefix("feed://") {
        format!("https://{rest}")
    } else if lower.starts_with("feed:") {
        raw[5..].to_owned()
    } else if looks_like_bare_domain(&lower) {
        format!("https://{raw}")
    } else {
        return None;
    };
    let url = Url::parse(&candidate).ok()?;
    let host = url.host_str()?;
    (host.contains('.') || host.eq_ignore_ascii_case("localhost")).then_some(url)
}

fn looks_like_bare_domain(s: &str) -> bool {
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next().unwrap_or("");
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let tld = labels[labels.len() - 1];
    tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn folds_case_punctuation_and_unicode() {
        assert_eq!(fold_text("  Darknet   Diaries! "), "darknet diaries");
        assert_eq!(
            fold_text("Jack Rhysider's Darknet‑Diaries"),
            "jack rhysiders darknet diaries"
        );
        assert_eq!(
            fold_text("Ｄａｒｋｎｅｔ"),
            "darknet",
            "NFKC folds fullwidth"
        );
        assert_eq!(fold_text("GEMISCHTES HACK"), "gemischtes hack");
        assert_eq!(fold_text("Café Ünïcode"), "café ünïcode");
        assert_eq!(ascii_fold("café ünïcode straße"), "cafe unicode strasse");
        assert_eq!(ascii_fold("привет мир"), "privet mir");
    }

    #[test]
    fn tokenizes_and_sorts() {
        let q = NormalizedQuery::parse("Rhysider darknet, darknet");
        assert_eq!(q.tokens, vec!["rhysider", "darknet", "darknet"]);
        assert_eq!(q.sorted_tokens(), "darknet rhysider");
        assert_eq!(q.kind, QueryKind::Term);
        assert!(!q.is_empty());
        assert!(NormalizedQuery::parse("  ...  ").is_empty());
    }

    #[test]
    fn detects_urls() {
        for (input, expected) in [
            (
                "https://example.com/feed.xml",
                "https://example.com/feed.xml",
            ),
            ("HTTP://Example.com/", "http://example.com/"),
            ("feed://example.com/rss", "https://example.com/rss"),
            ("feed:https://example.com/rss", "https://example.com/rss"),
            ("example.com/podcast", "https://example.com/podcast"),
            ("www.example.co.uk", "https://www.example.co.uk/"),
            (
                "podcasts.apple.com/us/podcast/x/id123",
                "https://podcasts.apple.com/us/podcast/x/id123",
            ),
        ] {
            let q = NormalizedQuery::parse(input);
            assert_eq!(q.url().map(Url::as_str), Some(expected), "{input}");
        }
        for input in [
            "darknet diaries",
            "the daily",
            "1.5 degrees",
            "e.g. this",
            "example",
            "darknet.diaries podcast",
        ] {
            assert!(
                !NormalizedQuery::parse(input).is_url(),
                "{input} must be a term"
            );
        }
    }

    #[test]
    fn relaxes_into_looser_terms() {
        let raws = |q: &str| -> Vec<String> {
            NormalizedQuery::parse(q)
                .relaxed()
                .into_iter()
                .map(|r| r.raw)
                .collect()
        };
        assert_eq!(raws("Darknet Diariez"), ["darknet", "diariez"]);
        assert_eq!(
            raws("the darknet diariez"),
            ["the darknet", "darknet diariez"]
        );
        assert_eq!(raws("Café-Ünïcode"), ["café", "ünïcode"]);
        assert_eq!(raws("darknet darknet"), ["darknet"]);
        assert_eq!(raws("darknett"), ["darknet"]);
        assert_eq!(raws("Straße"), ["straß"]);
        assert!(raws("abc").is_empty());
        assert!(raws("   ").is_empty());
        assert!(raws("https://example.com/feed").is_empty());
    }

    #[test]
    fn normalization_is_idempotent() {
        for s in ["Darknet Diaries", "Café Ünïcode!", "  a  b  ", "ＡＢＣ"] {
            let once = fold_text(s);
            assert_eq!(fold_text(&once), once);
            let a = ascii_fold(&once);
            assert_eq!(ascii_fold(&a), a);
        }
    }
}
