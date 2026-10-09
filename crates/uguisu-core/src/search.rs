//! Local search over the library (ADR 0029).
//!
//! Search is an index over podcasts and episodes that the database keeps
//! in step by trigger, and a query language the user never sees. What
//! they type is turned into a bounded list of quoted terms — never handed
//! to the FTS5 parser as written — so no input can produce a syntax
//! error, and none of the operator vocabulary (`*`, `"`, `(`, `)`, `:`,
//! `^`, `-`, `AND`, `OR`, `NOT`, `NEAR`) means anything but itself.
//!
//! That claim is about *syntax*. A corrupt index, a closed pool or a disk
//! error is still an ordinary query failure and surfaces as one: search
//! is not exempt from the failure modes every other read has.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{EpisodeId, PodcastId};
use crate::model::ArchiveState;

/// How much of a description is indexed.
///
/// A bound on an input the publisher chooses, not a guess: some feeds
/// carry a book in every item. Search covers the first ~4 000 characters
/// of the shownotes, and `docs/SERVICE.md` says so plainly.
pub const SEARCH_EXCERPT_CHARS: usize = 4000;

/// Longest term taken from a query.
pub const MAX_TERM_CHARS: usize = 64;

/// Most terms taken from a query.
pub const MAX_TERMS: usize = 16;

/// Shortest term that may be turned into a prefix search. One letter
/// would match most of the library and cost a full index scan to say so.
pub const MIN_PREFIX_CHARS: usize = 2;

/// A user's input, reduced to something FTS5 cannot misread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ParsedQuery {
    /// The terms, in the order they were typed.
    pub terms: Vec<String>,
    /// Whether the last term matches as a prefix.
    pub prefix: bool,
    /// Whether anything was dropped for being too long or too much.
    pub truncated: bool,
}

impl ParsedQuery {
    /// The `MATCH` expression.
    ///
    /// Every term is a quoted phrase, which is what makes the whole
    /// operator vocabulary literal: inside quotes, `OR` is the word "or"
    /// and `-` is a hyphen. Terms are implicitly ANDed, so more words
    /// narrow the result the way people expect.
    #[must_use]
    pub fn match_expression(&self) -> String {
        let last = self.terms.len().saturating_sub(1);
        self.terms
            .iter()
            .enumerate()
            .map(|(i, term)| {
                if self.prefix && i == last && term.chars().count() >= MIN_PREFIX_CHARS {
                    format!("\"{term}\"*")
                } else {
                    format!("\"{term}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Turns user input into terms, or `None` when there is nothing to search
/// for.
///
/// Splitting on everything that is not alphanumeric is what removes the
/// operators, and it is also how `unicode61` split the indexed text, so
/// the two sides agree. Case and diacritics are left alone on purpose:
/// FTS5 applies the table's tokenizer to the query as well, and folding
/// them here a second, slightly different way is how a search stops
/// finding what it indexed.
#[must_use]
pub fn parse_query(input: &str, prefix: bool) -> Option<ParsedQuery> {
    let mut terms: Vec<String> = Vec::new();
    let mut truncated = false;
    for raw in input.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        if terms.len() == MAX_TERMS {
            truncated = true;
            break;
        }
        let term: String = raw.chars().take(MAX_TERM_CHARS).collect();
        truncated |= term.chars().count() < raw.chars().count();
        terms.push(term);
    }
    if terms.is_empty() {
        return None;
    }
    Some(ParsedQuery {
        terms,
        prefix,
        truncated,
    })
}

/// Whether the index can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    /// Built, and kept current by the triggers.
    Ready,
    /// Being built right now; a search says so rather than answering with
    /// a silence that looks like "no results".
    Building,
    /// Never built, or invalidated. Searches report it.
    Stale,
}

impl IndexState {
    /// Stable string form (also the stored value).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Building => "building",
            Self::Stale => "stale",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ready" => Some(Self::Ready),
            "building" => Some(Self::Building),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

/// The index's state as the database holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchIndexStatus {
    /// Ready, building or stale.
    pub state: IndexState,
    /// When it was last finished.
    #[serde(with = "time::serde::rfc3339::option")]
    pub built_at: Option<OffsetDateTime>,
    /// Podcasts in the index now.
    pub podcasts: u64,
    /// Episodes in the index now.
    pub episodes: u64,
    /// What went wrong, when something did.
    pub detail: Option<String>,
    /// Last change to this row.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// One episode a search found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodeHit {
    /// The episode.
    pub episode_id: EpisodeId,
    /// Its podcast.
    pub podcast_id: PodcastId,
    /// The podcast's title, so a result list needs no second query.
    pub podcast_title: String,
    /// The episode's title.
    pub title: String,
    /// Where the match is, with the matched terms marked.
    pub snippet: String,
    /// Publication time, when the feed gave a usable one.
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    /// Duration in seconds.
    pub duration_secs: Option<u32>,
    /// Whether the episode is archived, queued, missing, …
    pub archive_state: ArchiveState,
    /// The relevance FTS5 computed (lower is better; negated on the way
    /// out so that higher is better).
    pub relevance: f64,
}

/// One podcast a search found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastHit {
    /// The podcast.
    pub podcast_id: PodcastId,
    /// Its title.
    pub title: String,
    /// Its author.
    pub author: Option<String>,
    /// Where the match is.
    pub snippet: String,
    /// The relevance FTS5 computed.
    pub relevance: f64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn an_operator_is_only_ever_a_word() {
        // Every one of these is a query somebody will type by accident.
        for hostile in [
            "foo OR bar",
            "\"unterminated",
            "a*b",
            "NEAR(x y)",
            "col:value",
            "^anchor",
            "-negated",
            "a AND NOT b",
            "((x))",
        ] {
            let parsed = parse_query(hostile, false).expect("something to search for");
            let expression = parsed.match_expression();
            assert!(
                !expression.contains('*')
                    && !expression.contains(':')
                    && !expression.contains('^')
                    && !expression.contains('(')
                    && !expression.contains('-'),
                "{hostile} produced {expression}"
            );
            // Two quotes per term, and nothing else quoted.
            assert_eq!(
                expression.matches('"').count(),
                parsed.terms.len() * 2,
                "{hostile} produced {expression}"
            );
        }
    }

    #[test]
    fn input_without_terms_is_none() {
        for empty in ["", "   ", "***", "\"\"", "-", "()", "\n\t"] {
            assert!(parse_query(empty, false).is_none(), "{empty:?}");
        }
    }

    #[test]
    fn terms_are_bounded_in_length_and_number() {
        let long = "a".repeat(MAX_TERM_CHARS + 50);
        let parsed = parse_query(&long, false).unwrap();
        assert_eq!(parsed.terms[0].chars().count(), MAX_TERM_CHARS);
        assert!(parsed.truncated);

        let many = (0..MAX_TERMS + 5)
            .map(|n| format!("t{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let parsed = parse_query(&many, false).unwrap();
        assert_eq!(parsed.terms.len(), MAX_TERMS);
        assert!(parsed.truncated);
        assert!(!parse_query("two words", false).unwrap().truncated);
    }

    #[test]
    fn a_prefix_stars_the_last_term() {
        let parsed = parse_query("rust pod", true).unwrap();
        assert_eq!(parsed.match_expression(), "\"rust\" \"pod\"*");
        let parsed = parse_query("rust p", true).unwrap();
        assert_eq!(
            parsed.match_expression(),
            "\"rust\" \"p\"",
            "one letter would match the whole library"
        );
        let parsed = parse_query("rust pod", false).unwrap();
        assert_eq!(parsed.match_expression(), "\"rust\" \"pod\"");
    }

    #[test]
    fn case_and_diacritics_stay_unfolded() {
        // The index folds them with `unicode61 remove_diacritics 2`, and
        // FTS5 folds the query the same way. Doing it here as well, even
        // slightly differently, is how a search stops finding what it
        // indexed.
        let parsed = parse_query("Café Größe", false).unwrap();
        assert_eq!(parsed.terms, vec!["Café", "Größe"]);
    }

    #[test]
    fn index_states_round_trip() {
        for state in [IndexState::Ready, IndexState::Building, IndexState::Stale] {
            assert_eq!(IndexState::parse(state.as_str()), Some(state));
        }
        assert!(IndexState::parse("half-built").is_none());
    }
}
