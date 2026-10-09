//! Library and feed endpoints:
//!
//! | Route | Purpose |
//! |---|---|
//! | `POST /api/v1/podcasts` `{"input": "…"}` | resolve, add and refresh a podcast (201 new, 200 existing, 409 conflict) |
//! | `GET /api/v1/podcasts` | list podcasts with counters |
//! | `GET /api/v1/podcasts/opml` | every podcast as an OPML file (ADR 0049) |
//! | `POST /api/v1/podcasts/opml` `{"opml": "…", "apply": true}` | plan an OPML import, or add its new feeds |
//! | `GET /api/v1/podcasts/{id}` | one podcast |
//! | `DELETE /api/v1/podcasts/{id}` (or `POST …/remove`) | remove a podcast from the library, keeping every file (ADR 0055; 409 while a download of it runs) |
//! | `GET /api/v1/podcasts/{id}/episodes?after=&limit=` | episodes, newest first, keyset paging |
//! | `GET /api/v1/episodes/duplicates?podcast=&after=&limit=` | candidate duplicates with their originals, newest first (ADR 0051) |
//! | `POST /api/v1/episodes/{id}/resolve` `{"resolution": "same"}` | resolve a candidate: `same` merges it, `separate` keeps both |
//! | `POST /api/v1/podcasts/{id}/refresh?force=true` | refresh one podcast (report) |
//! | `POST /api/v1/podcasts/{id}/move-feed` `{"url": "…", "dry_run": false, "force": false}` | move a podcast to another feed URL (ADR 0052) |
//! | `POST /api/v1/podcasts/refresh?force=true` | refresh every active podcast |
//! | `GET /api/v1/feeds/{source_id}/status` | fetch state of one source |
//! | `GET /api/v1/feeds/inspect?url=` | parse a feed without storing anything |
//! | `GET /api/v1/events?exclude=download.progress` | live domain events (SSE, `exclude` drops a comma-separated list of kinds); `?after=<id>&limit=n` lists stored events |
//!
//! Every JSON body carries `schema: 1`; the OPML export is a file, not a
//! JSON body. Errors are `{ "error": { "kind", "message" } }`
//! with the engine's error kind.

use std::convert::Infallible;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uguisu_core::UguisuError;
use uguisu_core::archive::ArchiveErrorKind;
use uguisu_core::ids::{EpisodeId, EventId, PodcastId, SourceId};
use uguisu_core::model::{DuplicateResolution, PodcastStatus};
use uguisu_engine::RefreshOptions;
use uguisu_engine::library::{MAX_PAGE, PodcastFilter};
use uguisu_engine::migration::MoveOptions;
use uguisu_engine::opml::{OpmlImport, OpmlOptions, PolicyDefaults};
use uguisu_storage::podcasts::PodcastOrder;

use crate::extract::{Body, Params};
use crate::{ApiError, AppState};

/// Schema version of every library response body.
pub const SCHEMA: u32 = 1;

pub(crate) fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/podcasts", get(list_podcasts).post(add_podcast))
        .route("/api/v1/podcasts/refresh", post(refresh_all))
        .route("/api/v1/podcasts/opml", get(export_opml).post(import_opml))
        .route(
            "/api/v1/podcasts/{id}",
            get(show_podcast).delete(remove_podcast),
        )
        .route("/api/v1/podcasts/{id}/episodes", get(list_episodes))
        .route("/api/v1/episodes/duplicates", get(list_duplicates))
        .route("/api/v1/episodes/{id}", get(show_episode))
        .route("/api/v1/episodes/{id}/resolve", post(resolve_duplicate))
        .route("/api/v1/podcasts/{id}/refresh", post(refresh_podcast))
        .route("/api/v1/podcasts/{id}/move-feed", post(move_feed))
        .route("/api/v1/podcasts/{id}/remove", post(remove_podcast))
        .route("/api/v1/feeds/inspect", get(inspect))
        .route("/api/v1/feeds/{source_id}/status", get(feed_status))
        .route("/api/v1/events", get(events))
}

pub(crate) type ApiResult = Result<Response, (StatusCode, Json<ApiError>)>;

/// HTTP status for an engine error.
#[must_use]
pub fn status_for(e: &UguisuError) -> StatusCode {
    match e {
        UguisuError::NotFound { .. } => StatusCode::NOT_FOUND,
        UguisuError::Conflict(_) => StatusCode::CONFLICT,
        UguisuError::BlockedByPolicy(_) => StatusCode::FORBIDDEN,
        UguisuError::Invalid(_) => StatusCode::BAD_REQUEST,
        UguisuError::Unresolvable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        UguisuError::Network { .. } | UguisuError::Feed { .. } => StatusCode::BAD_GATEWAY,
        UguisuError::Locked(_) => StatusCode::SERVICE_UNAVAILABLE,
        UguisuError::Cancelled(_) => StatusCode::REQUEST_TIMEOUT,
        UguisuError::DiskFull { .. } => StatusCode::INSUFFICIENT_STORAGE,
        // An archive problem is a conflict between the record and the disk,
        // except where the input itself is wrong or the file system failed.
        UguisuError::Archive { kind, .. } => match kind {
            ArchiveErrorKind::ArchiveNotFound | ArchiveErrorKind::SidecarMissing => {
                StatusCode::NOT_FOUND
            }
            ArchiveErrorKind::PathInvalid
            | ArchiveErrorKind::TemplateInvalid
            | ArchiveErrorKind::PolicyInvalid
            | ArchiveErrorKind::ImportSourceInvalid => StatusCode::BAD_REQUEST,
            ArchiveErrorKind::ArchiveMissing
            | ArchiveErrorKind::ArchiveInvalid
            | ArchiveErrorKind::HashMismatch
            | ArchiveErrorKind::SizeMismatch
            | ArchiveErrorKind::PathCollision
            | ArchiveErrorKind::PolicyConflict
            | ArchiveErrorKind::SidecarInvalid
            | ArchiveErrorKind::ManifestInvalid => StatusCode::CONFLICT,
            // Understood, well-formed, and still not something Uguisu will
            // act on: an ambiguous import would have to guess, and a
            // container that carries no tags cannot be made to.
            ArchiveErrorKind::ImportAmbiguous
            | ArchiveErrorKind::ImportUnmatched
            | ArchiveErrorKind::ArtworkInvalid
            | ArchiveErrorKind::TagsUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
            ArchiveErrorKind::RelocationFailed
            | ArchiveErrorKind::VerificationIo
            | ArchiveErrorKind::TagsFailed => StatusCode::INTERNAL_SERVER_ERROR,
        },
        UguisuError::Storage(_)
        | UguisuError::Config(_)
        | UguisuError::Io { .. }
        | UguisuError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub(crate) fn api_error(e: &UguisuError) -> (StatusCode, Json<ApiError>) {
    ApiError::new(status_for(e), e.kind(), e.to_string())
}

pub(crate) fn engine(
    state: &AppState,
) -> Result<&uguisu_engine::Engine, (StatusCode, Json<ApiError>)> {
    state.engine.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine_unavailable",
            "this server runs without a library (no data directory)",
        )
    })
}

pub(crate) fn parse_id<T>(
    raw: &str,
    entity: &str,
) -> Result<uguisu_core::ids::Id<T>, (StatusCode, Json<ApiError>)> {
    raw.parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("`{raw}` is not a valid {entity} id"),
        )
    })
}

/// Serializes `value` with `schema` at the top level.
///
/// `schema` versions the body, so a payload that is itself a versioned
/// document keeps its own number: `Inspection`, `RefreshReport` and `Sidecar`
/// each carry one, and replacing it with the API's — which is what this used
/// to do — would claim the document is a version it is not. Every other body
/// is an API envelope with no version of its own and gets this one.
///
/// A payload that will not serialize or is not a JSON object is a bug here,
/// not something a client did. Both used to pass silently: the first answered
/// `200 OK` with `null`, the second dropped the version without complaint.
pub(crate) fn body<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::Object(mut map)) => {
            map.entry("schema").or_insert_with(|| SCHEMA.into());
            (status, Json(serde_json::Value::Object(map))).into_response()
        }
        Ok(_) => unbuildable("the payload is not a JSON object"),
        Err(e) => unbuildable(&format!("the payload did not serialize: {e}")),
    }
}

fn unbuildable(reason: &str) -> Response {
    tracing::error!(reason, "cannot build a response body");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "the server could not build this response",
    )
    .into_response()
}

/// Body of `POST /api/v1/podcasts`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddBody {
    /// Feed URL, website or directory page.
    pub input: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts",
    tag = "library",
    request_body = AddBody,
    responses(
        (status = 201, description = "Added and refreshed", body = uguisu_engine::library::AddOutcome),
        (status = 200, description = "Already in the library", body = uguisu_engine::library::AddOutcome),
        (status = 409, body = ApiError),
        (status = 422, description = "Nothing resolvable at that input", body = ApiError)
    )
)]
pub(crate) async fn add_podcast(
    State(state): State<AppState>,
    Body(req): Body<AddBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let outcome = engine
        .add_podcast(&req.input, CancellationToken::new())
        .await
        .map_err(|e| api_error(&e))?;
    let status = if outcome.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(body(status, &outcome))
}

#[utoipa::path(
    get,
    path = "/api/v1/podcasts/opml",
    tag = "library",
    operation_id = "export_opml",
    responses(
        (status = 200, description = "Every podcast as a flat OPML 2.0 file", content_type = "text/x-opml", body = String)
    )
)]
pub(crate) async fn export_opml(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let document = engine.export_opml().await.map_err(|e| api_error(&e))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/x-opml; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"uguisu.opml\"",
            ),
        ],
        document,
    )
        .into_response())
}

/// Body of `POST /api/v1/podcasts/opml`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OpmlImportRequest {
    /// The OPML document, as text; at most 1 MiB.
    pub opml: String,
    /// Verify and add the new feeds. Absent means a dry run, which sends no request.
    pub apply: Option<bool>,
    /// Stored with each podcast the import adds; absent leaves the global defaults.
    pub policy: Option<crate::archive::PolicyUpdate>,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/opml",
    tag = "library",
    operation_id = "import_opml",
    request_body = OpmlImportRequest,
    responses(
        (status = 200, description = "The plan, or what applying it did", body = OpmlImport),
        (status = 400, description = "Not an OPML document, or an unknown policy mode", body = ApiError)
    )
)]
pub(crate) async fn import_opml(
    State(state): State<AppState>,
    Body(req): Body<OpmlImportRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let policy = req
        .policy
        .as_ref()
        .map(|p| {
            Ok::<_, (StatusCode, Json<ApiError>)>(PolicyDefaults {
                mode: p.mode()?,
                max_backlog: p.max_backlog,
                max_age_days: p.max_age_days,
                priority: p.priority,
            })
        })
        .transpose()?;
    let options = OpmlOptions {
        apply: req.apply.unwrap_or(false),
        policy,
    };
    let report = engine
        .import_opml(&req.opml, options, CancellationToken::new())
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &report))
}

/// Query of `GET /api/v1/podcasts`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PodcastParams {
    /// Only podcasts in this status.
    pub status: Option<String>,
    /// Only podcasts whose title contains this, case-insensitively (at most 200 characters).
    pub q: Option<String>,
    /// `title` (default), `added`, `refreshed` or `episodes`.
    pub sort: Option<String>,
    /// Last podcast id of the previous page.
    pub after: Option<String>,
    /// Page size (1..=500).
    pub limit: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/podcasts",
    tag = "library",
    params(PodcastParams),
    responses(
        (status = 200, body = uguisu_engine::library::PodcastPage),
        (status = 400, description = "Unknown status, sort or cursor, or `q` too long", body = ApiError)
    )
)]
pub(crate) async fn list_podcasts(
    State(state): State<AppState>,
    Params(params): Params<PodcastParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let status = match params.status.as_deref() {
        None => None,
        Some(raw) => Some(PodcastStatus::parse(raw).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("`{raw}` is not a podcast status (active, paused, error, archived)"),
            )
        })?),
    };
    let order = match params.sort.as_deref() {
        None | Some("title") => PodcastOrder::Title,
        Some("added") => PodcastOrder::Added,
        Some("refreshed") => PodcastOrder::Refreshed,
        Some("episodes") => PodcastOrder::Episodes,
        Some(raw) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("`{raw}` is not a sort (title, added, refreshed, episodes)"),
            ));
        }
    };
    let title = params
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_owned);
    let after: Option<PodcastId> = match params.after.as_deref() {
        Some(a) => Some(parse_id(a, "podcast")?),
        None => None,
    };
    let limit = uguisu_core::page::limit(params.limit, uguisu_core::page::DEFAULT, MAX_PAGE)
        .map_err(|e| api_error(&e))?;
    let page = engine
        .podcasts(
            &PodcastFilter {
                status,
                title,
                order,
            },
            after,
            limit,
        )
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &page))
}

#[utoipa::path(
    get,
    path = "/api/v1/podcasts/{id}",
    tag = "library",
    params(("id" = String, Path, description = "Podcast id")),
    responses(
        (status = 200, body = uguisu_engine::library::PodcastDetail),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn show_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let detail = engine.podcast(id).await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &detail))
}

#[utoipa::path(
    delete,
    path = "/api/v1/podcasts/{id}",
    tag = "library",
    params(("id" = String, Path, description = "Podcast id")),
    responses(
        (status = 200, body = uguisu_engine::library::PodcastRemoval),
        (status = 404, body = ApiError),
        (status = 409, body = ApiError)
    )
)]
pub(crate) async fn remove_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let removed = engine.remove_podcast(id).await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &removed))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EpisodeParams {
    /// Last episode id of the previous page.
    pub after: Option<String>,
    /// Page size (1..=500).
    pub limit: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/podcasts/{id}/episodes",
    tag = "library",
    params(("id" = String, Path, description = "Podcast id"), EpisodeParams),
    responses(
        (status = 200, body = uguisu_engine::library::EpisodePage),
        (status = 400, description = "Bad cursor or limit", body = ApiError),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn list_episodes(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Params(params): Params<EpisodeParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let after: Option<EpisodeId> = match params.after.as_deref() {
        Some(a) => Some(parse_id(a, "episode")?),
        None => None,
    };
    let limit = uguisu_core::page::limit(params.limit, uguisu_core::page::DEFAULT, MAX_PAGE)
        .map_err(|e| api_error(&e))?;
    let page = engine
        .episodes(id, after, limit)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &page))
}

#[utoipa::path(
    get,
    path = "/api/v1/episodes/{id}",
    tag = "library",
    params(("id" = String, Path, description = "Episode id")),
    responses(
        (status = 200, body = uguisu_engine::library::EpisodeDetail),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn show_episode(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: uguisu_core::ids::EpisodeId = parse_id(&id, "episode")?;
    let detail = engine.episode(id).await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &detail))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DuplicateParams {
    /// Only this podcast's candidates.
    pub podcast: Option<String>,
    /// Last candidate id of the previous page.
    pub after: Option<String>,
    /// Page size (1..=500).
    pub limit: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/episodes/duplicates",
    tag = "library",
    params(DuplicateParams),
    responses(
        (status = 200, body = uguisu_engine::duplicates::DuplicatePage),
        (status = 400, description = "Bad cursor or limit", body = ApiError),
        (status = 404, description = "Unknown podcast", body = ApiError)
    )
)]
pub(crate) async fn list_duplicates(
    State(state): State<AppState>,
    Params(params): Params<DuplicateParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast: Option<PodcastId> = match params.podcast.as_deref() {
        Some(p) => Some(parse_id(p, "podcast")?),
        None => None,
    };
    let after: Option<EpisodeId> = match params.after.as_deref() {
        Some(a) => Some(parse_id(a, "episode")?),
        None => None,
    };
    let limit = uguisu_core::page::limit(params.limit, uguisu_core::page::DEFAULT, MAX_PAGE)
        .map_err(|e| api_error(&e))?;
    let page = engine
        .duplicates(podcast, after, limit)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &page))
}

/// Body of `POST /api/v1/episodes/{id}/resolve`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResolutionRequest {
    /// `same` merges the candidate into the episode it duplicates;
    /// `separate` makes it an episode of its own.
    pub resolution: DuplicateResolution,
}

#[utoipa::path(
    post,
    path = "/api/v1/episodes/{id}/resolve",
    tag = "library",
    params(("id" = String, Path, description = "Candidate episode id")),
    request_body = ResolutionRequest,
    responses(
        (status = 200, description = "What the resolution did", body = uguisu_engine::duplicates::DuplicateResolved),
        (status = 400, description = "Not an episode id", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "Not a known resolution", body = ApiError),
        (status = 409, description = "Not a candidate, or a merge that would drop a file or join two items of the current feed", body = ApiError)
    )
)]
pub(crate) async fn resolve_duplicate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Body(req): Body<ResolutionRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&id, "episode")?;
    let resolved = engine
        .resolve_duplicate(id, req.resolution)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &resolved))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RefreshParams {
    /// Ignore validators and the body fingerprint.
    #[serde(default)]
    pub force: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/refresh",
    tag = "library",
    params(("id" = String, Path, description = "Podcast id"), RefreshParams),
    responses(
        (status = 200, body = uguisu_core::feed::RefreshReport),
        (status = 404, body = ApiError),
        (status = 502, description = "The feed could not be fetched or parsed", body = ApiError)
    )
)]
pub(crate) async fn refresh_podcast(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Params(params): Params<RefreshParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let report = engine
        .refresh_podcast(
            id,
            RefreshOptions {
                force: params.force,
                cancel: None,
            },
        )
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &report))
}

/// Body of `POST /api/v1/podcasts/{id}/move-feed`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MoveFeedRequest {
    /// The feed URL to move to.
    pub url: String,
    /// Check, and never move.
    #[serde(default)]
    pub dry_run: bool,
    /// Move even when the same-show check fails.
    #[serde(default)]
    pub force: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/move-feed",
    tag = "library",
    params(("id" = String, Path, description = "Podcast id")),
    request_body = MoveFeedRequest,
    responses(
        (status = 200, description = "What the check found and whether the podcast moved; an unverified feed without force is not moved", body = uguisu_engine::migration::FeedMove),
        (status = 400, description = "Not a podcast id, or not an http(s) URL", body = ApiError),
        (status = 403, description = "Refused by the network policy", body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "The URL is another podcast's feed, or the feed carries another podcast's podcast:guid", body = ApiError),
        (status = 502, description = "The URL does not serve a podcast feed", body = ApiError)
    )
)]
pub(crate) async fn move_feed(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Body(req): Body<MoveFeedRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: PodcastId = parse_id(&id, "podcast")?;
    let moved = engine
        .move_feed(
            id,
            &req.url,
            MoveOptions {
                dry_run: req.dry_run,
                force: req.force,
            },
            CancellationToken::new(),
        )
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &moved))
}

/// Answer of `POST /api/v1/podcasts/refresh`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct RefreshAllBody {
    entries: Vec<uguisu_engine::RefreshAllEntry>,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/refresh",
    tag = "library",
    params(RefreshParams),
    responses((status = 200, body = RefreshAllBody))
)]
pub(crate) async fn refresh_all(
    State(state): State<AppState>,
    Params(params): Params<RefreshParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let concurrency = engine.config().feed.refresh_concurrency;
    let entries = engine
        .refresh_all(params.force, concurrency, CancellationToken::new())
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &RefreshAllBody { entries }))
}

/// Fetch state of a source, as `feed status` shows it.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FeedStatus {
    /// The source.
    pub source: uguisu_core::model::PodcastSource,
    /// Its newest fetch-log row.
    pub last_fetch: Option<uguisu_core::feed::FeedFetch>,
}

#[utoipa::path(
    get,
    path = "/api/v1/feeds/{source_id}/status",
    tag = "feeds",
    params(("source_id" = String, Path, description = "Source id")),
    responses((status = 200, body = FeedStatus), (status = 404, body = ApiError))
)]
pub(crate) async fn feed_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: SourceId = parse_id(&id, "source")?;
    let (source, last_fetch) = engine.source(id).await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &FeedStatus { source, last_fetch }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct InspectParams {
    /// Feed URL.
    pub url: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/feeds/inspect",
    tag = "feeds",
    params(InspectParams),
    responses(
        (status = 200, description = "Parsed without storing anything", body = uguisu_engine::inspect::Inspection),
        (status = 400, body = ApiError),
        (status = 502, body = ApiError)
    )
)]
pub(crate) async fn inspect(
    State(state): State<AppState>,
    Params(params): Params<InspectParams>,
) -> ApiResult {
    let raw = params.url.as_deref().map(str::trim).unwrap_or_default();
    let url = url::Url::parse(raw).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("query parameter `url` must be a URL: {e}"),
        )
    })?;
    let inspection = if let Some(engine) = &state.engine {
        engine.inspect_url(&url, CancellationToken::new()).await
    } else {
        let client = uguisu_engine::inspect::standalone_client(&state.discovery.config.network)
            .map_err(|e| api_error(&e))?;
        uguisu_engine::inspect(
            &client,
            &uguisu_core::config::FeedLimits::default(),
            &url,
            CancellationToken::new(),
        )
        .await
    }
    .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &inspection))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventParams {
    /// Stored events after this id (JSON list instead of a live stream).
    pub after: Option<String>,
    /// Page size for the list; without `after`, the newest this many.
    pub limit: Option<u32>,
    /// Comma-separated event kinds to leave out of the live stream
    /// (`download.progress` is the usual one).
    pub exclude: Option<String>,
}

/// Answer of `GET /api/v1/events?after=`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct EventList {
    events: Vec<uguisu_core::Event>,
}

#[utoipa::path(
    get,
    path = "/api/v1/events",
    tag = "events",
    params(EventParams),
    responses(
        (status = 200, description = "Stored events when `after` or `limit` is given; otherwise a `text/event-stream` of live events", body = EventList)
    )
)]
pub(crate) async fn events(
    State(state): State<AppState>,
    Params(params): Params<EventParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    if params.after.is_some() || params.limit.is_some() {
        let after: Option<EventId> = match params.after.as_deref() {
            Some(a) => Some(parse_id(a, "event")?),
            None => None,
        };
        let events = engine
            .stored_events(after, params.limit.unwrap_or(100).clamp(1, 1000))
            .await
            .map_err(|e| api_error(&e))?;
        return Ok(body(StatusCode::OK, &EventList { events }));
    }
    let excluded: Vec<String> = params
        .exclude
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let sub = engine.subscribe();
    let stream = futures_util::stream::unfold(sub, |mut sub| async move {
        sub.recv().await.map(|event| (event, sub))
    })
    .filter(move |event| {
        let keep = !excluded.iter().any(|k| k == event.name());
        futures_util::future::ready(keep)
    })
    .map(|event| {
        Ok::<SseEvent, Infallible>(
            SseEvent::default()
                .event(event.name())
                .id(event.id.to_string())
                .json_data(&event)
                .unwrap_or_else(|_| {
                    SseEvent::default()
                        .event("error")
                        .data("serialization failed")
                }),
        )
    });
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use http_body_util::BodyExt;

    async fn read(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn a_body_gains_the_schema() {
        let (status, json) = read(body(
            StatusCode::CREATED,
            &serde_json::json!({ "podcast": "x" }),
        ))
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["schema"], SCHEMA);
        assert_eq!(json["podcast"], "x");
    }

    #[tokio::test]
    async fn a_versioned_document_keeps_its_own_schema() {
        let (status, json) = read(body(StatusCode::OK, &serde_json::json!({ "schema": 7 }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["schema"], 7);
    }

    #[tokio::test]
    async fn a_payload_that_is_not_an_object_is_refused() {
        let (status, json) = read(body(StatusCode::OK, &serde_json::json!([1, 2, 3]))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"]["kind"], "internal");
    }

    #[test]
    fn an_error_body_carries_the_schema() {
        let (status, json) = ApiError::new(StatusCode::NOT_FOUND, "not_found", "gone");
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json.schema, SCHEMA);
        assert_eq!(json.error.kind, "not_found");
    }
}
