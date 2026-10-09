//! Local search over the library (ADR 0029).
//!
//! The index is the database's to keep current — migration 0005's
//! triggers do that for every writer. What the engine adds is the parts a
//! trigger cannot express: building the index for rows that predate it,
//! saying so while that runs, and ranking what comes back.
//!
//! Ranking is FTS5's `bm25` plus three adjustments, in the shape
//! `uguisu-discovery` already uses for provider results: a named signal
//! with a weight, a value and a contribution, so `--explain` can show the
//! arithmetic instead of asserting the answer.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::UguisuError;
use uguisu_core::search::{
    EpisodeHit, IndexState, ParsedQuery, PodcastHit, SearchIndexStatus, parse_query,
};
use uguisu_core::{Event, EventKind};
use uguisu_storage::search as search_repo;

use crate::{Engine, lock};

/// Results returned when the caller does not say.
pub const DEFAULT_LIMIT: u32 = 25;

/// Most results one search may return.
pub const MAX_LIMIT: u32 = 200;

/// Half-life of the recency signal: an episode a month old scores half
/// the recency of one published today.
const RECENCY_HALF_LIFE_DAYS: f64 = 30.0;

/// What a search was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// What the user typed.
    pub text: String,
    /// How many hits of each kind to return.
    pub limit: u32,
    /// Whether the last word matches as a prefix (a search box that
    /// searches as you type wants this; a finished query does not).
    pub prefix: bool,
    /// Whether to include podcasts.
    pub podcasts: bool,
    /// Whether to include episodes.
    pub episodes: bool,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            text: String::new(),
            limit: DEFAULT_LIMIT,
            prefix: false,
            podcasts: true,
            episodes: true,
        }
    }
}

impl SearchRequest {
    /// A search for `text` with everything else left as it comes.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// How a search ended. Never a silence that has to be guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchOutcome {
    /// Hits were found.
    Ok,
    /// The query was well formed and nothing matched.
    NoResults,
    /// There was nothing to search for: empty input, or only punctuation.
    EmptyQuery,
    /// The index is being built. The counts say how far it has got, and
    /// the answer is "ask again", not "nothing matched".
    IndexBuilding,
    /// The index has never been built, or was invalidated.
    IndexStale,
}

impl SearchOutcome {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoResults => "no_results",
            Self::EmptyQuery => "empty_query",
            Self::IndexBuilding => "index_building",
            Self::IndexStale => "index_stale",
        }
    }
}

/// One component of a hit's score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchSignal {
    /// Stable name.
    pub name: String,
    /// Configured weight.
    pub weight: f64,
    /// Observed value, 0..1.
    pub value: f64,
    /// `weight × value`.
    pub contribution: f64,
    /// One line for a person reading `--explain`.
    pub note: String,
}

/// An episode hit with its score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RankedEpisode {
    /// What the index found.
    #[serde(flatten)]
    pub hit: EpisodeHit,
    /// Sum of the contributions.
    pub score: f64,
    /// Why, when the caller asked.
    pub signals: Vec<SearchSignal>,
}

/// A podcast hit with its score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RankedPodcast {
    /// What the index found.
    #[serde(flatten)]
    pub hit: PodcastHit,
    /// Sum of the contributions.
    pub score: f64,
    /// Why, when the caller asked.
    pub signals: Vec<SearchSignal>,
}

/// Everything a search returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchResults {
    /// How it ended.
    pub outcome: SearchOutcome,
    /// The terms that were actually searched for, so a user can see what
    /// happened to what they typed.
    pub terms: Vec<String>,
    /// Whether the query was cut short (too many or too long terms).
    pub truncated: bool,
    /// Matching podcasts, best first.
    pub podcasts: Vec<RankedPodcast>,
    /// Matching episodes, best first.
    pub episodes: Vec<RankedEpisode>,
    /// The index's state at the time of the search.
    pub index: SearchIndexStatus,
    /// How long it took.
    pub duration_ms: u64,
}

/// What a rebuild did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReindexReport {
    /// Podcasts indexed.
    pub podcasts: u64,
    /// Episodes indexed.
    pub episodes: u64,
    /// How long it took.
    pub duration_ms: u64,
}

impl Engine {
    /// Searches the library.
    ///
    /// Nothing is executed for an empty query: no SQL, no index read, and
    /// an outcome that says why. A search while the index is being built
    /// answers with what the index has *and* says it is incomplete —
    /// silently returning fewer results would be the one failure mode a
    /// search must not have.
    pub async fn search_library(
        &self,
        request: &SearchRequest,
    ) -> Result<SearchResults, UguisuError> {
        let started = Instant::now();
        let index = self.search_index_status().await?;
        let limit = request.limit.clamp(1, MAX_LIMIT);
        let Some(parsed) = parse_query(&request.text, request.prefix) else {
            return Ok(SearchResults {
                outcome: SearchOutcome::EmptyQuery,
                terms: Vec::new(),
                truncated: false,
                podcasts: Vec::new(),
                episodes: Vec::new(),
                index,
                duration_ms: elapsed_ms(started),
            });
        };
        let expression = parsed.match_expression();
        let mut reader = self.storage().reader().await?;
        let podcasts = if request.podcasts {
            search_repo::search_podcasts(&mut reader, &expression, limit).await?
        } else {
            Vec::new()
        };
        let episodes = if request.episodes {
            search_repo::search_episodes(&mut reader, &expression, limit).await?
        } else {
            Vec::new()
        };
        drop(reader);

        let now = OffsetDateTime::now_utc();
        let mut podcasts: Vec<RankedPodcast> = podcasts
            .into_iter()
            .map(|hit| {
                let signals = podcast_signals(&hit, &parsed);
                RankedPodcast {
                    score: total(&signals),
                    signals,
                    hit,
                }
            })
            .collect();
        let mut episodes: Vec<RankedEpisode> = episodes
            .into_iter()
            .map(|hit| {
                let signals = episode_signals(&hit, &parsed, now);
                RankedEpisode {
                    score: total(&signals),
                    signals,
                    hit,
                }
            })
            .collect();
        // The index returned them in `bm25` order; the signals may reorder
        // them. Ties keep the index's order, which is already stable.
        podcasts.sort_by(|a, b| b.score.total_cmp(&a.score));
        episodes.sort_by(|a, b| b.score.total_cmp(&a.score));

        let empty = podcasts.is_empty() && episodes.is_empty();
        let outcome = match (index.state, empty) {
            (IndexState::Building, _) => SearchOutcome::IndexBuilding,
            (IndexState::Stale, true) => SearchOutcome::IndexStale,
            (_, true) => SearchOutcome::NoResults,
            (_, false) => SearchOutcome::Ok,
        };
        Ok(SearchResults {
            outcome,
            terms: parsed.terms,
            truncated: parsed.truncated,
            podcasts,
            episodes,
            index,
            duration_ms: elapsed_ms(started),
        })
    }

    /// The index's state.
    pub async fn search_index_status(&self) -> Result<SearchIndexStatus, UguisuError> {
        let mut reader = self.storage().reader().await?;
        Ok(search_repo::state(&mut reader).await?)
    }

    /// Rebuilds the index from the library.
    ///
    /// In batches, one transaction each, because the writer pool has one
    /// connection and a hundred thousand episodes in a single transaction
    /// would hold it for the whole build. The state row says `building`
    /// while that happens, so a search during a rebuild can say so.
    ///
    /// Interrupting it leaves the row at `building` with the counts it
    /// reached; the next start finds a state that is not `ready` and
    /// builds again. Nothing is lost, because the index is derived.
    pub async fn reindex_search(&self) -> Result<ReindexReport, UguisuError> {
        let started = Instant::now();
        let now = OffsetDateTime::now_utc();
        {
            let mut tx = self.storage().begin().await?;
            search_repo::clear(&mut tx).await?;
            search_repo::set_state(&mut tx, IndexState::Building, None, now).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
        }
        let mut report = ReindexReport::default();
        let mut cursor = None;
        loop {
            let mut tx = self.storage().begin().await?;
            let (n, last) =
                search_repo::index_podcasts(&mut tx, cursor, search_repo::BATCH).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            report.podcasts += n;
            match last {
                Some(id) => cursor = Some(id),
                None => break,
            }
        }
        let mut cursor = None;
        loop {
            if self.inner.shutdown.is_cancelled() {
                tracing::info!("search index build stopped; it will resume on the next start");
                return Ok(report);
            }
            let mut tx = self.storage().begin().await?;
            let (n, last) =
                search_repo::index_episodes(&mut tx, cursor, search_repo::BATCH).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            report.episodes += n;
            match last {
                Some(id) => cursor = Some(id),
                None => break,
            }
        }
        let mut writer = self.storage().writer().await?;
        search_repo::optimize(&mut writer).await?;
        drop(writer);
        let mut tx = self.storage().begin().await?;
        search_repo::set_state(&mut tx, IndexState::Ready, None, OffsetDateTime::now_utc()).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        report.duration_ms = elapsed_ms(started);
        let event = vec![Event::now(
            None,
            None,
            EventKind::SearchReindexed {
                podcasts: report.podcasts,
                episodes: report.episodes,
                duration_ms: report.duration_ms,
                full: true,
            },
        )];
        {
            let mut tx = self.storage().begin().await?;
            uguisu_storage::events::insert_all(&mut tx, &event).await?;
            tx.commit()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
        }
        self.bus().publish(&event);
        tracing::info!(
            podcasts = report.podcasts,
            episodes = report.episodes,
            duration_ms = report.duration_ms,
            "search index built"
        );
        Ok(report)
    }

    /// Builds the index in the background if it is not ready
    /// (idempotent).
    ///
    /// Migrations create the tables but do not fill them: they run inside
    /// `Engine::open`, in one transaction, and a hundred thousand rows
    /// there would be tens of seconds that an interrupted start rolls
    /// back and repeats for ever. This is where that work belongs.
    pub fn start_search_index(&self) {
        let mut slot = lock(&self.inner.search_build);
        if slot.is_some() {
            return;
        }
        let engine = self.clone();
        *slot = Some(tokio::spawn(async move {
            match engine.search_index_status().await {
                Ok(status) if status.state == IndexState::Ready => return,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "search index state unreadable");
                    return;
                }
            }
            if let Err(e) = engine.reindex_search().await {
                tracing::warn!(error = %e, "search index build failed; searches will say so");
            }
        }));
    }

    /// Stops waiting for a background build, bounded by `grace`.
    pub(crate) async fn stop_search_index(&self, grace: std::time::Duration) {
        let task = lock(&self.inner.search_build).take();
        let Some(task) = task else { return };
        if tokio::time::timeout(grace, task).await.is_err() {
            tracing::warn!("search index build did not stop in time; it resumes on the next start");
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn total(signals: &[SearchSignal]) -> f64 {
    signals.iter().map(|s| s.contribution).sum()
}

fn signal(name: &str, weight: f64, value: f64, note: impl Into<String>) -> SearchSignal {
    let value = value.clamp(0.0, 1.0);
    SearchSignal {
        name: name.to_owned(),
        weight,
        value,
        contribution: weight * value,
        note: note.into(),
    }
}

/// `bm25` is unbounded; this maps it into 0..1 so it can share a scale
/// with the other signals. The curve is gentle on purpose — the point is
/// to let a title match or a recent episode break a near-tie, not to
/// overrule the text.
fn text_value(relevance: f64) -> f64 {
    let r = relevance.max(0.0);
    r / (r + 1.0)
}

fn episode_signals(
    hit: &EpisodeHit,
    query: &ParsedQuery,
    now: OffsetDateTime,
) -> Vec<SearchSignal> {
    let mut signals = vec![signal(
        "text",
        1.0,
        text_value(hit.relevance),
        format!("bm25 {:.3}", hit.relevance),
    )];
    signals.push(signal(
        "exact_title",
        0.30,
        f64::from(u8::from(is_exact_title(&hit.title, query))),
        "the title is what was searched for",
    ));
    let (value, note) = recency(hit.published_at, now);
    signals.push(signal("recency", 0.20, value, note));
    let archived = hit.archive_state == uguisu_core::model::ArchiveState::Archived;
    signals.push(signal(
        "archived",
        0.10,
        f64::from(u8::from(archived)),
        if archived {
            "already in the archive"
        } else {
            "not archived"
        },
    ));
    signals
}

fn podcast_signals(hit: &PodcastHit, query: &ParsedQuery) -> Vec<SearchSignal> {
    vec![
        signal(
            "text",
            1.0,
            text_value(hit.relevance),
            format!("bm25 {:.3}", hit.relevance),
        ),
        signal(
            "exact_title",
            0.30,
            f64::from(u8::from(is_exact_title(&hit.title, query))),
            "the title is what was searched for",
        ),
    ]
}

/// Whether every term appears in the title and the title has nothing else
/// of substance — "the thing I typed *is* this show".
fn is_exact_title(title: &str, query: &ParsedQuery) -> bool {
    let title_terms: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect();
    if title_terms.is_empty() || title_terms.len() != query.terms.len() {
        return false;
    }
    title_terms
        .iter()
        .zip(query.terms.iter())
        .all(|(a, b)| *a == b.to_lowercase())
}

/// Newer is better, halving every [`RECENCY_HALF_LIFE_DAYS`]. An episode
/// with no usable date scores zero rather than being penalised out of
/// the results: plenty of feeds have none.
fn recency(published_at: Option<OffsetDateTime>, now: OffsetDateTime) -> (f64, String) {
    let Some(at) = published_at else {
        return (0.0, "no publication date".to_owned());
    };
    let days = (now - at).as_seconds_f64() / 86_400.0;
    if days < 0.0 {
        return (1.0, "published in the future".to_owned());
    }
    let value = 0.5_f64.powf(days / RECENCY_HALF_LIFE_DAYS);
    (value, format!("{days:.0} days old"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use uguisu_core::ids::{EpisodeId, PodcastId};
    use uguisu_core::model::ArchiveState;

    fn hit(title: &str, relevance: f64, published: Option<OffsetDateTime>) -> EpisodeHit {
        EpisodeHit {
            episode_id: EpisodeId::new(),
            podcast_id: PodcastId::new(),
            podcast_title: "Show".to_owned(),
            title: title.to_owned(),
            snippet: String::new(),
            published_at: published,
            duration_secs: None,
            archive_state: ArchiveState::Expected,
            relevance,
        }
    }

    #[test]
    fn signals_sum_to_the_score() {
        let now = OffsetDateTime::now_utc();
        let query = parse_query("async runtimes", false).unwrap();
        let signals = episode_signals(&hit("Async runtimes", 3.0, Some(now)), &query, now);
        let score: f64 = signals.iter().map(|s| s.contribution).sum();
        assert!((total(&signals) - score).abs() < f64::EPSILON);
        for s in &signals {
            assert!((0.0..=1.0).contains(&s.value), "{}: {}", s.name, s.value);
            assert!(s.contribution <= s.weight + f64::EPSILON);
        }
        let exact = signals.iter().find(|s| s.name == "exact_title").unwrap();
        assert!(exact.value > 0.99);
    }

    #[test]
    fn containing_the_words_is_not_exact() {
        let query = parse_query("async runtimes", false).unwrap();
        assert!(is_exact_title("Async Runtimes", &query));
        assert!(is_exact_title("async, runtimes!", &query));
        assert!(!is_exact_title("Async runtimes explained", &query));
        assert!(!is_exact_title("Runtimes async", &query), "order matters");
        assert!(!is_exact_title("", &query));
    }

    #[test]
    fn recency_halves_no_date_is_free() {
        let now = OffsetDateTime::now_utc();
        let (today, _) = recency(Some(now), now);
        let (month, _) = recency(Some(now - time::Duration::days(30)), now);
        let (year, _) = recency(Some(now - time::Duration::days(365)), now);
        assert!((today - 1.0).abs() < 0.01);
        assert!((month - 0.5).abs() < 0.01);
        assert!(year < month);
        let (undated, undated_note) = recency(None, now);
        assert!(undated < f64::EPSILON);
        assert!(undated_note.contains("no publication date"));
        // A clock that disagrees with a feed must not produce a negative
        // signal, which would rank a mis-dated episode below everything.
        let (future, _) = recency(Some(now + time::Duration::days(3)), now);
        assert!(future > 0.99);
    }

    #[test]
    fn the_text_signal_is_bounded() {
        assert!(text_value(0.0) < f64::EPSILON);
        assert!(text_value(1.0) > 0.4 && text_value(1.0) < 0.6);
        assert!(text_value(1e9) < 1.0);
        assert!(
            text_value(-5.0) < f64::EPSILON,
            "a negative bm25 is not a match"
        );
    }
}
