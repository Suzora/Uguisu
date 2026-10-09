//! Sidecars, manifests, rebuild, import, artwork and tags (
//! `docs/ARCHIVE_ENGINE.md`):
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /api/v1/archive/manifests` | every podcast's manifest state |
//! | `POST /api/v1/archive/manifests/write` | write every stale manifest |
//! | `POST /api/v1/archive/manifests/{podcast_id}/write` | write one |
//! | `GET /api/v1/archive/manifests/{podcast_id}/verify?rehash=true` | compare a manifest with the archive |
//! | `POST /api/v1/archive/rebuild` `{"apply": false, "podcast": …}` | rebuild records from sidecars |
//! | `POST /api/v1/archive/import` `{"path": …, "format": …, "apply": false}` | read a foreign archive |
//! | `GET /api/v1/archive/orphans` | what nothing owns under the media directory (ADR 0051) |
//! | `POST /api/v1/archive/restore` `{"path": …, "apply": false}` | put missing files back from a folder, exact bytes only (ADR 0060) |
//! | `POST /api/v1/archive/{episode_id}/redownload` | download a missing file's episode again (ADR 0060) |
//! | `GET /api/v1/archive/{episode_id}/sidecar` | the document beside one artifact |
//! | `POST /api/v1/archive/{episode_id}/sidecar/write` | write it |
//! | `GET /api/v1/archive/{episode_id}/tags` | the tags the file currently carries |
//! | `POST /api/v1/archive/{episode_id}/tags/write` `{"mode": "fill_missing\|sync"}` | write them |
//! | `GET /api/v1/archive/{episode_id}/media` | the archived audio, with `Range` |
//! | `GET /api/v1/podcasts/{id}/artwork` | the current image and the ones it replaced |
//! | `GET /api/v1/podcasts/{id}/artwork/image` | its bytes |
//! | `POST /api/v1/podcasts/{id}/artwork/fetch` `{"force": false}` | fetch it |
//!
//! Every write here is explicit. `rebuild`, `import` and `restore` default to
//! a dry run, and say so in the body they return, because they read trees
//! Uguisu did not necessarily write.
//!
//! **`import` and `restore` name a directory on the machine the server runs on.** It is
//! therefore exactly as privileged as the server process, which is why
//! Uguisu binds to localhost by default (`docs/SECURITY.md`). It refuses a
//! path that overlaps the media directory, and it only ever reads.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uguisu_archive::import::ImportFormat;
use uguisu_archive::{RelativePath, is_control_path, is_sidecar_path};
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchiveErrorKind, TagMode};
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_engine::import::ImportOptions;
use uguisu_engine::rebuild::RebuildOptions;

use crate::bytes::{media_type, serve_file};
use crate::extract::{Body, MaybeBody, Params};
use crate::library::{ApiResult, api_error, body, engine, parse_id};
use crate::{ApiError, AppState};

type Rejection = (StatusCode, Json<ApiError>);

pub(crate) fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/archive/manifests", get(manifest_status))
        .route("/api/v1/archive/manifests/write", post(manifests_write_all))
        .route(
            "/api/v1/archive/manifests/{podcast_id}/write",
            post(manifest_write),
        )
        .route(
            "/api/v1/archive/manifests/{podcast_id}/verify",
            get(manifest_verify).post(manifest_verify),
        )
        .route("/api/v1/archive/rebuild", post(rebuild))
        .route("/api/v1/archive/import", post(import))
        .route("/api/v1/archive/restore", post(restore))
        .route("/api/v1/archive/{episode_id}/redownload", post(redownload))
        .route("/api/v1/archive/orphans", get(orphans))
        .route("/api/v1/archive/{episode_id}/sidecar", get(sidecar_show))
        .route(
            "/api/v1/archive/{episode_id}/sidecar/write",
            post(sidecar_write),
        )
        .route("/api/v1/archive/{episode_id}/media", get(media))
        .route("/api/v1/archive/{episode_id}/tags", get(tags_show))
        .route("/api/v1/archive/{episode_id}/tags/write", post(tags_write))
        .route("/api/v1/podcasts/{id}/artwork", get(artwork_show))
        .route("/api/v1/podcasts/{id}/artwork/image", get(artwork_image))
        .route("/api/v1/podcasts/{id}/artwork/fetch", post(artwork_fetch))
}

/// Manifest state per podcast.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ManifestPage {
    manifests: Vec<uguisu_core::archive::ArchiveManifest>,
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/manifests",
    tag = "manifests",
    responses((status = 200, body = ManifestPage))
)]
pub(crate) async fn manifest_status(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let manifests = engine.manifest_status().await.map_err(|e| api_error(&e))?;
    Ok(body(StatusCode::OK, &ManifestPage { manifests }))
}

/// What writing one or more manifests produced.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ManifestWriteBody {
    written: Vec<ManifestWritten>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ManifestWritten {
    podcast_id: PodcastId,
    path: String,
    entries: u64,
    /// `false` when a change landed while the file was being written, so
    /// the manifest is still stale and the next flush will redo it.
    cleared: bool,
}

fn written(w: uguisu_engine::archive_meta::ManifestWritten) -> ManifestWritten {
    ManifestWritten {
        podcast_id: w.podcast_id,
        path: w.path,
        entries: w.entries,
        cleared: w.cleared,
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/manifests/write",
    tag = "manifests",
    responses((status = 200, description = "Every stale manifest, rewritten", body = ManifestWriteBody))
)]
pub(crate) async fn manifests_write_all(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let out = engine
        .write_stale_manifests()
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &ManifestWriteBody {
            written: out.into_iter().map(written).collect(),
        },
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/manifests/{podcast_id}/write",
    tag = "manifests",
    params(("podcast_id" = String, Path, description = "Podcast id")),
    responses((status = 200, body = ManifestWriteBody), (status = 404, body = ApiError))
)]
pub(crate) async fn manifest_write(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let out = engine
        .write_manifest(podcast_id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &ManifestWriteBody {
            written: vec![written(out)],
        },
    ))
}

/// Query of the manifest check.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct VerifyParams {
    /// Re-read the files instead of comparing against the index.
    pub rehash: Option<bool>,
}

/// What a manifest check found.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ManifestCheckBody {
    podcast_id: PodcastId,
    path: String,
    stale: bool,
    rehashed: bool,
    unchanged: u64,
    changed: Findings,
    missing: Findings,
    added: Findings,
    unreadable: Findings,
    clean: bool,
}

/// A group of findings: how many, and the first few by name.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct Findings {
    count: u64,
    sample: Vec<String>,
}

fn findings(f: uguisu_archive::manifest::Findings) -> Findings {
    Findings {
        count: f.count,
        sample: f.sample,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/manifests/{podcast_id}/verify",
    tag = "manifests",
    params(("podcast_id" = String, Path, description = "Podcast id"), VerifyParams),
    responses((status = 200, body = ManifestCheckBody), (status = 404, body = ApiError))
)]
pub(crate) async fn manifest_verify(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Params(params): Params<VerifyParams>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let check = engine
        .verify_manifest(podcast_id, params.rehash.unwrap_or(false))
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &ManifestCheckBody {
            podcast_id: check.podcast_id,
            path: check.path,
            stale: check.stale,
            rehashed: check.rehashed,
            unchanged: check.diff.unchanged,
            clean: check.diff.is_clean(),
            changed: findings(check.diff.changed),
            missing: findings(check.diff.missing),
            added: findings(check.diff.added),
            unreadable: findings(check.diff.unreadable),
        },
    ))
}

/// Body of `POST /api/v1/archive/rebuild`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct RebuildRequest {
    /// Write the records. Absent means a dry run.
    pub apply: Option<bool>,
    /// Restrict to one podcast.
    pub podcast: Option<String>,
}

/// What a rebuild found.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct RebuildBody {
    applied: bool,
    scanned: u64,
    rebuilt: u64,
    unchanged: u64,
    conflicts: Findings,
    unknown_episode: Findings,
    malformed: Findings,
    missing_media: Findings,
    unreadable: Findings,
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/rebuild",
    tag = "archive",
    request_body(content = RebuildRequest, description = "Optional; a dry run without one"),
    responses((status = 200, body = RebuildBody))
)]
pub(crate) async fn rebuild(
    State(state): State<AppState>,
    MaybeBody(request): MaybeBody<RebuildRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let request = request.unwrap_or_default();
    let podcast = match request.podcast.as_deref() {
        Some(p) => Some(parse_id(p, "podcast")?),
        None => None,
    };
    let report = engine
        .rebuild_archive(&RebuildOptions {
            apply: request.apply.unwrap_or(false),
            podcast,
        })
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &RebuildBody {
            applied: report.applied,
            scanned: report.scanned,
            rebuilt: report.rebuilt,
            unchanged: report.unchanged,
            conflicts: findings(report.conflicts),
            unknown_episode: findings(report.unknown_episode),
            malformed: findings(report.malformed),
            missing_media: findings(report.missing_media),
            unreadable: findings(report.unreadable),
        },
    ))
}

/// What `archive orphans` found; nothing is ever removed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct OrphansBody {
    scanned: u64,
    leftovers: Findings,
    orphan_parts: Findings,
    unknown_media: Findings,
    stray_sidecars: Findings,
    unreadable: Findings,
    clean: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/orphans",
    tag = "archive",
    responses((status = 200, description = "What nothing owns under the media directory", body = OrphansBody))
)]
pub(crate) async fn orphans(State(state): State<AppState>) -> ApiResult {
    let engine = engine(&state)?;
    let report = engine.orphans().await.map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &OrphansBody {
            scanned: report.scanned,
            clean: !report.has_findings(),
            leftovers: findings(report.leftovers),
            orphan_parts: findings(report.orphan_parts),
            unknown_media: findings(report.unknown_media),
            stray_sidecars: findings(report.stray_sidecars),
            unreadable: findings(report.unreadable),
        },
    ))
}

/// Body of `POST /api/v1/archive/import`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ImportRequest {
    /// The directory to read, on the server's own filesystem.
    pub path: String,
    /// The layout; detected when absent.
    pub format: Option<String>,
    /// Copy the files. Absent means a dry run.
    pub apply: Option<bool>,
    /// Import only into this podcast.
    pub podcast: Option<String>,
    /// Confidence a match needs, in percent.
    pub threshold: Option<u32>,
    /// Podgrab's database, on the server's own filesystem, read only to
    /// name each file exactly (ADR 0050); implies the Podgrab layout.
    pub podgrab_db: Option<String>,
}

/// What an import would do, or did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ImportBody {
    applied: bool,
    source_root: String,
    format: String,
    threshold: u32,
    scanned: u64,
    imported: u64,
    already_present: u64,
    conflicts: u64,
    ambiguous: u64,
    unmatched: u64,
    invalid: u64,
    unreadable: u64,
    items: Vec<ImportItem>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ImportItem {
    source_path: String,
    size_bytes: u64,
    action: String,
    podcast_id: Option<PodcastId>,
    episode_id: Option<EpisodeId>,
    confidence: u32,
    matched_by: Option<String>,
    target_path: Option<String>,
    detail: Option<String>,
}

fn parse_format(raw: Option<&str>) -> Result<Option<ImportFormat>, Rejection> {
    match raw {
        None => Ok(None),
        Some(raw) => ImportFormat::parse(raw).map(Some).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!(
                    "`{raw}` is not an import format (one of {})",
                    ImportFormat::ALL
                        .iter()
                        .map(|f| f.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        }),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/import",
    tag = "archive",
    request_body = ImportRequest,
    responses(
        (status = 200, description = "The plan, or what applying it did", body = ImportBody),
        (status = 400, body = ApiError),
        (status = 422, description = "The layout could not be matched", body = ApiError)
    )
)]
pub(crate) async fn import(
    State(state): State<AppState>,
    Body(request): Body<ImportRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast = match request.podcast.as_deref() {
        Some(p) => Some(parse_id(p, "podcast")?),
        None => None,
    };
    let options = ImportOptions {
        apply: request.apply.unwrap_or(false),
        format: parse_format(request.format.as_deref())?,
        podcast,
        threshold: request.threshold,
        podgrab_db: request.podgrab_db.map(std::path::PathBuf::from),
    };
    let source = std::path::PathBuf::from(&request.path);
    let plan = if options.apply {
        engine.import_apply(&source, &options).await
    } else {
        engine.import_plan(&source, &options).await
    }
    .map_err(|e| api_error(&e))?;

    Ok(body(
        StatusCode::OK,
        &ImportBody {
            applied: plan.applied,
            source_root: plan.source_root,
            format: plan.format.as_str().to_owned(),
            threshold: plan.threshold,
            scanned: plan.counts.scanned,
            imported: plan.counts.imported,
            already_present: plan.counts.already_present,
            conflicts: plan.counts.conflicts,
            ambiguous: plan.counts.ambiguous,
            unmatched: plan.counts.unmatched,
            invalid: plan.counts.invalid,
            unreadable: plan.counts.unreadable,
            items: plan
                .items
                .into_iter()
                .map(|i| ImportItem {
                    source_path: i.source_path,
                    size_bytes: i.size_bytes,
                    action: i.action.as_str().to_owned(),
                    podcast_id: i.podcast_id,
                    episode_id: i.episode_id,
                    confidence: i.confidence,
                    matched_by: i.matched_by,
                    target_path: i.target_path,
                    detail: i.detail,
                })
                .collect(),
        },
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/{episode_id}/sidecar",
    tag = "sidecars",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses(
        (status = 200, description = "The sidecar document as it is on disk, with its own `schema`", body = uguisu_core::archive::Sidecar),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn sidecar_show(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let sidecar = engine
        .read_sidecar(id)
        .await
        .map_err(|e| api_error(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "sidecar_missing",
                format!("episode {id} has no sidecar on disk"),
            )
        })?;
    Ok(body(StatusCode::OK, &sidecar))
}

/// Body of `POST /api/v1/archive/restore`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RestoreRequest {
    /// The folder to look in, on the server's own filesystem.
    pub path: String,
    /// Copy the files back. Absent means a dry run.
    pub apply: Option<bool>,
    /// Only this podcast's missing files.
    pub podcast: Option<String>,
}

/// What a restore would do, or did (ADR 0060).
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct RestoreBody {
    applied: bool,
    source_root: String,
    /// Files in the folder that were looked at.
    scanned: u64,
    /// One line per missing archived file.
    items: Vec<RestoreLine>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct RestoreLine {
    episode_id: EpisodeId,
    podcast_id: PodcastId,
    /// Where the record says the file belongs, relative to the media root.
    target_path: String,
    /// `restore`, `returned`, `source_only`, `taken`, `changed`, `not_found` or `failed`.
    action: String,
    /// The file in the folder with the record's bytes, relative to the folder.
    source_path: Option<String>,
    detail: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/restore",
    tag = "archive",
    request_body = RestoreRequest,
    responses(
        (status = 200, description = "The plan, or what applying it did", body = RestoreBody),
        (status = 400, body = ApiError)
    )
)]
pub(crate) async fn restore(
    State(state): State<AppState>,
    Body(request): Body<RestoreRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast = match request.podcast.as_deref() {
        Some(p) => Some(parse_id(p, "podcast")?),
        None => None,
    };
    let options = uguisu_engine::restore::RestoreOptions {
        apply: request.apply.unwrap_or(false),
        podcast,
    };
    let report = engine
        .restore(std::path::Path::new(&request.path), &options)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &RestoreBody {
            applied: report.applied,
            source_root: report.source_root,
            scanned: report.scanned,
            items: report
                .items
                .into_iter()
                .map(|i| RestoreLine {
                    episode_id: i.episode_id,
                    podcast_id: i.podcast_id,
                    target_path: i.target_path,
                    action: i.action.as_str().to_owned(),
                    source_path: i.source_path,
                    detail: i.detail,
                })
                .collect(),
        },
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/{episode_id}/redownload",
    tag = "archive",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses(
        (status = 200, description = "Queued again, or a job that already exists", body = uguisu_download::EnqueueOutcome),
        (status = 201, description = "A new job, for a file an import or a rebuild archived", body = uguisu_download::EnqueueOutcome),
        (status = 400, description = "Not an episode id, or an episode without a usable enclosure", body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "Something is at the record's path or the old download's `.part`, the episode has no archived file, or it is a candidate, skipped or gone from the feed", body = ApiError)
    )
)]
pub(crate) async fn redownload(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let outcome = engine
        .downloads()
        .redownload(id, uguisu_core::download::Priority::Normal)
        .await
        .map_err(|e| api_error(&e))?;
    let status = match outcome {
        uguisu_download::EnqueueOutcome::Created(_) => StatusCode::CREATED,
        _ => StatusCode::OK,
    };
    Ok(body(status, &outcome))
}

/// Where a sidecar went.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct SidecarWriteBody {
    episode_id: EpisodeId,
    path: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/{episode_id}/sidecar/write",
    tag = "sidecars",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses((status = 200, body = SidecarWriteBody), (status = 404, body = ApiError))
)]
pub(crate) async fn sidecar_write(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let written = engine
        .write_sidecar(id)
        .await
        .map_err(|e| api_error(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "archive_not_found",
                format!("episode {id} has no archive file, or sidecars are switched off"),
            )
        })?;
    Ok(body(
        StatusCode::OK,
        &SidecarWriteBody {
            episode_id: written.episode_id,
            path: written.path,
        },
    ))
}

/// The tags a file currently carries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct TagsBody {
    values: std::collections::BTreeMap<String, String>,
    has_cover: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/archive/{episode_id}/tags",
    tag = "tags",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses((status = 200, body = TagsBody), (status = 404, body = ApiError))
)]
pub(crate) async fn tags_show(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let tags = engine
        .read_episode_tags(id)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &TagsBody {
            values: tags
                .values
                .into_iter()
                .map(|(k, v)| (k.as_str().to_owned(), v))
                .collect(),
            has_cover: tags.cover.is_some(),
        },
    ))
}

/// Body of `POST /api/v1/archive/{episode_id}/tags/write`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct TagsRequest {
    /// `fill_missing` or `sync`; the configured default when absent.
    pub mode: Option<String>,
}

/// What a tag write did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct TagsWriteBody {
    episode_id: EpisodeId,
    path: String,
    state: &'static str,
    mode: TagMode,
    fields: Vec<String>,
    not_embeddable: Vec<String>,
    hash_value: String,
    cover_written: bool,
    detail: Option<String>,
}

fn parse_mode(raw: Option<&str>, fallback: TagMode) -> Result<TagMode, Rejection> {
    match raw {
        None => Ok(fallback),
        Some(raw) => TagMode::parse(&raw.to_ascii_lowercase()).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!(
                    "`{raw}` is not a tag mode (one of {})",
                    TagMode::ALL
                        .iter()
                        .map(|m| m.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        }),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/archive/{episode_id}/tags/write",
    tag = "tags",
    params(("episode_id" = String, Path, description = "Episode id")),
    request_body(content = TagsRequest, description = "Optional; the configured mode without one"),
    responses(
        (status = 200, body = TagsWriteBody),
        (status = 404, body = ApiError),
        (status = 422, description = "The container carries no tags", body = ApiError)
    )
)]
pub(crate) async fn tags_write(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
    MaybeBody(request): MaybeBody<TagsRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let id: EpisodeId = parse_id(&episode_id, "episode")?;
    let request = request.unwrap_or_default();
    let mode = parse_mode(request.mode.as_deref(), engine.config().archive.tag_mode)?;
    let result = engine
        .write_episode_tags(id, mode)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(body(
        StatusCode::OK,
        &TagsWriteBody {
            episode_id: result.episode_id,
            path: result.path,
            state: result.state,
            mode: result.mode,
            fields: result.fields,
            not_embeddable: result.not_embeddable,
            hash_value: result.hash_value,
            cover_written: result.cover_written,
            detail: result.detail,
        },
    ))
}

/// A media file lives one HTTP request away from a browser, so the record's
/// path is checked against the two shapes that are never audio before it is
/// resolved: nothing under the reserved control directory and no sidecar is
/// reachable through this route, whatever a row says.
#[utoipa::path(
    get,
    path = "/api/v1/archive/{episode_id}/media",
    tag = "bytes",
    params(("episode_id" = String, Path, description = "Episode id")),
    responses(
        (status = 200, description = "The archived file", content_type = "application/octet-stream"),
        (status = 206, description = "The requested range", content_type = "application/octet-stream"),
        (status = 304, description = "Unchanged since the given validator"),
        (status = 404, body = ApiError),
        (status = 416, description = "The range cannot be satisfied", body = ApiError)
    )
)]
pub(crate) async fn media(
    State(state): State<AppState>,
    Path(episode_id): Path<String>,
    headers: HeaderMap,
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
    let relative = parse_relative(&file.relative_path)?;
    if is_control_path(&relative) || is_sidecar_path(&relative) {
        return Err(api_error(&UguisuError::Archive {
            kind: ArchiveErrorKind::ArchiveInvalid,
            detail: format!("{} is not media", relative.as_str()),
        }));
    }
    serve_file(
        &media_root(&state)?,
        &relative,
        media_type(file.content_type.as_deref(), file.sniffed_type.as_deref()),
        &file.hash_value,
        &headers,
        "private, max-age=0, must-revalidate",
    )
    .await
    .map_err(|e| api_error(&e))
}

/// The bytes of the artwork a podcast currently uses. Content-addressed, so
/// the hash is both the file name and a strong validator.
#[utoipa::path(
    get,
    path = "/api/v1/podcasts/{id}/artwork/image",
    tag = "bytes",
    params(("id" = String, Path, description = "Podcast id")),
    responses(
        (status = 200, description = "The image bytes", content_type = "application/octet-stream"),
        (status = 206, description = "The requested range", content_type = "application/octet-stream"),
        (status = 304, description = "Unchanged since the given validator"),
        (status = 404, body = ApiError)
    )
)]
pub(crate) async fn artwork_image(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let artwork = engine
        .current_artwork(podcast_id)
        .await
        .map_err(|e| api_error(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "artwork_not_found",
                format!("podcast {podcast_id} has no stored artwork"),
            )
        })?;
    let relative = parse_relative(&artwork.relative_path)?;
    serve_file(
        &media_root(&state)?,
        &relative,
        artwork.format.mime(),
        &artwork.hash_value,
        &headers,
        "private, max-age=3600",
    )
    .await
    .map_err(|e| api_error(&e))
}

fn parse_relative(stored: &str) -> Result<RelativePath, Rejection> {
    RelativePath::parse(stored).map_err(|e| {
        api_error(&uguisu_core::error::UguisuError::Archive {
            kind: e.kind(),
            detail: format!("{stored}: {e}"),
        })
    })
}

fn media_root(state: &AppState) -> Result<std::path::PathBuf, Rejection> {
    engine(state)?
        .config()
        .data
        .media_dir()
        .map_err(|e| api_error(&uguisu_core::error::UguisuError::Config(e.to_string())))
}

/// A podcast's artwork, current and superseded.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ArtworkBody {
    current: Option<uguisu_core::archive::PodcastArtwork>,
    /// Images an earlier fetch stored. They are still on disk: Uguisu
    /// replaces artwork, it does not remove it.
    history: Vec<uguisu_core::archive::PodcastArtwork>,
}

#[utoipa::path(
    get,
    path = "/api/v1/podcasts/{id}/artwork",
    tag = "artwork",
    params(("id" = String, Path, description = "Podcast id")),
    responses((status = 200, body = ArtworkBody), (status = 404, body = ApiError))
)]
pub(crate) async fn artwork_show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let current = engine
        .current_artwork(podcast_id)
        .await
        .map_err(|e| api_error(&e))?;
    let history = engine
        .artwork_history(podcast_id)
        .await
        .map_err(|e| api_error(&e))?
        .into_iter()
        .filter(|a| !a.is_current)
        .collect();
    Ok(body(StatusCode::OK, &ArtworkBody { current, history }))
}

/// Body of `POST /api/v1/podcasts/{id}/artwork/fetch`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct ArtworkRequest {
    /// Skip the conditional request and fetch the bytes again.
    pub force: Option<bool>,
}

/// What a fetch did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct ArtworkFetchBody {
    state: &'static str,
    artwork: Option<uguisu_core::archive::PodcastArtwork>,
    detail: Option<&'static str>,
}

#[utoipa::path(
    post,
    path = "/api/v1/podcasts/{id}/artwork/fetch",
    tag = "artwork",
    params(("id" = String, Path, description = "Podcast id")),
    request_body(content = ArtworkRequest, description = "Optional; conditional without one"),
    responses(
        (status = 200, body = ArtworkFetchBody),
        (status = 404, body = ApiError),
        (status = 422, description = "The bytes are not a usable image", body = ApiError)
    )
)]
pub(crate) async fn artwork_fetch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    MaybeBody(request): MaybeBody<ArtworkRequest>,
) -> ApiResult {
    let engine = engine(&state)?;
    let podcast_id: PodcastId = parse_id(&id, "podcast")?;
    let request = request.unwrap_or_default();
    let outcome = engine
        .fetch_artwork(podcast_id, request.force.unwrap_or(false))
        .await
        .map_err(|e| api_error(&e))?;
    let state_name = outcome.state();
    let (artwork, detail) = match outcome {
        uguisu_engine::artwork::ArtworkOutcome::Fetched(a) => (Some(*a), None),
        uguisu_engine::artwork::ArtworkOutcome::Unchanged(_) => (
            engine
                .current_artwork(podcast_id)
                .await
                .map_err(|e| api_error(&e))?,
            None,
        ),
        uguisu_engine::artwork::ArtworkOutcome::Skipped(why) => (None, Some(why)),
    };
    Ok(body(
        StatusCode::OK,
        &ArtworkFetchBody {
            state: state_name,
            artwork,
            detail,
        },
    ))
}
