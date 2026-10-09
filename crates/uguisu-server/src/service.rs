//! Service endpoints:
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /api/v1/status` | what the daemon is doing: scheduler, queue, index, settings |
//! | `GET /api/v1/scheduler` | scheduler status |
//! | `POST /api/v1/scheduler/pause` `{"reason": "…"}` | stop automatic refreshing (persisted) |
//! | `POST /api/v1/scheduler/resume` | resume it |
//! | `POST /api/v1/scheduler/run` | run one pass now |
//! | `POST /api/v1/scheduler/maintenance` | run housekeeping now |
//! | `POST /api/v1/podcasts/{id}/pause` \| `/resume` | take one podcast out of the schedule, or back in |
//! | `POST /api/v1/podcasts/{id}/archive` | stop fetching a podcast for good and keep it (ADR 0055); `resume` undoes it |
//! | `POST /api/v1/podcasts/{id}/schedule` `{"at": "…"}` | set when it is next due (`null` = as soon as possible) |
//! | `GET /api/v1/settings` | every key with its value, origin and provenance |
//! | `PUT /api/v1/settings/{key}` `{"value": "…"}` | store a setting (409 when the environment pins it) |
//! | `DELETE /api/v1/settings/{key}` | clear a stored setting |
//! | `GET /api/v1/search?q=&limit=&prefix=` | local search over the library |
//! | `POST /api/v1/search/reindex` | rebuild the index |
//! | `GET /api/v1/discovery/records?limit=` | recorded resolutions |
//! | `GET /api/v1/discovery/records/{id}` | one recorded resolution |
//! | `POST /api/v1/db/backup` | a consistent copy of the database in the server's `backups/` directory (ADR 0056) |
//! | `POST /api/v1/db/check` | `integrity_check` and `foreign_key_check` |
//! | `POST /api/v1/db/vacuum` | `VACUUM`, and the size before and after |
//!
//! Every body carries `schema: 1`, like the rest of the API.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uguisu_core::ids::{DiscoveryRecordId, PodcastId};
use uguisu_engine::search::SearchRequest;

use crate::AppState;
use crate::extract::{Body, MaybeBody, Params};
use crate::library::{ApiResult, api_error, body, engine, parse_id};

pub(crate) fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/scheduler", get(scheduler))
        .route("/api/v1/scheduler/pause", post(pause))
        .route("/api/v1/scheduler/resume", post(resume))
        .route("/api/v1/scheduler/run", post(run_pass))
        .route("/api/v1/scheduler/maintenance", post(run_maintenance))
        .route("/api/v1/podcasts/{id}/pause", post(pause_podcast))
        .route("/api/v1/podcasts/{id}/resume", post(resume_podcast))
        .route("/api/v1/podcasts/{id}/archive", post(archive_podcast))
        .route("/api/v1/podcasts/{id}/schedule", post(schedule_podcast))
        .route("/api/v1/settings", get(list_settings))
        .route(
            "/api/v1/settings/{key}",
            put(set_setting).delete(clear_setting),
        )
        .route("/api/v1/search", get(search))
        .route("/api/v1/search/reindex", post(reindex))
        .route("/api/v1/discovery/records", get(list_records))
        .route("/api/v1/discovery/records/{id}", get(show_record))
        .route("/api/v1/db/backup", post(backup_database))
        .route("/api/v1/db/check", post(check_database))
        .route("/api/v1/db/vacuum", post(vacuum_database))
}

/// What the whole service is doing, in one request.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct Status {
    version: &'static str,
    scheduler: uguisu_engine::scheduler::SchedulerStatus,
    search: uguisu_core::search::SearchIndexStatus,
    downloads: uguisu_download::DownloadStats,
    podcasts: u64,
    /// Stored settings that are being kept but not used.
    settings_problems: usize,
}

#[utoipa::path(
    get,
    path = "/api/v1/status",
    tag = "service",
    responses((status = 200, body = Status))
)]
pub(crate) async fn status(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let scheduler = engine.scheduler_status().await.map_err(|e| api_error(&e))?;
    let search = engine
        .search_index_status()
        .await
        .map_err(|e| api_error(&e))?;
    let downloads = engine
        .downloads()
        .stats()
        .await
        .map_err(|e| api_error(&e))?;
    let podcasts = engine.podcast_count().await.map_err(|e| api_error(&e))?;
    let report = engine.settings_report();
    Ok(body(
        StatusCode::OK,
        &Status {
            version: uguisu_core::VERSION,
            scheduler,
            search,
            downloads,
            podcasts,
            settings_problems: report.rejected.len() + report.unused.len(),
        },
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/scheduler",
    tag = "scheduler",
    responses((status = 200, body = uguisu_engine::scheduler::SchedulerStatus))
)]
pub(crate) async fn scheduler(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let status = engine.scheduler_status().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &status))
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub(crate) struct PauseBody {
    /// Why, in the operator's own words.
    reason: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/scheduler/pause",
    tag = "scheduler",
    operation_id = "scheduler_pause",
    request_body(content = PauseBody, description = "Optional; no reason recorded without one"),
    responses((status = 200, body = uguisu_core::schedule::SchedulerControl))
)]
pub(crate) async fn pause(
    State(state): State<AppState>,
    MaybeBody(body_in): MaybeBody<PauseBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let reason = body_in.and_then(|b| b.reason);
    let control = engine
        .pause_scheduler(reason.as_deref())
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &control))
}

#[utoipa::path(
    post,
    path = "/api/v1/scheduler/resume",
    tag = "scheduler",
    operation_id = "scheduler_resume",
    responses((status = 200, body = uguisu_core::schedule::SchedulerControl))
)]
pub(crate) async fn resume(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let control = engine.resume_scheduler().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &control))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct PassBody {
    due: u32,
    started: u32,
    paused: bool,
}

/// Runs one scheduler pass now. The loop keeps its own cadence; this is
/// for an operator who does not want to wait for it.
#[utoipa::path(
    post,
    path = "/api/v1/scheduler/run",
    tag = "scheduler",
    responses((status = 202, description = "The pass was started", body = PassBody))
)]
pub(crate) async fn run_pass(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let concurrency = engine.config().feed.refresh_concurrency;
    let pass = engine
        .run_scheduler_pass(concurrency)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::ACCEPTED,
        &PassBody {
            due: pass.due,
            started: pass.started,
            paused: pass.paused,
        },
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/scheduler/maintenance",
    tag = "scheduler",
    responses((status = 200, body = uguisu_engine::scheduler::MaintenanceReport))
)]
pub(crate) async fn run_maintenance(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let report = engine.run_maintenance().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &report))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct PodcastSchedule {
    podcast_id: PodcastId,
    status: uguisu_core::model::PodcastStatus,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/pause",
    tag = "scheduler",
    params(("id" = String, Path, description = "Podcast id")),
    responses(
        (status = 200, body = PodcastSchedule),
        (status = 404, body = crate::ApiError),
        (status = 409, body = crate::ApiError)
    )
)]
pub(crate) async fn pause_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let status = engine.pause_podcast(id).await.map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &PodcastSchedule {
            podcast_id: id,
            status,
        },
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/resume",
    tag = "scheduler",
    params(("id" = String, Path, description = "Podcast id")),
    responses((status = 200, body = PodcastSchedule), (status = 404, body = crate::ApiError))
)]
pub(crate) async fn resume_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let status = engine.resume_podcast(id).await.map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &PodcastSchedule {
            podcast_id: id,
            status,
        },
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/archive",
    tag = "scheduler",
    params(("id" = String, Path, description = "Podcast id")),
    responses((status = 200, body = PodcastSchedule), (status = 404, body = crate::ApiError))
)]
pub(crate) async fn archive_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let status = engine
        .archive_podcast(id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &PodcastSchedule {
            podcast_id: id,
            status,
        },
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct ScheduleBody {
    /// When the podcast is next due, RFC 3339. `null` means "as soon as
    /// the scheduler looks"; `podcast refresh` is what "now" means.
    at: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/schedule",
    tag = "scheduler",
    params(("id" = String, Path, description = "Podcast id")),
    request_body = ScheduleBody,
    responses(
        (status = 200, body = uguisu_engine::scheduler::SchedulerStatus),
        (status = 400, body = crate::ApiError),
        (status = 404, body = crate::ApiError)
    )
)]
pub(crate) async fn schedule_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Body(request): Body<ScheduleBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let at = match request.at.as_deref() {
        Some(raw) => Some(OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| {
            crate::ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("`{raw}` is not an RFC 3339 timestamp"),
            )
        })?),
        None => None,
    };
    engine
        .reschedule_podcast(id, at)
        .await
        .map_err(|e| api_error(&e))?;
    let status = engine.scheduler_status().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &status))
}

#[utoipa::path(
    get,
    path = "/api/v1/settings",
    tag = "settings",
    responses((status = 200, description = "Every key, with secret values redacted", body = uguisu_engine::settings::SettingsReport))
)]
pub(crate) async fn list_settings(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    Ok(body(StatusCode::OK, &engine.settings_report()))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct SettingBody {
    /// The value, in the same syntax the environment variable uses.
    value: String,
    /// Who is making the change, for the audit column.
    updated_by: Option<String>,
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/{key}",
    tag = "settings",
    params(("key" = String, Path, description = "The `UGUISU_*` name")),
    request_body = SettingBody,
    responses(
        (status = 200, body = uguisu_core::config::KeyDescription),
        (status = 400, description = "Unknown key, unstorable key or invalid value", body = crate::ApiError),
        (status = 409, description = "The environment pins this key", body = crate::ApiError)
    )
)]
pub(crate) async fn set_setting(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Body(request): Body<SettingBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let described = engine
        .set_setting(
            &key,
            &request.value,
            request.updated_by.as_deref().or(Some("api")),
        )
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &described))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct Cleared {
    key: String,
    cleared: bool,
}

#[utoipa::path(
    delete,
    path = "/api/v1/settings/{key}",
    tag = "settings",
    params(("key" = String, Path, description = "The `UGUISU_*` name")),
    responses((status = 200, body = Cleared), (status = 400, body = crate::ApiError))
)]
pub(crate) async fn clear_setting(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let cleared = engine
        .unset_setting(&key, Some("api"))
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &Cleared { key, cleared }))
}

/// Query parameters of the library search endpoint.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LibrarySearchParams {
    /// What to search for.
    pub q: Option<String>,
    /// Hits of each kind (1..=200).
    pub limit: Option<u32>,
    /// Whether the last word matches as a prefix.
    #[serde(default)]
    pub prefix: bool,
    /// Which kinds to search: `podcasts`, `episodes` or both (default).
    pub kind: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/search",
    tag = "search",
    operation_id = "library_search",
    params(LibrarySearchParams),
    responses(
        (status = 200, body = uguisu_engine::search::SearchResults),
        (status = 400, body = crate::ApiError)
    )
)]
pub(crate) async fn search(
    State(state): State<AppState>,
    Params(params): Params<LibrarySearchParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let kind = params.kind.as_deref().unwrap_or("all");
    let request = SearchRequest {
        text: params.q.unwrap_or_default(),
        limit: params.limit.unwrap_or(uguisu_engine::search::DEFAULT_LIMIT),
        prefix: params.prefix,
        podcasts: matches!(kind, "all" | "podcasts"),
        episodes: matches!(kind, "all" | "episodes"),
    };
    let results = engine
        .search_library(&request)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &results))
}

#[utoipa::path(
    post,
    path = "/api/v1/search/reindex",
    tag = "search",
    responses((status = 200, body = uguisu_engine::search::ReindexReport))
)]
pub(crate) async fn reindex(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let report = engine.reindex_search().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &report))
}

/// Query parameters of the resolution-record endpoint.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RecordParams {
    /// How many to return (1..=500).
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct RecordList {
    records: Vec<uguisu_core::provider::DiscoveryRecord>,
}

#[utoipa::path(
    get,
    path = "/api/v1/discovery/records",
    tag = "discovery",
    params(RecordParams),
    responses((status = 200, body = RecordList))
)]
pub(crate) async fn list_records(
    State(state): State<AppState>,
    Params(params): Params<RecordParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let records = engine
        .discovery_records(params.limit.unwrap_or(50))
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &RecordList { records }))
}

#[utoipa::path(
    get,
    path = "/api/v1/discovery/records/{id}",
    tag = "discovery",
    params(("id" = String, Path, description = "Discovery record id")),
    responses(
        (status = 200, body = uguisu_core::provider::DiscoveryRecord),
        (status = 404, body = crate::ApiError)
    )
)]
pub(crate) async fn show_record(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: DiscoveryRecordId = parse_id(&id, "discovery record")?;
    let record = engine
        .discovery_record(id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &record))
}

// A request names no path: the server writes only into its own `backups/`,
// so a credential that reaches this route cannot make it write anywhere else
// (ADR 0056).
#[utoipa::path(
    post,
    path = "/api/v1/db/backup",
    tag = "service",
    responses((status = 200, body = uguisu_engine::db::DbBackup), (status = 409, body = crate::ApiError))
)]
pub(crate) async fn backup_database(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let backup = engine
        .backup_database(None)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &backup))
}

#[utoipa::path(
    post,
    path = "/api/v1/db/check",
    tag = "service",
    responses((status = 200, body = uguisu_engine::db::DbCheck))
)]
pub(crate) async fn check_database(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let check = engine.check_database().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &check))
}

#[utoipa::path(
    post,
    path = "/api/v1/db/vacuum",
    tag = "service",
    responses((status = 200, body = uguisu_engine::db::DbVacuum))
)]
pub(crate) async fn vacuum_database(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let vacuum = engine.vacuum_database().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &vacuum))
}
