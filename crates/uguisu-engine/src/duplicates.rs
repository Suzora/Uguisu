//! Candidate duplicates and their resolution by a person (ADR 0051).
//!
//! A refresh stores an item that contradicts a stored episode's identity as
//! a candidate (ADR 0014) and never resolves it. Here a person does: `same`
//! merges the candidate into the episode it duplicates, `separate` makes it
//! an episode of its own. Neither touches a file.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::ids::{ChangeId, EpisodeId, PodcastId};
use uguisu_core::model::{DuplicateResolution, Episode, EpisodeChange};
use uguisu_core::page;
use uguisu_core::{Event, EventKind, UguisuError};
use uguisu_storage::{archive_files, changes, episodes, events, podcasts};

use crate::Engine;
use crate::library::MAX_PAGE;

/// A candidate duplicate with the episode it probably duplicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DuplicatePair {
    /// The candidate, stored as skipped and never downloaded.
    pub candidate: Episode,
    /// The episode it probably duplicates (absent only for corrupt data).
    pub original: Option<Episode>,
}

/// One page of candidate duplicates, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DuplicatePage {
    /// The candidates.
    pub duplicates: Vec<DuplicatePair>,
    /// The cursor for the next page; absent on the last one.
    pub next_after: Option<EpisodeId>,
}

/// What a resolution did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DuplicateResolved {
    /// How the candidate was resolved.
    pub resolution: DuplicateResolution,
    /// The candidate; after `same` it no longer exists.
    pub candidate: EpisodeId,
    /// The episode it was a candidate duplicate of.
    pub original: EpisodeId,
    /// The episode that remains: the original after `same`, the candidate
    /// after `separate`.
    pub episode: Episode,
    /// Whether the archive policy queued the separated episode.
    pub queued: bool,
}

impl Engine {
    /// A page of candidate duplicates, of one podcast or of all. `after` is
    /// the last candidate of the previous page; `limit` is clamped to
    /// `1..=MAX_PAGE`.
    pub async fn duplicates(
        &self,
        podcast_id: Option<PodcastId>,
        after: Option<EpisodeId>,
        limit: u32,
    ) -> Result<DuplicatePage, UguisuError> {
        let limit = page::limit(Some(limit), page::DEFAULT, MAX_PAGE)?;
        let mut conn = self.storage().reader().await?;
        if let Some(id) = podcast_id
            && podcasts::get(&mut conn, id).await?.is_none()
        {
            return Err(UguisuError::NotFound {
                entity: "podcast".to_owned(),
                id: id.to_string(),
            });
        }
        if let Some(id) = after {
            let inside = episodes::get(&mut conn, id).await?.is_some_and(|e| {
                e.duplicate_of_episode_id.is_some() && podcast_id.is_none_or(|p| e.podcast_id == p)
            });
            if !inside {
                return Err(UguisuError::Invalid(format!("unknown cursor {id}")));
            }
        }
        let rows =
            episodes::duplicates(&mut conn, podcast_id, after, page::over_fetch(limit)).await?;
        let (candidates, next_after) = page::truncate(rows, limit, |e| e.id);
        let ids: Vec<EpisodeId> = candidates
            .iter()
            .filter_map(|e| e.duplicate_of_episode_id)
            .collect();
        let mut originals = episodes::get_many(&mut conn, &ids).await?;
        let duplicates = candidates
            .into_iter()
            .map(|candidate| {
                let original = originals
                    .iter()
                    .position(|o| Some(o.id) == candidate.duplicate_of_episode_id)
                    .map(|i| originals.swap_remove(i));
                DuplicatePair {
                    candidate,
                    original,
                }
            })
            .collect();
        Ok(DuplicatePage {
            duplicates,
            next_after,
        })
    }

    /// Resolves candidate duplicate `id` (ADR 0051).
    ///
    /// `same` is refused with [`UguisuError::Conflict`] when the candidate
    /// has an archived file of its own, or when the last fetch saw both
    /// episodes in the feed. Anything that is not a candidate is a conflict
    /// too.
    pub async fn resolve_duplicate(
        &self,
        id: EpisodeId,
        resolution: DuplicateResolution,
    ) -> Result<DuplicateResolved, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        let candidate = episodes::get(&mut tx, id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let Some(original_id) = candidate.duplicate_of_episode_id else {
            return Err(UguisuError::Conflict(format!(
                "episode {id} is not a candidate duplicate"
            )));
        };
        let original = episodes::get(&mut tx, original_id)
            .await?
            .ok_or_else(|| not_found(original_id))?;

        let mut log = Vec::new();
        let remains = match resolution {
            DuplicateResolution::Same => {
                if archive_files::get_by_episode(&mut tx, id).await?.is_some() {
                    return Err(UguisuError::Conflict(format!(
                        "episode {id} has an archived file of its own; keep both with `separate`"
                    )));
                }
                // Sightings are stored to the second, so the missed-fetch
                // streak breaks a tie: one fetch saw both items only when
                // both carry the same sighting and neither was missed since.
                let seen =
                    |e: &Episode| (e.last_seen_in_feed_at, std::cmp::Reverse(e.missing_streak));
                if seen(&candidate) == seen(&original) && candidate.missing_streak == 0 {
                    return Err(UguisuError::Conflict(format!(
                        "episodes {id} and {original_id} are both in the feed; \
                         they can be merged once one of them has left it"
                    )));
                }
                // A candidate has no download job to lose: the queue refuses
                // candidates, and an episode becomes one only on insert.
                let adopt = seen(&candidate) > seen(&original);
                let reason = format!("kept stored identity: merged candidate {id}");
                episodes::merge_into(&mut tx, id, original_id, adopt.then_some(&*reason), now)
                    .await?;
                if adopt && candidate.guid != original.guid {
                    log.push(change(
                        &original,
                        "guid",
                        original.guid.clone(),
                        candidate.guid.clone(),
                        now,
                    ));
                }
                original_id
            }
            DuplicateResolution::Separate => {
                episodes::separate(&mut tx, id, now).await?;
                id
            }
        };
        let other = if remains == id { original_id } else { id };
        let survivor = if remains == id { &candidate } else { &original };
        log.push(change(
            survivor,
            "duplicate_resolved",
            Some(other.to_string()),
            Some(resolution.as_str().to_owned()),
            now,
        ));
        changes::insert_all(&mut tx, &log).await?;
        let event = Event::now(
            Some(candidate.podcast_id),
            Some(remains),
            EventKind::EpisodeDuplicateResolved {
                candidate: id,
                original: original_id,
                resolution,
            },
        );
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));

        // As after a refresh: the resolution is stored either way, and a
        // queue that refuses must not undo it.
        let mut queued = false;
        if resolution == DuplicateResolution::Separate {
            match self.apply_policy(candidate.podcast_id, &[id]).await {
                Ok(outcome) => queued = outcome.queued > 0,
                Err(e) => {
                    tracing::warn!(episode_id = %id, error = %e, "archive policy could not run");
                }
            }
        }
        let mut conn = self.storage().reader().await?;
        let episode = episodes::get(&mut conn, remains)
            .await?
            .ok_or_else(|| not_found(remains))?;
        Ok(DuplicateResolved {
            resolution,
            candidate: id,
            original: original_id,
            episode,
            queued,
        })
    }
}

fn not_found(id: EpisodeId) -> UguisuError {
    UguisuError::NotFound {
        entity: "episode".to_owned(),
        id: id.to_string(),
    }
}

fn change(
    episode: &Episode,
    field: &str,
    old_value: Option<String>,
    new_value: Option<String>,
    now: OffsetDateTime,
) -> EpisodeChange {
    EpisodeChange {
        id: ChangeId::new(),
        episode_id: episode.id,
        podcast_id: episode.podcast_id,
        fetch_id: None,
        changed_at: now,
        field: field.to_owned(),
        old_value,
        new_value,
    }
}
