//! HTTP routes: the web interface (login required) and the published feeds (secret URL).

use std::time::Duration;

use anyhow::anyhow;
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, HOST, LOCATION, ORIGIN, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use maud::Markup;
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uguisu_core::model::Podcast;
use uguisu_core::search::Provider;
use uguisu_core::{Uguisu, opml, rss};

use crate::{auth, ui};

#[derive(Clone)]
pub struct AppState {
    pub app: Uguisu,
    pub base_url: Option<String>,
}

// ── Errors ─────────────────────────────────────────────────────────────────────────────────

pub enum AppError {
    NotFound,
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(error: E) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                page(ui::message_page("Not found", "This page does not exist.")),
            )
                .into_response(),
            Self::Internal(error) => {
                tracing::error!("request failed: {error:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    page(ui::message_page(
                        "Something went wrong",
                        "The error was written to the server log.",
                    )),
                )
                    .into_response()
            }
        }
    }
}

type AppResult<T = Response> = Result<T, AppError>;

fn page(markup: Markup) -> Html<String> {
    Html(markup.into_string())
}

fn see_other(location: &str) -> Response {
    (StatusCode::SEE_OTHER, [(LOCATION, location.to_string())]).into_response()
}

// ── Router and middleware ──────────────────────────────────────────────────────────────────

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/", get(library))
        .route("/add", get(add_page))
        .route("/search", get(search))
        .route("/search/{provider}", get(search_provider))
        .route("/podcasts", post(add_podcast))
        .route("/podcasts/{id}", get(podcast_page))
        .route("/podcasts/{id}/episodes", get(episodes_fragment))
        .route("/podcasts/{id}/refresh", post(refresh_podcast))
        .route("/podcasts/{id}/settings", post(update_settings))
        .route("/podcasts/{id}/token", post(regenerate_token))
        .route("/podcasts/{id}/remove", post(remove_podcast))
        .route("/cover/{id}", get(cover))
        .route("/opml", get(opml_export).post(opml_import))
        .route("/logout", post(logout))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_login));

    let public = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/{file}", get(asset))
        .route("/setup", get(setup_page).post(setup))
        .route("/login", get(login_page).post(login))
        .route("/feed/{file}", get(feed))
        .route("/media/{token}/{file}", get(feed_cover))
        .route("/media/{token}/{episode_id}/{file}", get(media));

    protected
        .merge(public)
        .fallback(|| async { AppError::NotFound })
        .layer(middleware::from_fn(security))
        .with_state(state)
}

/// Rejects cross-site state changes and adds security headers to every response.
async fn security(request: Request, next: Next) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !safe && !is_same_origin(request.headers()) {
        return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' https: data:; media-src 'self'; \
             frame-ancestors 'none'; form-action 'self'; base-uri 'none'",
        ),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

fn is_same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site, "same-origin" | "none");
    }
    // Older browsers: compare Origin with Host. Clients that send neither are not browsers.
    let Some(origin) = headers.get(ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    origin
        .split_once("://")
        .is_some_and(|(_, origin_host)| origin_host.eq_ignore_ascii_case(host))
}

async fn require_login(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let pool = state.app.pool();
    let logged_in = match auth::session_token(request.headers()) {
        Some(token) => auth::session_is_valid(pool, &token).await,
        None => Ok(false),
    };
    match logged_in {
        Ok(true) => next.run(request).await,
        Ok(false) => {
            let target = match auth::password_is_set(pool).await {
                Ok(true) => "/login",
                Ok(false) => "/setup",
                Err(e) => return AppError::Internal(e).into_response(),
            };
            if request.headers().contains_key("hx-request") {
                // htmx swaps fragments; tell it to navigate instead.
                (StatusCode::UNAUTHORIZED, [("hx-redirect", target)]).into_response()
            } else {
                Redirect::to(target).into_response()
            }
        }
        Err(e) => AppError::Internal(e).into_response(),
    }
}

/// Public base URL: configured, or derived from the request (honouring reverse-proxy headers).
fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(url) = &state.base_url {
        return url.clone();
    }
    let value = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let scheme = value("x-forwarded-proto")
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| matches!(*v, "http" | "https"))
        .unwrap_or("http");
    let host = value("x-forwarded-host")
        .or_else(|| value("host"))
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

// ── Assets ─────────────────────────────────────────────────────────────────────────────────

async fn asset(Path(file): Path<String>) -> AppResult {
    let (content_type, body): (&str, &'static [u8]) = match file.as_str() {
        "app.css" => (
            "text/css; charset=utf-8",
            include_bytes!("../assets/app.css"),
        ),
        "app.js" => (
            "text/javascript; charset=utf-8",
            include_bytes!("../assets/app.js"),
        ),
        "htmx.min.js" => (
            "text/javascript; charset=utf-8",
            include_bytes!("../assets/htmx.min.js"),
        ),
        "favicon.svg" => ("image/svg+xml", include_bytes!("../assets/favicon.svg")),
        _ => return Err(AppError::NotFound),
    };
    Ok((
        [
            (CONTENT_TYPE, content_type),
            (CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response())
}

// ── Setup, login, logout ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SetupForm {
    password: String,
    confirm: String,
}

async fn setup_page(State(state): State<AppState>) -> AppResult {
    if auth::password_is_set(state.app.pool()).await? {
        return Ok(Redirect::to("/login").into_response());
    }
    Ok(page(ui::setup_page(None)).into_response())
}

async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SetupForm>,
) -> AppResult {
    let pool = state.app.pool();
    if auth::password_is_set(pool).await? {
        return Ok(Redirect::to("/login").into_response());
    }
    let problem = if form.password.chars().count() < auth::MIN_PASSWORD_LENGTH {
        Some(format!(
            "Please choose at least {} characters.",
            auth::MIN_PASSWORD_LENGTH
        ))
    } else if form.password != form.confirm {
        Some("The two passwords do not match.".to_string())
    } else {
        None
    };
    if let Some(problem) = problem {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            page(ui::setup_page(Some(&problem))),
        )
            .into_response());
    }
    auth::set_password(pool, form.password).await?;
    start_session(&state, &headers).await
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

async fn login_page(State(state): State<AppState>) -> AppResult {
    if !auth::password_is_set(state.app.pool()).await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    Ok(page(ui::login_page(None)).into_response())
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> AppResult {
    if auth::check_password(state.app.pool(), form.password).await? {
        return start_session(&state, &headers).await;
    }
    // Slows down guessing; Argon2 already makes every attempt expensive.
    tokio::time::sleep(Duration::from_millis(800)).await;
    Ok((
        StatusCode::UNAUTHORIZED,
        page(ui::login_page(Some("That password is not correct."))),
    )
        .into_response())
}

async fn start_session(state: &AppState, headers: &HeaderMap) -> AppResult {
    let token = auth::create_session(state.app.pool()).await?;
    let secure = base_url(state, headers).starts_with("https://");
    Ok((
        StatusCode::SEE_OTHER,
        [
            (LOCATION, "/".to_string()),
            (SET_COOKIE, auth::session_cookie(&token, secure)),
        ],
    )
        .into_response())
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult {
    if let Some(token) = auth::session_token(&headers) {
        auth::delete_session(state.app.pool(), &token).await?;
    }
    Ok((
        StatusCode::SEE_OTHER,
        [
            (LOCATION, "/login".to_string()),
            (SET_COOKIE, auth::clear_cookie()),
        ],
    )
        .into_response())
}

// ── Library and adding ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct LibraryQuery {
    import: Option<usize>,
}

async fn library(State(state): State<AppState>, Query(query): Query<LibraryQuery>) -> AppResult {
    let podcasts = state.app.podcasts().await?;
    let stats = state.app.stats().await?;
    Ok(page(ui::library_page(&podcasts, &stats, query.import)).into_response())
}

async fn add_page() -> Html<String> {
    page(ui::add_page(None, ""))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// Answers at once with one placeholder per directory; each placeholder then loads its own
/// results, so a slow directory never delays a fast one.
async fn search(Query(query): Query<SearchQuery>) -> Html<String> {
    page(ui::search_placeholders(query.q.trim()))
}

async fn search_provider(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(query): Query<SearchQuery>,
) -> AppResult {
    let provider = Provider::from_key(&key).ok_or(AppError::NotFound)?;
    let q = query.q.trim();
    let outcome = if q.chars().count() < 2 {
        Ok(Vec::new())
    } else {
        state.app.search(provider, q).await
    };
    if let Err(e) = &outcome {
        tracing::warn!("search on {} failed: {e:#}", provider.name());
    }
    Ok(page(ui::provider_results(provider, outcome.as_deref().ok())).into_response())
}

#[derive(Deserialize)]
struct AddForm {
    url: String,
}

async fn add_podcast(State(state): State<AppState>, Form(form): Form<AddForm>) -> AppResult {
    match state.app.add_podcast(&form.url).await {
        Ok(id) => Ok(see_other(&format!("/podcasts/{id}"))),
        Err(e) => {
            tracing::info!("cannot add {}: {e:#}", form.url);
            let message = format!("{e:#}");
            Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                page(ui::add_page(Some(&message), &form.url)),
            )
                .into_response())
        }
    }
}

// ── Podcast pages ──────────────────────────────────────────────────────────────────────────

async fn podcast_page(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> AppResult {
    let summary = state
        .app
        .podcast_summary(id)
        .await?
        .ok_or(AppError::NotFound)?;
    let episodes = state.app.episodes(id).await?;
    let feed_url = rss::feed_url(&base_url(&state, &headers), &summary.podcast.feed_token);
    Ok(page(ui::podcast_page(
        &summary,
        &episodes,
        &state.app.downloading(),
        &feed_url,
    ))
    .into_response())
}

async fn episodes_fragment(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult {
    let podcast = state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    let episodes = state.app.episodes(id).await?;
    Ok(page(ui::episode_list(
        &podcast,
        &episodes,
        &state.app.downloading(),
    ))
    .into_response())
}

async fn refresh_podcast(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult {
    state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    // A failure is stored on the podcast and shown on its page.
    if let Err(e) = state.app.refresh(id).await {
        tracing::warn!(podcast_id = id, "refresh failed: {e:#}");
    }
    Ok(see_other(&format!("/podcasts/{id}")))
}

#[derive(Deserialize)]
struct SettingsForm {
    include_removed: Option<String>,
    keep_versions: Option<String>,
}

async fn update_settings(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<SettingsForm>,
) -> AppResult {
    state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    state
        .app
        .set_options(
            id,
            form.include_removed.is_some(),
            form.keep_versions.is_some(),
        )
        .await?;
    Ok(see_other(&format!("/podcasts/{id}")))
}

async fn regenerate_token(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult {
    state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    state.app.regenerate_token(id).await?;
    Ok(see_other(&format!("/podcasts/{id}")))
}

#[derive(Deserialize)]
struct RemoveForm {
    delete_files: Option<String>,
}

async fn remove_podcast(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<RemoveForm>,
) -> AppResult {
    state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    state
        .app
        .remove_podcast(id, form.delete_files.is_some())
        .await?;
    Ok(see_other("/"))
}

async fn cover(State(state): State<AppState>, Path(id): Path<i64>, request: Request) -> AppResult {
    let podcast = state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    serve_cover(&state, &podcast, request).await
}

// ── OPML ───────────────────────────────────────────────────────────────────────────────────

async fn opml_export(State(state): State<AppState>, headers: HeaderMap) -> AppResult {
    let podcasts: Vec<Podcast> = state
        .app
        .podcasts()
        .await?
        .into_iter()
        .map(|s| s.podcast)
        .collect();
    let xml = opml::export(&podcasts, &base_url(&state, &headers));
    Ok((
        [
            (CONTENT_TYPE, "text/x-opml; charset=utf-8"),
            (CONTENT_DISPOSITION, "attachment; filename=\"uguisu.opml\""),
        ],
        xml,
    )
        .into_response())
}

async fn opml_import(State(state): State<AppState>, mut multipart: Multipart) -> AppResult {
    let mut content = None;
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("file") {
            content = Some(field.bytes().await?);
        }
    }
    let content = content.ok_or_else(|| anyhow!("no file uploaded"))?;
    let urls = match opml::parse(&content) {
        Ok(urls) if !urls.is_empty() => urls,
        Ok(_) => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                page(ui::add_page(
                    Some("The file contains no feed addresses."),
                    "",
                )),
            )
                .into_response());
        }
        Err(e) => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                page(ui::add_page(Some(&format!("{e:#}")), "")),
            )
                .into_response());
        }
    };
    let count = urls.len();
    let app = state.app.clone();
    // Fetching many feeds takes a while; the library fills up as they arrive.
    tokio::spawn(async move {
        let report = app.import_feeds(&urls).await;
        tracing::info!(
            added = report.added,
            failed = report.failed,
            "OPML import finished"
        );
    });
    Ok(see_other(&format!("/?import={count}")))
}

// ── Published feeds (no login, secret token in the URL) ────────────────────────────────────

async fn feed(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> AppResult {
    let token = file.strip_suffix(".xml").unwrap_or(&file);
    let podcast = state
        .app
        .podcast_by_token(token)
        .await?
        .ok_or(AppError::NotFound)?;
    let episodes = state.app.feed_episodes(podcast.id).await?;
    let xml = rss::build_feed(&podcast, &episodes, &base_url(&state, &headers));
    Ok((
        [
            (CONTENT_TYPE, "application/rss+xml; charset=utf-8"),
            (CACHE_CONTROL, "private, max-age=60"),
        ],
        xml,
    )
        .into_response())
}

async fn feed_cover(
    State(state): State<AppState>,
    Path((token, _file)): Path<(String, String)>,
    request: Request,
) -> AppResult {
    let podcast = state
        .app
        .podcast_by_token(&token)
        .await?
        .ok_or(AppError::NotFound)?;
    serve_cover(&state, &podcast, request).await
}

async fn media(
    State(state): State<AppState>,
    Path((token, episode_id, _file)): Path<(String, i64, String)>,
    request: Request,
) -> AppResult {
    let podcast = state
        .app
        .podcast_by_token(&token)
        .await?
        .ok_or(AppError::NotFound)?;
    let episode = state
        .app
        .episode(podcast.id, episode_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let (Some(path), Some(mime)) = (&episode.media_path, &episode.media_mime) else {
        return Err(AppError::NotFound);
    };
    serve_file(&state.app.archive_path(path), mime, request).await
}

async fn serve_cover(state: &AppState, podcast: &Podcast, request: Request) -> AppResult {
    let file = podcast.cover_file.as_deref().ok_or(AppError::NotFound)?;
    let mime = file
        .rsplit_once('.')
        .and_then(|(_, ext)| uguisu_core::archive::mime_from_extension(ext))
        .unwrap_or("image/jpeg");
    let path = state.app.archive_path(&podcast.dir).join(file);
    serve_file(&path, mime, request).await
}

/// Streams a file with support for range requests, which podcast apps need for seeking.
async fn serve_file(path: &std::path::Path, mime: &str, request: Request) -> AppResult {
    let mime: mime::Mime = mime.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM);
    let response = ServeFile::new_with_mime(path, &mime)
        .oneshot(request)
        .await
        .map_err(|e| anyhow!("cannot serve file: {e}"))?;
    Ok(response.map(Body::new))
}
