//! Explainable ranking.
//!
//! Every candidate gets a [`RankingExplanation`] listing each signal's
//! value, weight and contribution. Weights are configurable; the defaults
//! are the original design values, kept unless a test in this module shows a
//! need to change them (see `docs/DISCOVERY.md` §7 for the rationale).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime};
use uguisu_core::provider::ProviderId;

use crate::candidate::PodcastCandidate;
use crate::dedup::AmbiguityNote;
use crate::fuzzy::{fold_for_match, jaro_winkler, token_coverage, token_set_ratio};
use crate::query::NormalizedQuery;

/// Signal weights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RankingWeights {
    /// Folded query equals folded title.
    pub exact_title: f32,
    /// Token-set ratio between query and title.
    pub token_set: f32,
    /// Jaro-Winkler between query and title (typo tolerance).
    pub jaro_winkler: f32,
    /// Improvement when the author is added to the matched text.
    pub author: f32,
    /// A query token appears in the website host.
    pub website: f32,
    /// Provider-supplied popularity (0..1).
    pub popularity: f32,
    /// Feed present, HTTPS, has episodes, recently updated, not dead/locked.
    pub feed_quality: f32,
    /// Configured trust of the best contributing provider.
    pub provider_trust: f32,
    /// Artwork present.
    pub artwork: f32,
    /// Several providers agree on the same podcast.
    pub agreement: f32,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            exact_title: 3.0,
            token_set: 2.5,
            jaro_winkler: 1.5,
            author: 1.0,
            website: 0.5,
            popularity: 1.0,
            feed_quality: 1.0,
            provider_trust: 0.5,
            artwork: 0.2,
            agreement: 0.8,
        }
    }
}

impl RankingWeights {
    /// Sum of all weights (the maximum possible score).
    pub fn total(&self) -> f32 {
        self.exact_title
            + self.token_set
            + self.jaro_winkler
            + self.author
            + self.website
            + self.popularity
            + self.feed_quality
            + self.provider_trust
            + self.artwork
            + self.agreement
    }
}

/// One signal in an explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Signal {
    /// Signal name (stable identifier).
    pub name: String,
    /// Configured weight.
    pub weight: f32,
    /// Observed value in 0..1.
    pub value: f32,
    /// `weight × value`.
    pub contribution: f32,
    /// Short human-readable note.
    pub note: String,
}

/// Why a candidate scored what it scored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RankingExplanation {
    /// Signals in evaluation order.
    pub signals: Vec<Signal>,
    /// Sum of contributions.
    pub total: f32,
    /// Maximum possible total for the configured weights.
    pub max_total: f32,
}

/// A ranked, merged candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RankedCandidate {
    /// 1-based rank.
    pub rank: usize,
    /// Score in 0..`max_total`.
    pub score: f32,
    /// Score normalized to 0..1.
    pub confidence: f32,
    /// The candidate.
    pub candidate: PodcastCandidate,
    /// Explanation.
    pub explanation: RankingExplanation,
    /// Similar candidates that were deliberately not merged.
    pub ambiguities: Vec<AmbiguityNote>,
}

/// Inputs to ranking that are not part of the candidate.
#[derive(Debug, Clone, Default)]
pub struct RankContext {
    /// Provider trust by id.
    pub trust: HashMap<ProviderId, f32>,
    /// Weights.
    pub weights: RankingWeights,
    /// "Now", for recency; defaults to the current time.
    pub now: Option<OffsetDateTime>,
}

fn feed_quality(c: &PodcastCandidate, now: OffsetDateTime) -> (f32, String) {
    let mut notes = Vec::new();
    if c.health.dead == Some(true) {
        return (0.0, "directory marks the feed as dead".to_owned());
    }
    let mut v: f32 = 0.0;
    if let Some(f) = &c.feed_url {
        v += 0.5;
        notes.push("feed url");
        if f.scheme() == "https" {
            v += 0.1;
            notes.push("https");
        }
    }
    if c.episode_count.is_some_and(|n| n > 0) {
        v += 0.15;
        notes.push("has episodes");
    }
    if c.last_published
        .is_some_and(|d| now - d < TimeDuration::days(730))
    {
        v += 0.15;
        notes.push("updated within 2 years");
    }
    if c.health.locked != Some(true) && c.health.dead != Some(true) {
        v += 0.1;
        notes.push("not locked/dead");
    }
    (v.min(1.0), notes.join(", "))
}

/// Removes whitespace so "dark net" and "darknet" compare equal.
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Scores one candidate.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)] // ten signals in evaluation order; provider counts are tiny
pub fn explain(
    query: &NormalizedQuery,
    c: &PodcastCandidate,
    ctx: &RankContext,
) -> RankingExplanation {
    let w = &ctx.weights;
    let now = ctx.now.unwrap_or_else(OffsetDateTime::now_utc);
    let q = query.ascii.as_str();
    let title = fold_for_match(&c.title);
    let mut signals = Vec::with_capacity(10);
    let mut push = |name: &'static str, weight: f32, value: f32, note: String| {
        let value = value.clamp(0.0, 1.0);
        signals.push(Signal {
            name: name.to_owned(),
            weight,
            value,
            contribution: weight * value,
            note,
        });
    };

    // Spacing variants ("dark net" vs "darknet") count as exact when the user typed several words.
    let exact =
        !q.is_empty() && (q == title || (query.tokens.len() > 1 && squash(q) == squash(&title)));
    push(
        "exact_title",
        w.exact_title,
        if exact { 1.0 } else { 0.0 },
        if exact {
            "title matches exactly".into()
        } else {
            format!("title `{title}`")
        },
    );

    let ts = token_set_ratio(q, &title);
    push(
        "token_set",
        w.token_set,
        ts,
        format!("token-set ratio {ts:.2}"),
    );

    let jw = jaro_winkler(q, &title)
        .max(jaro_winkler(&squash(q), &squash(&title)))
        .max(token_coverage(q, &title) * 0.95);
    push(
        "jaro_winkler",
        w.jaro_winkler,
        jw,
        format!("typo-tolerant similarity {jw:.2}"),
    );

    let author_gain = match c.author.as_deref() {
        Some(a) => {
            let with_author = token_set_ratio(q, &format!("{title} {}", fold_for_match(a)));
            (with_author - ts).max(0.0)
        }
        None => 0.0,
    };
    push(
        "author",
        w.author,
        author_gain,
        match &c.author {
            Some(a) if author_gain > 0.0 => format!("author `{a}` matches part of the query"),
            Some(a) => format!("author `{a}`"),
            None => "no author".into(),
        },
    );

    let website_hit = c
        .website
        .as_ref()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| {
            query
                .tokens
                .iter()
                .any(|t| t.len() >= 4 && host.contains(t.as_str()))
        });
    push(
        "website",
        w.website,
        if website_hit { 1.0 } else { 0.0 },
        if website_hit {
            "website host contains a query word".into()
        } else {
            "no website match".into()
        },
    );

    let pop = c.popularity_score().unwrap_or(0.0);
    push(
        "popularity",
        w.popularity,
        pop,
        if c.popularity.is_empty() {
            "no popularity data".into()
        } else {
            format!("popularity {pop:.2}")
        },
    );

    let (fq, fq_note) = feed_quality(c, now);
    push("feed_quality", w.feed_quality, fq, fq_note);

    let trust = c
        .providers()
        .iter()
        .filter_map(|p| ctx.trust.get(p).copied())
        .fold(0.0_f32, f32::max);
    push(
        "provider_trust",
        w.provider_trust,
        trust,
        format!("best provider trust {trust:.2}"),
    );

    push(
        "artwork",
        w.artwork,
        if c.artwork.is_some() { 1.0 } else { 0.0 },
        if c.artwork.is_some() {
            "artwork present".into()
        } else {
            "no artwork".into()
        },
    );

    let n = c.providers().len();
    let agreement = ((n.saturating_sub(1)) as f32 / 2.0).min(1.0);
    push(
        "agreement",
        w.agreement,
        agreement,
        format!("{n} provider(s) agree"),
    );

    let total = signals.iter().map(|s| s.contribution).sum();
    RankingExplanation {
        signals,
        total,
        max_total: w.total(),
    }
}

/// Ranks merged candidates (with their ambiguity notes) for a query.
pub fn rank(
    query: &NormalizedQuery,
    candidates: Vec<(PodcastCandidate, Vec<AmbiguityNote>)>,
    ctx: &RankContext,
) -> Vec<RankedCandidate> {
    let max_total = ctx.weights.total().max(f32::EPSILON);
    let mut ranked: Vec<RankedCandidate> = candidates
        .into_iter()
        .map(|(candidate, ambiguities)| {
            let explanation = explain(query, &candidate, ctx);
            let score = explanation.total;
            RankedCandidate {
                rank: 0,
                score,
                confidence: (score / max_total).clamp(0.0, 1.0),
                candidate,
                explanation,
                ambiguities,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.candidate.title.cmp(&b.candidate.title))
            .then_with(|| {
                a.candidate
                    .identities
                    .first()
                    .map(|i| i.provider_ref.clone())
                    .cmp(
                        &b.candidate
                            .identities
                            .first()
                            .map(|i| i.provider_ref.clone()),
                    )
            })
    });
    for (i, r) in ranked.iter_mut().enumerate() {
        r.rank = i + 1;
    }
    ranked
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use url::Url;

    use super::*;
    use crate::candidate::{Popularity, ProviderIdentity};

    fn cand(title: &str, author: Option<&str>, feed: Option<&str>) -> PodcastCandidate {
        let identity = ProviderIdentity {
            provider: ProviderId::APPLE,
            provider_ref: title.to_owned(),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        };
        let mut c = PodcastCandidate::new(title, identity);
        c.author = author.map(str::to_owned);
        c.feed_url = feed.map(|f| Url::parse(f).unwrap());
        c.episode_count = Some(100);
        c.last_published = Some(OffsetDateTime::UNIX_EPOCH + TimeDuration::days(20_000));
        c.artwork = Some(Url::parse("https://a.test/x.jpg").unwrap());
        c
    }

    fn corpus() -> Vec<(PodcastCandidate, Vec<AmbiguityNote>)> {
        vec![
            (
                cand(
                    "Darknet Diaries",
                    Some("Jack Rhysider"),
                    Some("https://feeds.megaphone.fm/darknetdiaries"),
                ),
                vec![],
            ),
            (
                cand(
                    "Darknet Diaries Fan Recap",
                    Some("Recap Crew"),
                    Some("https://recap.test/feed"),
                ),
                vec![],
            ),
            (
                cand(
                    "Diary of a CEO",
                    Some("Steven Bartlett"),
                    Some("https://ceo.test/feed"),
                ),
                vec![],
            ),
            (
                cand(
                    "Dark Net",
                    Some("Some Network"),
                    Some("https://darknet-show.test/rss"),
                ),
                vec![],
            ),
            (
                cand(
                    "The Daily",
                    Some("The New York Times"),
                    Some("https://daily.test/rss"),
                ),
                vec![],
            ),
            (
                cand(
                    "Hacking Diaries",
                    Some("Nobody"),
                    Some("https://hd.test/rss"),
                ),
                vec![],
            ),
        ]
    }

    fn ctx() -> RankContext {
        RankContext {
            trust: HashMap::from([(ProviderId::APPLE, 0.8)]),
            weights: RankingWeights::default(),
            now: Some(OffsetDateTime::UNIX_EPOCH + TimeDuration::days(20_100)),
        }
    }

    fn top_title(query: &str) -> String {
        rank(&NormalizedQuery::parse(query), corpus(), &ctx())[0]
            .candidate
            .title
            .clone()
    }

    #[test]
    fn brief_query_table_ranks_darknet_diaries_first() {
        for q in [
            "Darknet Diaries",
            "darknet diaries",
            "dark net diaries",
            "darknet diary",
            "darknet diariez",
            "darknet",
            "rhysider darknet",
            "DARKNET   DIARIES!",
        ] {
            assert_eq!(top_title(q), "Darknet Diaries", "query `{q}`");
        }
        assert_eq!(top_title("diary of a ceo"), "Diary of a CEO");
        assert_eq!(top_title("the daily"), "The Daily");
    }

    #[test]
    fn explanation_lists_every_signal_with_contributions() {
        let ranked = rank(&NormalizedQuery::parse("darknet diaries"), corpus(), &ctx());
        let top = &ranked[0];
        assert_eq!(top.rank, 1);
        assert_eq!(top.explanation.signals.len(), 10);
        let exact = top
            .explanation
            .signals
            .iter()
            .find(|s| s.name == "exact_title")
            .unwrap();
        assert_eq!(exact.value, 1.0);
        assert_eq!(exact.contribution, 3.0);
        let sum: f32 = top.explanation.signals.iter().map(|s| s.contribution).sum();
        assert!((sum - top.explanation.total).abs() < 1e-4);
        assert!(top.confidence > 0.6 && top.confidence <= 1.0);
        assert!(
            ranked
                .iter()
                .all(|r| r.explanation.max_total == RankingWeights::default().total())
        );
        assert!(ranked.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn author_signal_rewards_author_queries_only() {
        let q = NormalizedQuery::parse("rhysider darknet");
        let e = explain(&q, &corpus()[0].0, &ctx());
        let author = e.signals.iter().find(|s| s.name == "author").unwrap();
        assert!(author.value > 0.3, "{author:?}");
        let e2 = explain(
            &NormalizedQuery::parse("darknet diaries"),
            &corpus()[0].0,
            &ctx(),
        );
        assert_eq!(
            e2.signals
                .iter()
                .find(|s| s.name == "author")
                .unwrap()
                .value,
            0.0
        );
    }

    #[test]
    fn feed_quality_popularity_and_agreement() {
        let mut dead = cand("Dead Show", None, Some("http://dead.test/rss"));
        dead.health.dead = Some(true);
        let e = explain(&NormalizedQuery::parse("dead show"), &dead, &ctx());
        assert_eq!(
            e.signals
                .iter()
                .find(|s| s.name == "feed_quality")
                .unwrap()
                .value,
            0.0
        );

        let mut popular = cand("Pop Show", None, Some("https://pop.test/rss"));
        popular.popularity.push(Popularity {
            provider: ProviderId::GPODDER_NET,
            raw: 1.0,
            normalized: 0.7,
            label: "subscribers".into(),
        });
        popular.identities.push(ProviderIdentity {
            provider: ProviderId::PODCAST_INDEX,
            provider_ref: "2".into(),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        });
        popular.identities.push(ProviderIdentity {
            provider: ProviderId::GPODDER_NET,
            provider_ref: "3".into(),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        });
        let e = explain(&NormalizedQuery::parse("pop show"), &popular, &ctx());
        assert_eq!(
            e.signals
                .iter()
                .find(|s| s.name == "popularity")
                .unwrap()
                .value,
            0.7
        );
        assert_eq!(
            e.signals
                .iter()
                .find(|s| s.name == "agreement")
                .unwrap()
                .value,
            1.0
        );
        assert!(
            e.signals
                .iter()
                .find(|s| s.name == "feed_quality")
                .unwrap()
                .value
                > 0.9
        );
    }

    #[test]
    fn ranking_is_deterministic_for_ties() {
        let a = cand("Same Title", None, Some("https://a.test/rss"));
        let mut b = a.clone();
        b.identities[0].provider_ref = "zzz".into();
        let r1 = rank(
            &NormalizedQuery::parse("same title"),
            vec![(a.clone(), vec![]), (b.clone(), vec![])],
            &ctx(),
        );
        let r2 = rank(
            &NormalizedQuery::parse("same title"),
            vec![(b, vec![]), (a, vec![])],
            &ctx(),
        );
        assert_eq!(
            r1[0].candidate.identities[0].provider_ref,
            r2[0].candidate.identities[0].provider_ref
        );
    }
}
