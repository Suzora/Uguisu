//! Archive and policy endpoints (`docs/ARCHIVE_ENGINE.md`):
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /api/v1/archive?state=&podcast=&limit=` | list artifacts, newest first |
//! | `GET /api/v1/archive/stats` | how many artifacts are in each verification state |
//! | `GET /api/v1/archive/missing` / `…/invalid` | the artifacts that need attention |
//! | `GET /api/v1/archive/{episode_id}` | one episode's artifact (404 when it has none) |
//! | `POST /api/v1/archive/verify` `{"depth": "existence\|light\|full", "podcast": …, "state": …}` | verify everything a filter selects |
//! | `POST /api/v1/archive/{episode_id}/verify` `{"depth": …}` | verify one artifact |
//! | `POST /api/v1/archive/{episode_id}/path-preview` | what the template would produce (creates nothing) |
//! | `POST /api/v1/archive/{episode_id}/relocate` `{"dry_run": true}` | move one artifact to its template path |
//! | `POST /api/v1/archive/reconcile?deep=true` | register unrecorded downloads, then check |
//! | `GET /api/v1/archive/policies` | every stored per-podcast policy |
//! | `GET\|PUT\|POST\|DELETE /api/v1/podcasts/{id}/policy` | read, set or clear one podcast's policy |
//! | `POST /api/v1/podcasts/{id}/policy/clear` | clear it, for clients without `DELETE` |
//!
//! Reading and previewing never write to the archive, so every `GET` and
//! `path-preview` works against a read-only media directory. Verification
//! writes only to the database: it never changes a file, and never deletes
//! a record. Every body carries `schema: 1`; errors follow `library.rs`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::archive::{
    ArchivePolicy, PolicyMode, VerificationState, VerifyDepth, policy_reason,
};
use uguisu_core::download::Priority;
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_storage::archive_files::ArchiveFilter;

use crate::extract::{Body, MaybeBody, Params};
use crate::library::{ApiResult, api_error, body, engine, parse_id};
use crate::{ApiError, AppState};

/// What a rejected request answers with, as the other modules spell it.
type Rejection = (StatusCode, Json<ApiError>);

pub(crate) fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/archive", get(list))
        .route("/api/v1/archive/stats", get(stats))
        .route("/api/v1/archive/missing", get(missing))
        .route("/api/v1/archive/invalid", get(invalid))
        .route("/api/v1/archive/verify", post(verify_all))
        .route("/api/v1/archive/reconcile", post(reconcile))
        .route("/api/v1/archive/policies", get(policies))
        .route("/api/v1/archive/{episode_id}", get(show))
        .route("/api/v1/archive/{episode_id}/verify", post(verify_one))
        .route(
            "/api/v1/archive/{episode_id}/path-preview",
            post(path_preview),
        )
        .route("/api/v1/archive/{episode_id}/relocate", post(relocate))
        .route(
            "/api/v1/podcasts/{id}/policy",
            // `POST` is accepted alongside `PUT` so a client that only
            // speaks GET and POST — the CLI's — can set a policy too.
            get(policy_show)
                .put(policy_set)
                .post(policy_set)
                .delete(policy_clear),
        )
        .route("/api/v1/podcasts/{id}/policy/clear", post(policy_clear))
}

/// Query of `GET /api/v1/archive`.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// Only artifacts in this verification state.
    pub state: Option<String>,
    /// Only artifacts of this podcast.
    pub podcast: Option<String>,
    /// Only artifacts whose feed now points at different audio.
    pub source_changed: Option<bool>,
    /// Last artifact id of the previous page.
    pub after: Option<String>,
    /// Page size (1..=500).
    pub limit: Option<u32>,
}

fn parse_state(raw: &str) -> Result<VerificationState, Rejection> {
    VerificationState::parse(raw).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!(
                "`{raw}` is not a verification state (one of {})",
                VerificationState::ALL
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })
}

fn parse_depth(raw: Option<&str>) -> Result<VerifyDepth, Rejection> {
    match raw {
        None => Ok(VerifyDepth::default()),
        Some(raw) => VerifyDepth::parse(raw).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!(
                    "`{raw}` is not a verification depth (one of {})",
                    VerifyDepth::ALL
                        .iter()
                        .map(|d| d.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        }),
    }
}

async fn filtered(state: &AppState, params: &ListParams) -> ApiResult {
    let engine = engine(state)?;
    let filter = ArchiveFilter {
        state: params.state.as_deref().map(parse_state).transpose()?,
        podcast_id: match params.podcast.as_deref() {
            Some(p) => Some(parse_id(p, "podcast")?),
            None => None,
        },
        source_changed: params.source_changed.unwrap_or(false),
    };
    let after: Option<uguisu_core::ids::ArchiveFileId> = match params.after.as_deref() {
        Some(a) => Some(parse_id(a, "archive file")?),
        None => None,
    };
    let limit = uguisu_core::page::limit(
        params.limit,
        uguisu_core::page::DEFAULT,
        uguisu_core::page::MAX,
    )
    .map_err(|e| api_error(&e))?;
    let page = engine
        .archive_list(&filter, after, limit)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &page))
}

#[utoipa::path(
    get,
    path = "/api/v1/archive",
    tag = "archive",
    operation_id = "archive_list",
    params(ListParams),
    responses(
        (status = 200, body = uguisu_engine::archive::ArchivePage),
        (status = 400, description = "Unknown state, cursor or limit", body = ApiError)
    )
)]
pub(crate) async fn list(
    State(state): State<AppState>,
    Params(params): Params<ListParams>,
) -> ApiResult {
    filtered(&state, &params).await
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/missing",
    tag = "archive",
    params(ListParams),
    responses((status = 200, body = uguisu_engine::archive::ArchivePage))
)]
pub(crate) async fn missing(
    State(state): State<AppState>,
    Params(params): Params<ListParams>,
) -> ApiResult {
    filtered(
        &state,
        &ListParams {
            state: Some(VerificationState::Missing.as_str().to_owned()),
            ..params
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/invalid",
    tag = "archive",
    params(ListParams),
    responses((status = 200, body = uguisu_engine::archive::ArchivePage))
)]
pub(crate) async fn invalid(
    State(state): State<AppState>,
    Params(params): Params<ListParams>,
) -> ApiResult {
    filtered(
        &state,
        &ListParams {
            state: Some(VerificationState::Invalid.as_str().to_owned()),
            ..params
        },
    )
    .await
}

/// Counts per verification state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct StatsBody {
    by_state: std::collections::BTreeMap<String, u64>,
    total: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/stats",
    tag = "archive",
    operation_id = "archive_stats",
    responses((status = 200, body = StatsBody))
)]
pub(crate) async fn stats(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let counts = engine.archive_counts().await.map_err(|e| api_error(&e))?;
    let total = counts.values().sum();
    let by_state = counts
        .into_iter()
        .map(|(k, v)| (k.as_str().to_owned(), v))
        .collect();
    Ok(body(StatusCode::OK, &StatsBody { by_state, total }))
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/{episode_id}",
    tag = "archive",
    operation_id = "archive_show",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses(
        (status = 200, body = uguisu_core::archive::ArchiveFile),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn show(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let file = engine
        .archive_file(id)
        .await
        .map_err(|e| api_error(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "archive_not_found",
                format!("episode {id} has no archive file"),
            )
        })?;
    Ok(body(StatusCode::OK, &file))
}

/// Body of the verification endpoints.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct VerifyBody {
    /// How hard to look (default `light`).
    pub depth: Option<String>,
    /// Only artifacts of this podcast (the bulk endpoint).
    pub podcast: Option<String>,
    /// Only artifacts in this state (the bulk endpoint).
    pub state: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/{episode_id}/verify",
    tag = "archive",
    params(("episode_id" = String, Path, description = "Episode id")),
    request_body(content = VerifyBody, description = "Optional; `light` depth without one"),
    responses(
        (status = 200, body = uguisu_engine::archive::VerifiedFile),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn verify_one(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
    MaybeBody(req): MaybeBody<VerifyBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let req = req.unwrap_or_default();
    let depth = parse_depth(req.depth.as_deref())?;
    let verified = engine
        .verify_episode(id, depth)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &verified))
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/verify",
    tag = "archive",
    request_body(content = VerifyBody, description = "Optional; every artifact at `light` depth without one"),
    responses((status = 200, body = uguisu_engine::archive::VerifySummary))
)]
pub(crate) async fn verify_all(
    State(state): State<AppState>,
    MaybeBody(req): MaybeBody<VerifyBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let req = req.unwrap_or_default();
    let depth = parse_depth(req.depth.as_deref())?;
    let filter = ArchiveFilter {
        state: req.state.as_deref().map(parse_state).transpose()?,
        podcast_id: match req.podcast.as_deref() {
            Some(p) => Some(parse_id(p, "podcast")?),
            None => None,
        },
        source_changed: false,
    };
    let summary = engine
        .verify_all(&filter, depth)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &summary))
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/{episode_id}/path-preview",
    tag = "archive",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses(
        (status = 200, description = "Where this episode would be filed, without touching anything", body = uguisu_engine::archive::PathPreview),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn path_preview(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let preview = engine.path_preview(id).await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &preview))
}

/// Body of `POST /api/v1/archive/{episode_id}/relocate`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct RelocateBody {
    /// Report what would happen without moving anything.
    #[serde(default)]
    pub dry_run: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/{episode_id}/relocate",
    tag = "archive",
    params(("episode_id" = String, Path, description = "Episode id")),
    request_body(content = RelocateBody, description = "Optional; moves the file without one"),
    responses(
        (status = 200, body = uguisu_engine::archive::Relocation),
        (status = 404, body = ApiError),
        (status = 409, description = "The target is taken or the record disagrees with the disk", body = ApiError)
    )
)]
pub(crate) async fn relocate(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
    MaybeBody(req): MaybeBody<RelocateBody>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let dry_run = req.is_some_and(|b| b.dry_run);
    let outcome = engine
        .relocate(id, dry_run)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &outcome))
}

/// Query of `POST /api/v1/archive/reconcile`.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReconcileParams {
    /// Also verify every artifact (size, not hash).
    #[serde(default)]
    pub deep: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/reconcile",
    tag = "archive",
    operation_id = "archive_reconcile",
    params(ReconcileParams),
    responses((status = 200, body = uguisu_engine::archive::ArchiveReconcileReport))
)]
pub(crate) async fn reconcile(
    State(state): State<AppState>,
    Params(params): Params<ReconcileParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let report = engine
        .reconcile_archive(params.deep)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &report))
}

/// Every stored per-podcast policy.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct PolicyList {
    policies: Vec<ArchivePolicy>,
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/policies",
    tag = "policies",
    responses((status = 200, body = PolicyList))
)]
pub(crate) async fn policies(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let policies = engine.policies().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &PolicyList { policies }))
}

/// One podcast's policy, with the values actually in force.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct PolicyBody {
    podcast_id: PodcastId,
    /// The stored override, when the podcast has one.
    stored: Option<ArchivePolicy>,
    /// What the global defaults and the override add up to.
    effective: EffectiveBody,
}

/// The resolved policy, flattened for the wire.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct EffectiveBody {
    mode: PolicyMode,
    max_backlog: u32,
    max_age_days: u32,
    priority: Priority,
}

#[utoipa::path(
    get,
    path = "/api/v1/podcasts/{id}/policy",
    tag = "policies",
    params(("id" = String, Path, description = "Podcast id")),
    responses((status = 200, body = PolicyBody), (status = 404, body = ApiError))
)]
pub(crate) async fn policy_show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let stored = engine
        .policies()
        .await
        .map_err(|e| api_error(&e))?
        .into_iter()
        .find(|p| p.podcast_id == podcast_id);
    let effective = engine
        .effective_policy(podcast_id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &PolicyBody {
            podcast_id,
            stored,
            effective: EffectiveBody {
                mode: effective.mode,
                max_backlog: effective.max_backlog,
                max_age_days: effective.max_age_days,
                priority: effective.priority,
            },
        },
    ))
}

/// One podcast's policy: the body of `PUT /api/v1/podcasts/{id}/policy`,
/// and the `policy` an OPML import stores with each podcast it adds.
///
/// Every field but `mode` is optional and means "use the global default"
/// when absent, which is how one podcast can override the backlog without
/// pinning the age limit as a side effect.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PolicyUpdate {
    /// `manual` or `auto`.
    pub mode: String,
    /// Episodes of this podcast that may await archiving at once.
    pub max_backlog: Option<u32>,
    /// Ignore episodes published longer ago than this.
    pub max_age_days: Option<u32>,
    /// Priority for jobs the policy creates.
    pub priority: Option<Priority>,
}

impl PolicyUpdate {
    /// The mode, or the `policy_invalid` error naming the two there are.
    pub(crate) fn mode(&self) -> Result<PolicyMode, (StatusCode, Json<ApiError>)> {
        PolicyMode::parse(&self.mode).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "policy_invalid",
                format!("`{}` is not a policy mode (manual, auto)", self.mode),
            )
        })
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/podcasts/{id}/policy",
    tag = "policies",
    params(("id" = String, Path, description = "Podcast id")),
    request_body = PolicyUpdate,
    responses(
        (status = 200, description = "The policy as it now stands", body = PolicyBody),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn policy_set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Body(req): Body<PolicyUpdate>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let policy = ArchivePolicy {
        podcast_id,
        mode: req.mode()?,
        max_backlog: req.max_backlog,
        max_age_days: req.max_age_days,
        priority: req.priority,
        updated_at: OffsetDateTime::now_utc(),
    };
    engine
        .set_policy(&policy)
        .await
        .map_err(|e| api_error(&e))?;
    policy_show(State(state), Path(id)).await
}

/// What clearing a policy did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ClearOutcome {
    podcast_id: PodcastId,
    /// `false` when the podcast had no override to begin with.
    cleared: bool,
    /// The vocabulary a skip reason comes from, so a client can render it.
    reasons: [&'static str; 9],
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/policy/clear",
    tag = "policies",
    params(("id" = String, Path, description = "Podcast id")),
    responses((status = 200, body = ClearOutcome), (status = 404, body = ApiError))
)]
pub(crate) async fn policy_clear(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let cleared = engine
        .clear_policy(podcast_id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &ClearOutcome {
            podcast_id,
            cleared,
            reasons: [
                policy_reason::DISABLED,
                policy_reason::ALREADY_ARCHIVED,
                policy_reason::ALREADY_QUEUED,
                policy_reason::NO_ENCLOSURE,
                policy_reason::DUPLICATE,
                policy_reason::SKIPPED,
                policy_reason::TOO_OLD,
                policy_reason::BACKLOG_EXCEEDED,
                policy_reason::REMOVED_FROM_FEED,
            ],
        },
    ))
}
