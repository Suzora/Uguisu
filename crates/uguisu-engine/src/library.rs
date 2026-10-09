//! Library service: adding podcasts, listing and showing them, paging
//! through episodes.
//!
//! Adding is idempotent: the input is resolved to a verified feed
//! and a podcast whose current source already uses that feed
//! URL (or canonical URL) is returned as is. A feed that carries a
//! `podcast:guid` another podcast already has is refused with a
//! [`UguisuError::Conflict`] naming the existing podcast, because two
//! sources for one show must be an explicit migration, not an accident.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::feed::{FeedFetch, RefreshReport};
use uguisu_core::ids::{EpisodeId, EventId, PodcastId, SourceId};
use uguisu_core::model::{Episode, FetchStatus, Podcast, PodcastSource, PodcastStatus, sort_title};
use uguisu_core::page;
use uguisu_core::redact;
use uguisu_core::{Event, EventKind, UguisuError};
use uguisu_discovery::resolve::{ResolveError, ResolveFailure, ResolvedFeed};
use uguisu_http::CancellationToken;
use uguisu_storage::podcasts::PodcastOrder;
use uguisu_storage::{
    archive_files, archive_policies, downloads, episodes, events, fetches, podcasts, sources,
};

use crate::Engine;
use crate::opml::PolicyDefaults;
use crate::refresh::RefreshOptions;

/// A podcast with its current source and a few counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastDetail {
    /// The podcast.
    pub podcast: Podcast,
    /// Its current feed source (absent only for corrupt data).
    pub source: Option<PodcastSource>,
    /// A feed URL the feed announces that failed the same-show check, with
    /// that check's outcome in its fetch state (ADR 0052).
    pub announced: Option<PodcastSource>,
    /// Episodes stored for it.
    pub episodes_total: u64,
    /// Episodes not detected as removed from the feed.
    pub episodes_present: u64,
    /// The newest fetch-log row of the current source.
    pub last_fetch: Option<FeedFetch>,
}

/// What removing a podcast took out of the library (ADR 0055).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastRemoval {
    /// The podcast that is gone.
    pub podcast_id: PodcastId,
    /// Its title.
    pub title: String,
    /// Episodes the library held for it.
    pub episodes: u64,
    /// Archived files it had; every one is still on disk.
    pub files: u64,
}

/// One episode with everything a reader needs about it.
///
/// Composed rather than joined: four reads by primary key, against three
/// `Option`s a caller would otherwise fetch separately and interleave wrongly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodeDetail {
    /// The episode, with its enclosures and extras.
    pub episode: Episode,
    /// Its podcast's title, so a page can be headed without a second request.
    pub podcast_title: String,
    /// The archive record, when the episode has been downloaded.
    pub archive: Option<uguisu_core::archive::ArchiveFile>,
    /// The download job, when one exists.
    pub job: Option<uguisu_core::download::DownloadJob>,
}

/// Which podcasts a list asks for, and in what order.
#[derive(Debug, Clone, Default)]
pub struct PodcastFilter {
    /// Only podcasts in this status.
    pub status: Option<PodcastStatus>,
    /// Only podcasts whose title contains this, case-insensitively; at most
    /// [`MAX_TITLE_QUERY`] characters.
    pub title: Option<String>,
    /// How to order the page.
    pub order: PodcastOrder,
}

/// One page of podcasts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PodcastPage {
    /// The podcasts.
    pub podcasts: Vec<PodcastDetail>,
    /// Cursor for the next page (`None` when this is the last one).
    pub next_after: Option<PodcastId>,
}

/// What adding a podcast produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AddOutcome {
    /// The podcast (new or already present).
    pub podcast: Podcast,
    /// Its current source.
    pub source: PodcastSource,
    /// `false` when the podcast already existed.
    pub created: bool,
    /// How the input was resolved.
    pub resolved: ResolvedFeed,
    /// The first refresh, when one ran.
    pub report: Option<RefreshReport>,
}

/// One page of episodes, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EpisodePage {
    /// The episodes.
    pub episodes: Vec<Episode>,
    /// Cursor for the next page (`None` when this is the last one).
    pub next_after: Option<EpisodeId>,
}

/// Largest page size accepted by [`Engine::episodes`] (ADR 0040).
pub const MAX_PAGE: u32 = page::MAX;

/// Longest title filter [`Engine::podcasts`] accepts, in characters.
pub const MAX_TITLE_QUERY: usize = 200;

/// Maps a resolution failure to the engine error type.
#[must_use]
pub fn resolve_error(input: &str, failure: ResolveFailure) -> UguisuError {
    let detail = failure.error.to_string();
    match failure.error {
        ResolveError::BlockedByPolicy { detail, .. } => UguisuError::BlockedByPolicy(detail),
        ResolveError::Cancelled => UguisuError::Cancelled("resolution".to_owned()),
        ResolveError::Network { .. } => UguisuError::Network {
            kind: uguisu_core::feed::FetchErrorKind::NetworkError,
            detail,
        },
        ResolveError::HttpStatus { status, .. } => UguisuError::Network {
            kind: uguisu_core::feed::FetchErrorKind::from_status(status)
                .unwrap_or(uguisu_core::feed::FetchErrorKind::HttpClientError),
            detail,
        },
        _ => UguisuError::Unresolvable {
            input: input.to_owned(),
            detail: format!("{detail}; {}", failure.error.suggestion()),
        },
    }
}

impl Engine {
    /// Resolves any input (feed URL, website, directory page) to a verified
    /// feed without storing anything.
    pub async fn resolve_input(
        &self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<ResolvedFeed, UguisuError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(UguisuError::Invalid("empty input".to_owned()));
        }
        let outcome = self.discovery().resolver.resolve(input, cancel).await;
        // Provenance, whichever way it went: "why did it pick that feed"
        // and "why did it refuse mine" are the same question asked at
        // different moments (ADR 0030).
        match &outcome {
            Ok(feed) => {
                self.record_resolution(input, Ok(feed), None).await;
            }
            Err(failure) => {
                self.record_resolution(input, Err(failure), None).await;
            }
        }
        outcome.map_err(|f| resolve_error(input, f))
    }

    /// [`resolve_input`](Self::resolve_input), also handing back the
    /// identifier of the record it wrote, so the caller can link the
    /// podcast it goes on to create.
    pub(crate) async fn resolve_input_recorded(
        &self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<(ResolvedFeed, uguisu_core::ids::DiscoveryRecordId), UguisuError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(UguisuError::Invalid("empty input".to_owned()));
        }
        let outcome = self.discovery().resolver.resolve(input, cancel).await;
        let record = match &outcome {
            Ok(feed) => self.record_resolution(input, Ok(feed), None).await,
            Err(failure) => self.record_resolution(input, Err(failure), None).await,
        };
        Ok((outcome.map_err(|f| resolve_error(input, f))?, record))
    }

    /// Resolves the input, stores the podcast (idempotent, see the module
    /// documentation) and runs the first refresh for a new podcast.
    pub async fn add_podcast(
        &self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<AddOutcome, UguisuError> {
        let resolved = self.resolve_input_recorded(input, cancel.clone()).await?;
        let record = resolved.1;
        let mut outcome = self.add_resolved(resolved.0).await?;
        self.attach_resolution(record, outcome.podcast.id).await;
        if outcome.created {
            let report = self
                .refresh_podcast(
                    outcome.podcast.id,
                    RefreshOptions {
                        force: false,
                        cancel: Some(cancel),
                    },
                )
                .await?;
            outcome.podcast = self.podcast(outcome.podcast.id).await?.podcast;
            outcome.report = Some(report);
        }
        Ok(outcome)
    }

    /// Stores an already resolved feed (used when a frontend asked the
    /// user to confirm the resolution first).
    pub async fn add_resolved(&self, resolved: ResolvedFeed) -> Result<AddOutcome, UguisuError> {
        self.insert_resolved(resolved, "direct", None).await
    }

    /// [`add_resolved`](Self::add_resolved), recording `provider` on the
    /// source and storing `policy` in the same transaction as a new podcast,
    /// so no refresh can run before it (ADR 0049). An existing podcast's
    /// policy is never touched.
    pub(crate) async fn insert_resolved(
        &self,
        resolved: ResolvedFeed,
        provider: &'static str,
        policy: Option<PolicyDefaults>,
    ) -> Result<AddOutcome, UguisuError> {
        let mut candidates: Vec<String> = vec![resolved.feed_url.to_string()];
        for u in [&resolved.canonical_url, &resolved.moved_to]
            .into_iter()
            .flatten()
        {
            let s = u.to_string();
            if !candidates.contains(&s) {
                candidates.push(s);
            }
        }

        let mut tx = self.storage().begin().await?;
        if let Some(existing) = sources::find_current_by_urls(&mut tx, &candidates)
            .await?
            .into_iter()
            .next()
        {
            let podcast = podcasts::get(&mut tx, existing.podcast_id)
                .await?
                .ok_or_else(|| {
                    UguisuError::Storage(format!(
                        "source {} points at a missing podcast {}",
                        existing.id, existing.podcast_id
                    ))
                })?;
            tx.rollback()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            tracing::info!(podcast = %podcast.id, feed = %redact::urls(existing.feed_url.as_str()), "podcast already present");
            return Ok(AddOutcome {
                podcast,
                source: existing,
                created: false,
                resolved,
                report: None,
            });
        }
        if let Some(guid) = resolved.podcast_guid.as_deref()
            && let Some(other) = podcasts::find_by_guid(&mut tx, guid).await?.first()
        {
            let other_url = sources::current(&mut tx, other.id)
                .await?
                .map_or_else(|| "unknown feed".to_owned(), |s| s.feed_url.to_string());
            tx.rollback()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            return Err(UguisuError::Conflict(format!(
                "podcast {} ({}) already carries podcast:guid {guid} under {other_url}; \
                 {} looks like a second feed of the same show",
                other.id, other.title, resolved.feed_url
            )));
        }

        let now = OffsetDateTime::now_utc();
        let podcast = podcast_from(&resolved, now);
        let source = PodcastSource {
            id: SourceId::new(),
            podcast_id: podcast.id,
            feed_url: resolved.feed_url.clone(),
            canonical_url: resolved.canonical_url.clone(),
            website_url: resolved.website.clone(),
            provider: provider.to_owned(),
            provider_ref: None,
            discovered_at: now,
            verified_at: Some(resolved.verified_at),
            is_current: true,
            replaced_by_source_id: None,
            replacement_reason: None,
            fetch: FetchStatus::default(),
            created_at: now,
            updated_at: now,
        };
        podcasts::insert(&mut tx, &podcast).await?;
        sources::insert(&mut tx, &source).await?;
        if let Some(policy) = policy {
            archive_policies::set(&mut tx, &policy.for_podcast(podcast.id, now)).await?;
        }
        let added = vec![Event::now(
            Some(podcast.id),
            None,
            EventKind::PodcastAdded {
                title: podcast.title.clone(),
                feed_url: source.feed_url.clone(),
                source_id: source.id,
            },
        )];
        events::insert_all(&mut tx, &added).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(&added);
        tracing::info!(podcast = %podcast.id, title = %podcast.title, feed = %redact::urls(source.feed_url.as_str()), "podcast added");
        Ok(AddOutcome {
            podcast,
            source,
            created: true,
            resolved,
            report: None,
        })
    }

    /// Every podcast with its current source and counters, sorted by title.
    ///
    /// Reads the whole library in pages, which is what makes it four queries
    /// per page rather than three per podcast. The CLI's `podcast list` is the
    /// caller that wants all of them at once.
    pub async fn list_podcasts(&self) -> Result<Vec<PodcastDetail>, UguisuError> {
        let mut out = Vec::new();
        let mut after: Option<PodcastId> = None;
        loop {
            let page = self
                .podcasts(&PodcastFilter::default(), after, page::MAX)
                .await?;
            let more = page.next_after;
            out.extend(page.podcasts);
            match more {
                Some(cursor) => after = Some(cursor),
                None => return Ok(out),
            }
        }
    }

    /// Number of podcasts, without reading any of them.
    pub async fn podcast_count(&self) -> Result<u64, UguisuError> {
        let mut conn = self.storage().reader().await?;
        Ok(podcasts::count(&mut conn).await?)
    }

    /// One page of podcasts with their sources and counters.
    ///
    /// Four queries, whatever the page size: the page itself, then the current
    /// sources, the episode counts and the newest fetches for exactly the rows
    /// on it. It used to be one query plus three per podcast — two of which
    /// were full scans — so listing a library was quadratic in its size and
    /// `GET /api/v1/status` paid for all of it to take a `.len()`.
    pub async fn podcasts(
        &self,
        filter: &PodcastFilter,
        after: Option<PodcastId>,
        limit: u32,
    ) -> Result<PodcastPage, UguisuError> {
        let limit = page::limit(Some(limit), page::DEFAULT, page::MAX)?;
        if let Some(title) = &filter.title
            && title.chars().count() > MAX_TITLE_QUERY
        {
            return Err(UguisuError::Invalid(format!(
                "a title filter is at most {MAX_TITLE_QUERY} characters"
            )));
        }
        let mut conn = self.storage().reader().await?;
        let cursor = match after {
            None => None,
            Some(id) => Some(
                podcasts::get(&mut conn, id)
                    .await?
                    .ok_or_else(|| UguisuError::Invalid(format!("unknown cursor {id}")))?,
            ),
        };
        // A cursor from a differently-filtered list would silently offset this
        // one by an unrelated row's sort key (ADR 0040).
        if let Some(cursor) = &cursor
            && (filter.status.is_some_and(|s| cursor.status != s)
                || filter
                    .title
                    .as_ref()
                    .is_some_and(|t| !podcasts::title_matches(cursor, t)))
        {
            return Err(UguisuError::Invalid(format!(
                "cursor {} is not in this list",
                cursor.id
            )));
        }
        let rows = podcasts::page(
            &mut conn,
            filter.status,
            filter.title.as_deref(),
            filter.order,
            cursor.as_ref(),
            page::over_fetch(limit),
        )
        .await?;
        let (rows, next_after) = page::truncate(rows, limit, |p| p.id);

        let ids: Vec<PodcastId> = rows.iter().map(|p| p.id).collect();
        let mut sources = sources::current_many(&mut conn, &ids).await?;
        let mut announced = sources::announced_many(&mut conn, &ids).await?;
        let counts = episodes::counts_many(&mut conn, &ids).await?;
        let source_ids: Vec<uguisu_core::ids::SourceId> = sources.values().map(|s| s.id).collect();
        let mut fetches = fetches::latest_for_sources(&mut conn, &source_ids).await?;

        let podcasts = rows
            .into_iter()
            .map(|podcast| {
                let source = sources.remove(&podcast.id);
                let last_fetch = source.as_ref().and_then(|s| fetches.remove(&s.id));
                let (episodes_total, episodes_present) =
                    counts.get(&podcast.id).copied().unwrap_or((0, 0));
                PodcastDetail {
                    announced: announced.remove(&podcast.id),
                    podcast,
                    source,
                    episodes_total,
                    episodes_present,
                    last_fetch,
                }
            })
            .collect();
        Ok(PodcastPage {
            podcasts,
            next_after,
        })
    }

    /// One episode with its podcast's title, its archive record and its job.
    pub async fn episode(&self, id: EpisodeId) -> Result<EpisodeDetail, UguisuError> {
        let mut conn = self.storage().reader().await?;
        let episode = episodes::get(&mut conn, id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "episode".to_owned(),
                id: id.to_string(),
            })?;
        let podcast_title = podcasts::get(&mut conn, episode.podcast_id)
            .await?
            .map(|p| p.title)
            .unwrap_or_default();
        drop(conn);
        Ok(EpisodeDetail {
            archive: self.archive_file(id).await?,
            job: self.downloads().job_for_episode(id).await?,
            episode,
            podcast_title,
        })
    }

    /// Removes a podcast from the library and keeps every file (ADR 0055).
    /// Refused while a refresh of it or one of its downloads is running.
    pub async fn remove_podcast(&self, id: PodcastId) -> Result<PodcastRemoval, UguisuError> {
        if self.coordinator().inflight_ids().contains(&id) {
            return Err(UguisuError::Conflict(format!(
                "podcast {id} is being refreshed; remove it when that is done"
            )));
        }
        let mut tx = self.storage().begin().await?;
        let podcast = podcasts::get(&mut tx, id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: id.to_string(),
            })?;
        if downloads::running_for_podcast(&mut tx, id).await? > 0 {
            return Err(UguisuError::Conflict(format!(
                "a download of {} is running; cancel it or wait for it",
                podcast.title
            )));
        }
        let feed_url = sources::current(&mut tx, id).await?.map(|s| s.feed_url);
        let (episodes, _) = episodes::counts(&mut tx, id).await?;
        let files = archive_files::count_for_podcast(&mut tx, id).await?;
        podcasts::delete(&mut tx, id).await?;
        let event = Event::now(
            Some(id),
            None,
            EventKind::PodcastRemoved {
                title: podcast.title.clone(),
                feed_url,
                episodes,
                files,
            },
        );
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));
        Ok(PodcastRemoval {
            podcast_id: id,
            title: podcast.title,
            episodes,
            files,
        })
    }

    /// One podcast with its current source and counters.
    pub async fn podcast(&self, id: PodcastId) -> Result<PodcastDetail, UguisuError> {
        let mut conn = self.storage().reader().await?;
        let podcast = podcasts::get(&mut conn, id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: id.to_string(),
            })?;
        detail(&mut conn, podcast).await
    }

    /// A source (current or historical) with its newest fetch-log row.
    pub async fn source(
        &self,
        id: SourceId,
    ) -> Result<(PodcastSource, Option<FeedFetch>), UguisuError> {
        let mut conn = self.storage().reader().await?;
        let source = sources::get(&mut conn, id)
            .await?
            .ok_or_else(|| UguisuError::NotFound {
                entity: "source".to_owned(),
                id: id.to_string(),
            })?;
        let last = fetches::latest_for_source(&mut conn, id).await?;
        Ok((source, last))
    }

    /// Every source of a podcast, current first.
    pub async fn sources(&self, id: PodcastId) -> Result<Vec<PodcastSource>, UguisuError> {
        let mut conn = self.storage().reader().await?;
        Ok(sources::history(&mut conn, id).await?)
    }

    /// A page of episodes, newest first. `after` is the last id of the
    /// previous page; `limit` is clamped to `1..=MAX_PAGE`.
    pub async fn episodes(
        &self,
        podcast_id: PodcastId,
        after: Option<EpisodeId>,
        limit: u32,
    ) -> Result<EpisodePage, UguisuError> {
        let limit = page::limit(Some(limit), page::DEFAULT, MAX_PAGE)?;
        let mut conn = self.storage().reader().await?;
        if podcasts::get(&mut conn, podcast_id).await?.is_none() {
            return Err(UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: podcast_id.to_string(),
            });
        }
        let cursor = match after {
            None => None,
            Some(id) => {
                let e = episodes::get(&mut conn, id)
                    .await?
                    .filter(|e| e.podcast_id == podcast_id)
                    .ok_or_else(|| UguisuError::Invalid(format!("unknown cursor {id}")))?;
                Some((e.sort_at, e.id))
            }
        };
        let rows = episodes::page(&mut conn, podcast_id, cursor, page::over_fetch(limit)).await?;
        let (episodes, next_after) = page::truncate(rows, limit, |e| e.id);
        Ok(EpisodePage {
            episodes,
            next_after,
        })
    }

    /// Fetch-log rows of a podcast, newest first.
    pub async fn fetch_log(
        &self,
        podcast_id: PodcastId,
        limit: u32,
    ) -> Result<Vec<FeedFetch>, UguisuError> {
        let mut conn = self.storage().reader().await?;
        Ok(fetches::list(&mut conn, podcast_id, limit).await?)
    }

    /// Stored events after `after`, or the newest ones without it; oldest
    /// first either way.
    pub async fn stored_events(
        &self,
        after: Option<EventId>,
        limit: u32,
    ) -> Result<Vec<Event>, UguisuError> {
        let mut conn = self.storage().reader().await?;
        Ok(match after {
            Some(_) => events::list_after(&mut conn, after, limit).await?,
            None => events::list_latest(&mut conn, limit).await?,
        })
    }
}

async fn detail(
    conn: &mut uguisu_storage::SqliteConnection,
    podcast: Podcast,
) -> Result<PodcastDetail, UguisuError> {
    let source = sources::current(conn, podcast.id).await?;
    let announced = sources::announced(conn, podcast.id).await?;
    let (episodes_total, episodes_present) = episodes::counts(conn, podcast.id).await?;
    let last_fetch = match &source {
        Some(s) => fetches::latest_for_source(conn, s.id).await?,
        None => None,
    };
    Ok(PodcastDetail {
        podcast,
        source,
        announced,
        episodes_total,
        episodes_present,
        last_fetch,
    })
}

/// The initial podcast row from a resolved feed; the first refresh fills
/// in everything the probe did not extract.
fn podcast_from(resolved: &ResolvedFeed, now: OffsetDateTime) -> Podcast {
    let title = resolved
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(
            || {
                resolved
                    .feed_url
                    .host_str()
                    .map_or_else(|| "Untitled podcast".to_owned(), str::to_owned)
            },
            str::to_owned,
        );
    Podcast {
        id: PodcastId::new(),
        sort_title: sort_title(&title),
        title,
        subtitle: None,
        author: resolved.author.clone(),
        publisher: None,
        owner_name: None,
        owner_email: None,
        description_html: None,
        description_text: resolved.description.clone(),
        website: resolved.website.clone(),
        artwork_url: resolved.artwork.clone(),
        language: resolved.language.clone(),
        categories: Vec::new(),
        explicit: None,
        copyright: None,
        podcast_guid: resolved.podcast_guid.clone(),
        feed_kind: resolved.feed_kind,
        status: PodcastStatus::Active,
        refresh_interval_secs: None,
        next_refresh_at: None,
        last_refresh_at: None,
        last_error: None,
        directory_name: None,
        metadata_hash: String::new(),
        created_at: now,
        updated_at: now,
    }
}
