//! Per-podcast refresh coalescing and the bounded `refresh_all` (ADR 0016).
//!
//! Concurrent refresh requests for one podcast share a single run: the
//! first caller spawns the work as a task (so it completes even if that
//! caller stops waiting) and every caller awaits the same shared result.
//! The entry is removed after the run has committed and published, so the
//! next request starts a fresh run. A request that joins an in-flight run
//! gets that run's report, including when it asked for `force` and the
//! running one did not.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt as _;
use futures_util::future::{BoxFuture, Shared};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use uguisu_core::UguisuError;
use uguisu_core::feed::RefreshReport;
use uguisu_core::ids::PodcastId;
use uguisu_http::CancellationToken;
use uguisu_storage::podcasts;

use crate::Engine;
use crate::refresh::RefreshOptions;

type SharedRun = Shared<BoxFuture<'static, Result<Arc<RefreshReport>, UguisuError>>>;

/// In-flight refreshes by podcast.
#[derive(Default)]
pub struct Coordinator {
    inflight: Mutex<HashMap<PodcastId, SharedRun>>,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator")
            .field("inflight", &self.inflight_count())
            .finish()
    }
}

impl Coordinator {
    /// Number of podcasts currently being refreshed.
    #[must_use]
    pub fn inflight_count(&self) -> usize {
        self.inflight.lock().map_or(0, |m| m.len())
    }

    /// Which podcasts are being refreshed, for a caller that wants to
    /// spend its capacity on something else (the scheduler).
    #[must_use]
    pub fn inflight_ids(&self) -> Vec<PodcastId> {
        self.inflight
            .lock()
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default()
    }

    fn remove(&self, id: PodcastId) {
        if let Ok(mut m) = self.inflight.lock() {
            m.remove(&id);
        }
    }
}

/// One podcast's result inside a [`Engine::refresh_all`] run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct RefreshAllEntry {
    /// The podcast.
    pub podcast_id: PodcastId,
    /// Its title at the time of the run.
    pub title: String,
    /// The report, when the refresh ran (its outcome may still be `failed`).
    pub report: Option<RefreshReport>,
    /// Why the refresh could not run at all.
    pub error: Option<UguisuError>,
}

impl RefreshAllEntry {
    /// The report or the error that replaced it.
    pub fn result(&self) -> Result<&RefreshReport, UguisuError> {
        match (&self.report, &self.error) {
            (Some(r), _) => Ok(r),
            (None, Some(e)) => Err(e.clone()),
            (None, None) => Err(UguisuError::Internal("entry without result".to_owned())),
        }
    }
}

impl Engine {
    /// Refreshes one podcast, joining an in-flight run for the same
    /// podcast when there is one.
    pub async fn refresh_podcast(
        &self,
        id: PodcastId,
        opts: RefreshOptions,
    ) -> Result<RefreshReport, UguisuError> {
        let run = {
            let mut map = self
                .coordinator()
                .inflight
                .lock()
                .map_err(|_| UguisuError::Internal("coordinator lock poisoned".to_owned()))?;
            if let Some(existing) = map.get(&id) {
                tracing::debug!(podcast = %id, "joining in-flight refresh");
                existing.clone()
            } else {
                let engine = self.clone();
                let handle = tokio::spawn(async move {
                    let result = engine.run_refresh(id, opts).await;
                    engine.coordinator().remove(id);
                    result.map(Arc::new)
                });
                let shared: SharedRun = handle
                    .map(|joined| match joined {
                        Ok(r) => r,
                        Err(e) => Err(UguisuError::Internal(format!("refresh task failed: {e}"))),
                    })
                    .boxed()
                    .shared();
                map.insert(id, shared.clone());
                shared
            }
        };
        run.await.map(|r| (*r).clone())
    }

    /// Refreshes every active or errored podcast with at most
    /// `concurrency` fetches in flight. Results are in title order.
    pub async fn refresh_all(
        &self,
        force: bool,
        concurrency: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<RefreshAllEntry>, UguisuError> {
        let targets: Vec<(PodcastId, String)> = {
            let mut conn = self.storage().reader().await?;
            let ids = podcasts::refreshable_ids(&mut conn).await?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let title = podcasts::get(&mut conn, id)
                    .await?
                    .map(|p| p.title)
                    .unwrap_or_default();
                out.push((id, title));
            }
            out
        };
        let limit = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut set = JoinSet::new();
        for (index, (id, title)) in targets.iter().cloned().enumerate() {
            let engine = self.clone();
            let limit = Arc::clone(&limit);
            let cancel = cancel.clone();
            set.spawn(async move {
                let _permit = limit.acquire_owned().await;
                let result = if cancel.is_cancelled() {
                    Err(UguisuError::Cancelled("refresh_all".to_owned()))
                } else {
                    engine
                        .refresh_podcast(
                            id,
                            RefreshOptions {
                                force,
                                cancel: Some(cancel),
                            },
                        )
                        .await
                };
                let (report, error) = match result {
                    Ok(r) => (Some(r), None),
                    Err(e) => (None, Some(e)),
                };
                (
                    index,
                    RefreshAllEntry {
                        podcast_id: id,
                        title,
                        report,
                        error,
                    },
                )
            });
        }
        let mut entries: Vec<Option<RefreshAllEntry>> = targets.iter().map(|_| None).collect();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((index, entry)) => entries[index] = Some(entry),
                Err(e) => {
                    return Err(UguisuError::Internal(format!("refresh task failed: {e}")));
                }
            }
        }
        Ok(entries.into_iter().flatten().collect())
    }
}
