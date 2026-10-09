//! Download queue endpoints (`docs/DOWNLOAD_ENGINE.md`):
//!
//! | Route | Purpose |
//! |---|---|
//! | `POST /api/v1/downloads` `{"episode_id": "…", "priority": "low\|normal\|high"}` | enqueue one episode (201 created, 200 existing / re-queued / already completed) |
//! | `POST /api/v1/podcasts/{id}/downloads` `{"priority": …}` | enqueue every downloadable episode of a podcast (summary) |
//! | `GET /api/v1/downloads?state=&podcast=&after=&limit=` | list jobs, newest first, keyset paging |
//! | `GET /api/v1/downloads/stats` | counts per state, running workers, global pause, next retry |
//! | `GET /api/v1/downloads/{id}` | one job with its attempts and live progress |
//! | `POST /api/v1/downloads/{id}/cancel` `…/pause` `…/resume` `…/retry` | job commands (409 when the state does not allow it, or a resume or retry of an episode whose file an import or a rebuild put in place) |
//! | `POST /api/v1/downloads/retry-failed` | queue every failed job again, except one whose episode's file an import or a rebuild put in place |
//! | `POST /api/v1/downloads/pause` / `…/resume` | pause or resume the whole queue (persisted) |
//! | `POST /api/v1/downloads/reconcile?deep=true` | reconcile the queue with the media directory |
//!
//! Workers run only when the process started them (`uguisu serve` does);
//! enqueueing through the API never starts one. Every JSON body carries
//! `schema: 1`; errors follow `library.rs`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uguisu_core::download::{DownloadState, PauseAllReason, Priority};
use uguisu_core::ids::{EpisodeId, JobId, PodcastId};
use uguisu_download::JobFilter;

use crate::extract::{Body, MaybeBody, Params};
use crate::library::{ApiResult, api_error, body, engine, parse_id};
use crate::{ApiError, AppState};

pub(crate) fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/downloads", get(list).post(enqueue))
        .route("/api/v1/downloads/stats", get(stats))
        .route("/api/v1/downloads/retry-failed", post(retry_failed))
        .route("/api/v1/downloads/pause", post(pause_all))
        .route("/api/v1/downloads/resume", post(resume_all))
        .route("/api/v1/downloads/reconcile", post(reconcile))
        .route("/api/v1/downloads/{id}", get(show))
        .route("/api/v1/downloads/{id}/cancel", post(cancel))
        .route("/api/v1/downloads/{id}/pause", post(pause))
        .route("/api/v1/downloads/{id}/resume", post(resume))
        .route("/api/v1/downloads/{id}/retry", post(retry))
        .route("/api/v1/podcasts/{id}/downloads", post(enqueue_podcast))
}

/// Body of `POST /api/v1/downloads`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct EnqueueBody {
    /// The episode to download.
    pub episode_id: String,
    /// Queue priority (default `normal`).
    #[serde(default)]
    pub priority: Priority,
}

/// Body of `POST /api/v1/podcasts/{id}/downloads`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct EnqueuePodcastBody {
    /// Queue priority (default `normal`).
    #[serde(default)]
    pub priority: Priority,
}

#[utoipa::path(
    post,
    path = "/api/v1/downloads",
    tag = "downloads",
    request_body = EnqueueBody,
    responses(
        (status = 201, description = "A new job", body = uguisu_download::EnqueueOutcome),
        (status = 200, description = "Existing, re-queued or already complete", body = uguisu_download::EnqueueOutcome),
        (status = 400, description = "Not an episode id, a malformed body, or an episode without a usable enclosure", body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "The episode cannot be downloaded: a candidate, skipped, gone from the feed, or its file, from an import or a rebuild, is in place", body = ApiError)
    )
)]
pub(crate) async fn enqueue(
    State(state): State<AppState>,
    Body(req): Body<EnqueueBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&req.episode_id, "episode")?;
    let outcome = engine
        .downloads()
        .enqueue_episode(id, req.priority)
        .await
        .map_err(|e| api_error(&e))?;
    let status = match outcome {
        uguisu_download::EnqueueOutcome::Created(_) => StatusCode::CREATED,
        _ => StatusCode::OK,
    };
    Ok(body(status, &outcome))
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/downloads",
    tag = "downloads",
    params(("id" = String, Path, description = "Podcast id")),
    request_body(content = EnqueuePodcastBody, description = "Optional; `normal` priority without one"),
    responses(
        (status = 200, body = uguisu_download::service::EnqueueSummary),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn enqueue_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
    MaybeBody(req): MaybeBody<EnqueuePodcastBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let priority = req.map_or(Priority::Normal, |b| b.priority);
    let summary = engine
        .downloads()
        .enqueue_podcast(id, priority)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &summary))
}

/// Query of `GET /api/v1/downloads`.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// Only jobs in this state.
    pub state: Option<String>,
    /// Only jobs of this podcast.
    pub podcast: Option<String>,
    /// Last job id of the previous page.
    pub after: Option<String>,
    /// Page size (1..=500).
    pub limit: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/downloads",
    tag = "downloads",
    params(ListParams),
    operation_id = "download_list",
    responses(
        (status = 200, body = uguisu_download::JobPage),
        (status = 400, description = "Unknown state, cursor or limit", body = ApiError)
    )
)]
pub(crate) async fn list(
    State(state): State<AppState>,
    Params(params): Params<ListParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let job_state = match params.state.as_deref() {
        None => None,
        Some(raw) => Some(DownloadState::parse(raw).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!(
                    "`{raw}` is not a download state (one of {})",
                    DownloadState::ALL
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })?),
    };
    let podcast_id: Option<PodcastId> = match params.podcast.as_deref() {
        Some(p) => Some(parse_id(p, "podcast")?),
        None => None,
    };
    let after: Option<JobId> = match params.after.as_deref() {
        Some(a) => Some(parse_id(a, "download job")?),
        None => None,
    };
    let limit = uguisu_core::page::limit(
        params.limit,
        uguisu_core::page::DEFAULT,
        uguisu_download::service::MAX_PAGE,
    )
    .map_err(|e| api_error(&e))?;
    let page = engine
        .downloads()
        .list(&JobFilter {
            state: job_state,
            podcast_id,
            after,
            limit,
        })
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &page))
}

#[utoipa::path(
    get,
    path = "/api/v1/downloads/stats",
    tag = "downloads",
    operation_id = "download_stats",
    responses((status = 200, body = uguisu_download::service::DownloadStats))
)]
pub(crate) async fn stats(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let summary = engine
        .downloads()
        .stats()
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &summary))
}

#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}",
    tag = "downloads",
    operation_id = "download_show",
    params(("id" = String, Path, description = "Job id")),
    responses(
        (status = 200, body = uguisu_download::JobDetail),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn show(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let engine = engine(&state)?;
    let id: JobId = parse_id(&id, "download job")?;
    let detail = engine
        .downloads()
        .job(id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &detail))
}

/// A job command's answer: the job after the command.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct JobBody {
    job: uguisu_core::download::DownloadJob,
}

macro_rules! job_command {
    ($name:ident, $path:literal, $id:literal) => {
        #[utoipa::path(
            post,
            path = $path,
            tag = "downloads",
            operation_id = $id,
            params(("id" = String, Path, description = "Job id")),
            responses(
                (status = 200, body = JobBody),
                (status = 404, body = ApiError),
                (status = 409, description = "The job's state does not allow it, or (resume, retry) the episode's file, from an import or a rebuild, is in place", body = ApiError)
            )
        )]
        pub(crate) async fn $name(
            State(state): State<AppState>,
            Path(id): Path<String>,
        ) -> ApiResult {
            let engine = engine(&state)?;
            let id: JobId = parse_id(&id, "download job")?;
            let job = engine
                .downloads()
                .$name(id)
                .await
                .map_err(|e| api_error(&e))?;
            Ok(body(StatusCode::OK, &JobBody { job }))
        }
    };
}

job_command!(cancel, "/api/v1/downloads/{id}/cancel", "download_cancel");
job_command!(pause, "/api/v1/downloads/{id}/pause", "download_pause");
job_command!(resume, "/api/v1/downloads/{id}/resume", "download_resume");
job_command!(retry, "/api/v1/downloads/{id}/retry", "download_retry");

/// Answer of `POST /api/v1/downloads/retry-failed`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct RetryFailedBody {
    requeued: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/downloads/retry-failed",
    tag = "downloads",
    responses((status = 200, body = RetryFailedBody))
)]
pub(crate) async fn retry_failed(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let requeued = engine
        .downloads()
        .retry_failed()
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &RetryFailedBody { requeued }))
}

/// Answer of the global pause/resume commands.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ControlBody {
    control: uguisu_core::download::DownloadControl,
}

#[utoipa::path(
    post,
    path = "/api/v1/downloads/pause",
    tag = "downloads",
    responses((status = 200, body = ControlBody))
)]
pub(crate) async fn pause_all(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let control = engine
        .downloads()
        .pause_all(PauseAllReason::User)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &ControlBody { control }))
}

#[utoipa::path(
    post,
    path = "/api/v1/downloads/resume",
    tag = "downloads",
    responses((status = 200, body = ControlBody))
)]
pub(crate) async fn resume_all(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let control = engine
        .downloads()
        .resume_all()
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &ControlBody { control }))
}

/// Query of `POST /api/v1/downloads/reconcile`.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReconcileParams {
    /// Also check every completed target and report unexpected files.
    #[serde(default)]
    pub deep: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/downloads/reconcile",
    tag = "downloads",
    operation_id = "download_reconcile",
    params(ReconcileParams),
    responses((status = 200, body = uguisu_download::service::ReconcileReport))
)]
pub(crate) async fn reconcile(
    State(state): State<AppState>,
    Params(params): Params<ReconcileParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let report = engine
        .downloads()
        .reconcile(params.deep)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &report))
}
