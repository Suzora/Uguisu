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
    login_guard: std::sync::Arc<std::sync::Mutex<LoginGuard>>,
}

impl AppState {
    pub fn new(app: Uguisu, base_url: Option<String>) -> Self {
        Self {
            app,
            base_url,
            login_guard: Default::default(),
        }
    }
}

/// Slows down guessing the password. After a few wrong attempts in a row, the next attempt is
/// only looked at after a pause that doubles with every further failure, from four seconds up
/// to a minute. There
/// is one account and Uguisu usually sits behind a proxy, so the count is for everyone
/// together; the owner is held up by a minute at most, never locked out.
#[derive(Default)]
struct LoginGuard {
    failures: u32,
    last_failure: Option<std::time::Instant>,
}

impl LoginGuard {
    /// Wrong attempts that cost nothing extra.
    const FREE: u32 = 5;

    /// How long to wait before the next attempt is looked at.
    fn wait(&self, now: std::time::Instant) -> Option<Duration> {
        let last = self.last_failure?;
        let over = self.failures.checked_sub(Self::FREE)?;
        // Every attempt already takes about a second (password hashing and a fixed pause), so
        // the pauses start well above that: 4, 8, 16, 32 seconds, then a minute.
        let pause = Duration::from_secs(4u64 << over.min(4)).min(Duration::from_secs(60));
        pause
            .checked_sub(now.duration_since(last))
            .filter(|left| !left.is_zero())
    }

    fn failed(&mut self, now: std::time::Instant) {
        // A quiet quarter of an hour forgets earlier failures.
        if self
            .last_failure
            .is_some_and(|last| now.duration_since(last) > Duration::from_secs(900))
        {
            self.failures = 0;
        }
        self.failures += 1;
        self.last_failure = Some(now);
    }
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
        .route("/podcasts/{id}/episodes/{episode_id}", get(episode_page))
        .route("/podcasts/{id}/history", get(history_page))
        .route("/podcasts/{id}/snapshots/{snapshot_id}", get(snapshot_file))
        .route("/podcasts/{id}/refresh", post(refresh_podcast))
        .route("/podcasts/{id}/settings", post(update_settings))
        .route("/podcasts/{id}/token", post(regenerate_token))
        .route("/podcasts/{id}/remove", post(remove_podcast))
        .route("/cover/{id}", get(cover))
        .route("/opml", get(opml_export).post(opml_import))
        .route("/logout", post(logout))
        .route("/find", get(find))
        .route("/status", get(status_page))
        .route("/status/retry", post(retry_all))
        .route(
            "/podcasts/{id}/episodes/{episode_id}/retry",
            post(retry_episode),
        )
        .route("/settings", get(settings_page))
        .route("/settings/password", post(change_password))
        .route("/settings/sessions", post(end_other_sessions))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_login));

    let public = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/{file}", get(asset))
        .route("/setup", get(setup_page).post(setup))
        .route("/login", get(login_page).post(login))
        // Feeds are text that shrinks to a fraction; media files are compressed already.
        .route(
            "/feed/{file}",
            get(feed).layer(tower_http::compression::CompressionLayer::new()),
        )
        .route(
            "/feed/all/{file}",
            get(combined_feed).layer(tower_http::compression::CompressionLayer::new()),
        )
        .route("/media/{token}/{file}", get(feed_cover))
        .route("/media/{token}/{episode_id}/{file}", get(media))
        .route(
            "/media/{token}/{episode_id}/a/{attachment_id}/{file}",
            get(attachment),
        );

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
    // Archived files come from third parties. Whatever they contain and whatever type they
    // claim, a browser opening one must not run it with access to the interface.
    let is_archived_file = ["/media/", "/cover/"]
        .iter()
        .any(|prefix| request.uri().path().starts_with(prefix));
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(if is_archived_file {
            "sandbox; default-src 'none'; media-src 'self'; img-src 'self'"
        } else {
            "default-src 'self'; img-src 'self' https: data:; media-src 'self'; \
             frame-ancestors 'none'; form-action 'self'; base-uri 'none'"
        }),
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
        "favicon.png" => ("image/png", include_bytes!("../assets/favicon.png")),
        "icon.webp" => ("image/webp", include_bytes!("../assets/icon.webp")),
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
    let now = std::time::Instant::now();
    if let Some(wait) = state.login_guard.lock().unwrap().wait(now) {
        let seconds = wait.as_secs().max(1);
        let message = format!("Too many wrong attempts. Please try again in {seconds} seconds.");
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, seconds.to_string())],
            page(ui::login_page(Some(&message))),
        )
            .into_response());
    }
    if auth::check_password(state.app.pool(), form.password).await? {
        *state.login_guard.lock().unwrap() = LoginGuard::default();
        return start_session(&state, &headers).await;
    }
    state.login_guard.lock().unwrap().failed(now);
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

/// What the archive is doing and what needs attention.
async fn status_page(State(state): State<AppState>) -> AppResult {
    let app = &state.app;
    let podcasts = app.podcasts().await?;
    let stats = app.stats().await?;
    let problems = app.download_problems(200).await?;
    let damaged = app.damaged_files(200).await?;
    Ok(page(ui::status_page(&ui::StatusReport {
        podcasts: &podcasts,
        stats: &stats,
        problems: &problems,
        damaged: &damaged,
        downloading: app.downloading().len(),
        free_space: app.free_space(),
        reserve: app.space_reserve(),
    }))
    .into_response())
}

async fn retry_all(State(state): State<AppState>) -> AppResult {
    state.app.retry_all().await?;
    Ok(see_other("/status"))
}

async fn retry_episode(
    State(state): State<AppState>,
    Path((id, episode_id)): Path<(i64, i64)>,
) -> AppResult {
    state
        .app
        .episode(id, episode_id)
        .await?
        .ok_or(AppError::NotFound)?;
    state.app.retry_episode(episode_id).await?;
    Ok(see_other(&format!("/podcasts/{id}/episodes/{episode_id}")))
}

#[derive(Deserialize, Default)]
struct FindQuery {
    #[serde(default)]
    q: String,
}

/// Search in the titles and show notes of the archive.
async fn find(State(state): State<AppState>, Query(query): Query<FindQuery>) -> AppResult {
    let q = query.q.trim();
    let results = state.app.search_episodes(q, 100).await?;
    Ok(page(ui::find_page(q, &results)).into_response())
}

#[derive(Deserialize, Default)]
struct SettingsQuery {
    done: Option<String>,
}

async fn settings_page(
    State(state): State<AppState>,
    Query(query): Query<SettingsQuery>,
) -> AppResult {
    let sessions = auth::session_count(state.app.pool()).await?;
    let notice = match query.done.as_deref() {
        Some("password") => Some("The password is changed. Other devices have been signed out."),
        Some("sessions") => Some("All other devices have been signed out."),
        _ => None,
    };
    Ok(page(ui::settings_page(sessions, notice, None)).into_response())
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    password: String,
    confirm: String,
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<PasswordForm>,
) -> AppResult {
    let pool = state.app.pool();
    // The same pause as at the login: this form checks a password too.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let problem = if !auth::check_password(pool, form.current).await? {
        Some("The current password is not correct.".to_string())
    } else if form.password.chars().count() < auth::MIN_PASSWORD_LENGTH {
        Some(format!(
            "Please choose at least {} characters.",
            auth::MIN_PASSWORD_LENGTH
        ))
    } else if form.password != form.confirm {
        Some("The two new passwords do not match.".to_string())
    } else {
        None
    };
    if let Some(problem) = problem {
        let sessions = auth::session_count(pool).await?;
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            page(ui::settings_page(sessions, None, Some(&problem))),
        )
            .into_response());
    }
    auth::set_password(pool, form.password).await?;
    // Whoever knew the old password is signed out; this browser stays.
    auth::delete_other_sessions(pool, auth::session_token(&headers).as_deref()).await?;
    Ok(see_other("/settings?done=password"))
}

async fn end_other_sessions(State(state): State<AppState>, headers: HeaderMap) -> AppResult {
    auth::delete_other_sessions(state.app.pool(), auth::session_token(&headers).as_deref()).await?;
    Ok(see_other("/settings?done=sessions"))
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

async fn library(
    State(state): State<AppState>,
    Query(query): Query<LibraryQuery>,
    headers: HeaderMap,
) -> AppResult {
    let podcasts = state.app.podcasts().await?;
    let stats = state.app.stats().await?;
    let space_low = state.app.space_is_low();
    let combined = rss::combined_feed_url(
        &base_url(&state, &headers),
        &state.app.combined_feed_token().await?,
    );
    Ok(page(ui::library_page(
        &podcasts,
        &stats,
        query.import,
        space_low,
        &combined,
    ))
    .into_response())
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
    /// Checked: only episodes published from today on are downloaded.
    only_new: Option<String>,
}

async fn add_podcast(State(state): State<AppState>, Form(form): Form<AddForm>) -> AppResult {
    let archive_from = form.only_new.is_some().then(|| {
        let now = uguisu_core::util::now();
        now - now.rem_euclid(86_400)
    });
    // In a task of its own: a browser that gives up waiting must not stop the work half way,
    // which would leave a podcast without its files written.
    let added = {
        let app = state.app.clone();
        let url = form.url.clone();
        tokio::spawn(async move { app.add_podcast_from(&url, archive_from).await })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
    };
    match added {
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

#[derive(Deserialize, Default)]
struct PageQuery {
    page: Option<i64>,
}

async fn episode_page(
    State(state): State<AppState>,
    Path((id, episode_id)): Path<(i64, i64)>,
) -> AppResult {
    let podcast = state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    let view = state
        .app
        .episode(id, episode_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let media = state.app.media_files(episode_id).await?;
    let attachments = state.app.episode_attachments(episode_id).await?;
    let revisions = state.app.revisions(episode_id).await?;
    Ok(page(ui::episode_page(
        &podcast,
        &view,
        &media,
        &attachments,
        &revisions,
        &state.app.downloading(),
    ))
    .into_response())
}

/// What the source feed of a podcast did over time.
async fn history_page(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult {
    let podcast = state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    let snapshots = state.app.snapshots(id).await?;
    let events = state.app.history(id, 500).await?;
    Ok(page(ui::history_page(&podcast, &snapshots, &events)).into_response())
}

/// A stored version of the source feed, exactly as it was fetched. Offered as a download: it
/// is third-party content and is not to be shown as a page of this site.
async fn snapshot_file(
    State(state): State<AppState>,
    Path((id, snapshot_id)): Path<(i64, i64)>,
    request: Request,
) -> AppResult {
    let snapshot = state
        .app
        .snapshot(id, snapshot_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let path = state.app.archive_path(&snapshot.path);
    let name = snapshot.path.rsplit('/').next().unwrap_or("feed.xml");
    let mut response = serve_file(&path, "application/xml", request).await?;
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

async fn podcast_page(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> AppResult {
    let summary = state
        .app
        .podcast_summary(id)
        .await?
        .ok_or(AppError::NotFound)?;
    let page_number = query.page.unwrap_or(1).max(1);
    let episodes = state.app.episodes_page(id, page_number).await?;
    let feed_url = rss::feed_url(&base_url(&state, &headers), &summary.podcast.feed_token);
    Ok(page(ui::podcast_page(
        &summary,
        &episodes,
        &state.app.downloading(),
        &feed_url,
        page_number,
    ))
    .into_response())
}

async fn episodes_fragment(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<PageQuery>,
) -> AppResult {
    let summary = state
        .app
        .podcast_summary(id)
        .await?
        .ok_or(AppError::NotFound)?;
    let page_number = query.page.unwrap_or(1).max(1);
    let episodes = state.app.episodes_page(id, page_number).await?;
    Ok(page(ui::episode_list(
        &summary,
        &episodes,
        &state.app.downloading(),
        page_number,
    ))
    .into_response())
}

async fn refresh_podcast(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult {
    state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    // A failure is stored on the podcast and shown on its page.
    // In a task of its own, so that it finishes even if the browser stops waiting.
    let app = state.app.clone();
    match tokio::spawn(async move { app.refresh(id).await }).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(podcast_id = id, "refresh failed: {e:#}"),
        Err(e) => tracing::error!(podcast_id = id, "refresh stopped: {e}"),
    }
    Ok(see_other(&format!("/podcasts/{id}")))
}

#[derive(Deserialize)]
struct SettingsForm {
    include_removed: Option<String>,
    keep_versions: Option<String>,
    /// `YYYY-MM-DD`, or empty for no start date.
    archive_from: Option<String>,
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
    let archive_from = form
        .archive_from
        .as_deref()
        .map(str::trim)
        .filter(|date| !date.is_empty())
        .and_then(|date| uguisu_core::util::parse_rfc3339(&format!("{date}T00:00:00Z")));
    state.app.set_archive_from(id, archive_from).await?;
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

#[derive(Deserialize, Default)]
struct CoverQuery {
    /// `small` asks for a reduced copy, which is what the interface shows.
    size: Option<String>,
}

async fn cover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<CoverQuery>,
    request: Request,
) -> AppResult {
    let podcast = state.app.podcast(id).await?.ok_or(AppError::NotFound)?;
    if query.size.as_deref() == Some("small")
        && let Some(path) = small_cover(&state, &podcast).await
    {
        let mut response = serve_file(&path, "image/jpeg", request).await?;
        response.headers_mut().insert(
            CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=86400"),
        );
        return Ok(response);
    }
    serve_cover(&state, &podcast, request).await
}

/// Longest side of a reduced cover, enough for the largest place it is shown at double density.
const SMALL_COVER: u32 = 600;

/// Path of a reduced copy of the podcast's cover, made at first use. Covers are commonly
/// 3000 pixels wide and several megabytes, and the library shows all of them at once. The
/// copies live outside the archive, in `cache/`, and can be deleted at any time. `None` if
/// there is no cover or it cannot be read as an image; the original is served then.
async fn small_cover(state: &AppState, podcast: &Podcast) -> Option<std::path::PathBuf> {
    let file = podcast.cover_file.as_deref()?;
    let original = state.app.archive_path(&podcast.dir).join(file);
    let metadata = tokio::fs::metadata(&original).await.ok()?;
    let changed = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    // A new cover has another size or date, and so another name.
    let dir = state.app.config().data_dir.join("cache").join("covers");
    let target = dir.join(format!("{}-{}-{changed}.jpg", podcast.id, metadata.len()));
    if tokio::fs::try_exists(&target).await.unwrap_or(false) {
        return Some(target);
    }
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let written = target.clone();
    let made = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut reader = image::ImageReader::open(&original)?.with_guessed_format()?;
        // The file comes from somewhere on the internet: bound what decoding it may cost.
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(12_000);
        limits.max_image_height = Some(12_000);
        limits.max_alloc = Some(512 << 20);
        reader.limits(limits);
        let small = reader
            .decode()?
            .thumbnail(SMALL_COVER, SMALL_COVER)
            .to_rgb8();
        let part = written.with_extension("jpg.part");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&part)?);
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).encode_image(&small)?;
        drop(out);
        std::fs::rename(&part, &written)?;
        Ok(())
    })
    .await;
    match made {
        Ok(Ok(())) => Some(target),
        Ok(Err(error)) => {
            tracing::debug!(podcast = %podcast.title, "no reduced cover: {error:#}");
            None
        }
        Err(_) => None,
    }
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

#[derive(Deserialize, Default)]
struct FeedQuery {
    /// Publish only the newest episodes. Feeds of long-running podcasts grow to megabytes,
    /// which some apps handle badly.
    limit: Option<usize>,
}

async fn feed(
    State(state): State<AppState>,
    Path(file): Path<String>,
    Query(query): Query<FeedQuery>,
    headers: HeaderMap,
) -> AppResult {
    let token = file.strip_suffix(".xml").unwrap_or(&file);
    let podcast = state
        .app
        .podcast_by_token(token)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut episodes = state.app.feed_episodes(podcast.id).await?;
    if let Some(limit) = query.limit.filter(|limit| *limit > 0) {
        episodes = rss::newest(&podcast, episodes, limit);
    }
    let attachments = state.app.attachments(podcast.id).await?;
    let xml = rss::build_feed(
        &podcast,
        &episodes,
        &attachments,
        &base_url(&state, &headers),
    );
    Ok(feed_response(&headers, xml))
}

/// How many episodes the feed over all podcasts carries unless the address says otherwise.
const COMBINED_FEED_EPISODES: usize = 100;

async fn combined_feed(
    State(state): State<AppState>,
    Path(file): Path<String>,
    Query(query): Query<FeedQuery>,
    headers: HeaderMap,
) -> AppResult {
    let token = file.strip_suffix(".xml").unwrap_or(&file);
    // Compared in full, so that nothing about the token can be learned from a wrong guess.
    let expected = state.app.combined_feed_token().await?;
    if blake3::hash(token.as_bytes()) != blake3::hash(expected.as_bytes()) {
        return Err(AppError::NotFound);
    }
    let limit = query
        .limit
        .filter(|limit| *limit > 0)
        .unwrap_or(COMBINED_FEED_EPISODES)
        .min(1000);
    let episodes = state.app.newest_episodes(limit as i64).await?;
    let podcasts = state.app.podcasts().await?;
    let entries: Vec<(&Podcast, &uguisu_core::model::EpisodeView)> = episodes
        .iter()
        .filter_map(|view| {
            let summary = podcasts
                .iter()
                .find(|p| p.podcast.id == view.episode.podcast_id)?;
            Some((&summary.podcast, view))
        })
        .collect();
    let ids: Vec<i64> = episodes.iter().map(|view| view.episode.id).collect();
    let attachments = state.app.attachments_of(&ids).await?;
    let xml = rss::build_combined_feed(&entries, &attachments, &base_url(&state, &headers));
    Ok(feed_response(&headers, xml))
}

/// A feed document, or "not modified" if the client already has this very document.
fn feed_response(headers: &HeaderMap, xml: String) -> Response {
    // Podcast apps ask for their feeds every hour or so, and mostly nothing has changed. The
    // tag lets them find that out without downloading the feed again.
    let etag = format!("\"{}\"", &blake3::hash(xml.as_bytes()).to_hex()[..32]);
    let unchanged = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|sent| {
            sent.split(',')
                .map(|tag| tag.trim().trim_start_matches("W/"))
                .any(|tag| tag == etag || tag == "*")
        });
    let cache = [
        (CACHE_CONTROL, "private, max-age=60".to_string()),
        (axum::http::header::ETAG, etag),
    ];
    if unchanged {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    (
        cache,
        [(CONTENT_TYPE, "application/rss+xml; charset=utf-8")],
        xml,
    )
        .into_response()
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

async fn attachment(
    State(state): State<AppState>,
    Path((token, episode_id, attachment_id, _file)): Path<(String, i64, i64, String)>,
    request: Request,
) -> AppResult {
    let podcast = state
        .app
        .podcast_by_token(&token)
        .await?
        .ok_or(AppError::NotFound)?;
    let attachment = state
        .app
        .attachment(podcast.id, episode_id, attachment_id)
        .await?
        .ok_or(AppError::NotFound)?;
    serve_file(
        &state.app.archive_path(&attachment.path),
        &attachment.mime,
        request,
    )
    .await
}

async fn serve_cover(state: &AppState, podcast: &Podcast, request: Request) -> AppResult {
    let file = podcast.cover_file.as_deref().ok_or(AppError::NotFound)?;
    let mime = file
        .rsplit_once('.')
        .and_then(|(_, ext)| uguisu_core::archive::mime_from_extension(ext))
        .unwrap_or("image/jpeg");
    let path = state.app.archive_path(&podcast.dir).join(file);
    // Covers are often several megabytes and the library shows all of them.
    let mut response = serve_file(&path, mime, request).await?;
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    Ok(response)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    /// A router over an archive with one podcast (without episodes), and its feed address.
    async fn archive() -> (tempfile::TempDir, Router, String) {
        let data = tempfile::tempdir().unwrap();
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO podcasts (feed_url, original_feed_url, title, dir, feed_token, created_at) \
             VALUES ('https://example.org/feed', 'https://example.org/feed', 'Test Cast', 'test', \
             'secret-token', 0)",
        )
        .execute(app.pool())
        .await
        .unwrap();
        let router = router(AppState::new(app, Some("https://archive.example".into())));
        (data, router, "/feed/secret-token.xml".to_string())
    }

    #[tokio::test]
    async fn an_unchanged_feed_is_not_sent_again() {
        let (_data, router, address) = archive().await;
        let get = |extra: Option<(header::HeaderName, String)>| {
            let mut request = Request::get(&address);
            if let Some((name, value)) = extra {
                request = request.header(name, value);
            }
            router.clone().oneshot(request.body(Body::empty()).unwrap())
        };

        let first = get(None).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers()[header::ETAG].to_str().unwrap().to_string();
        assert!(etag.starts_with('"') && etag.len() > 10);

        let again = get(Some((header::IF_NONE_MATCH, etag.clone())))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(again.headers()[header::ETAG].to_str().unwrap(), etag);

        let other = get(Some((header::IF_NONE_MATCH, "\"something-else\"".into())))
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn feeds_are_compressed_on_request() {
        let (_data, router, address) = archive().await;
        let request = Request::get(&address)
            .header(header::ACCEPT_ENCODING, "gzip")
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");

        let plain = Request::get(&address).body(Body::empty()).unwrap();
        let response = router.oneshot(plain).await.unwrap();
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    }

    #[tokio::test]
    async fn a_feed_can_be_limited_to_the_newest_episodes() {
        let (_data, router, address) = archive().await;
        // Five archived episodes, one a day.
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(_data.path()))
            .await
            .unwrap();
        for n in 1..=5 {
            sqlx::query(
                "INSERT INTO episodes (id, podcast_id, guid, title, published_at, enclosure_url, \
                 dir, meta_hash, first_seen_at, last_seen_at) \
                 VALUES (?1, 1, 'ep-' || ?1, 'Episode ' || ?1, ?1 * 86400, 'https://x.test/a.mp3', \
                 'e' || ?1, 'h' || ?1, 0, 0)",
            )
            .bind(n)
            .execute(app.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
                 downloaded_at, verified_at) \
                 VALUES (?1, 'test/episodes/e' || ?1 || '/media.mp3', 1, 'b', 'audio/mpeg', 's', 0, 0)",
            )
            .bind(n)
            .execute(app.pool())
            .await
            .unwrap();
        }
        let titles = |query: &'static str| {
            let router = router.clone();
            let address = format!("{address}{query}");
            async move {
                let request = Request::get(&address).body(Body::empty()).unwrap();
                let response = router.oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                    .await
                    .unwrap();
                let feed = uguisu_core::feed::parse(&body).unwrap();
                feed.items
                    .into_iter()
                    .map(|item| item.title)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(titles("").await.len(), 5);
        assert_eq!(titles("?limit=2").await, ["Episode 5", "Episode 4"]);
        assert_eq!(titles("?limit=0").await.len(), 5, "0 means no limit");
        assert_eq!(titles("?limit=99").await.len(), 5);
    }

    #[tokio::test]
    async fn one_feed_covers_all_podcasts() {
        let (_data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(_data.path()))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO podcasts (feed_url, original_feed_url, title, dir, feed_token, created_at) \
             VALUES ('https://example.org/2', 'https://example.org/2', 'Other Cast', 'other', \
             'other-token', 0)",
        )
        .execute(app.pool())
        .await
        .unwrap();
        // The same GUID in both podcasts, one day apart.
        for (id, podcast) in [(1, 1), (2, 2)] {
            sqlx::query(
                "INSERT INTO episodes (id, podcast_id, guid, title, published_at, enclosure_url, \
                 dir, meta_hash, first_seen_at, last_seen_at) \
                 VALUES (?1, ?2, 'same-guid', 'Episode', ?1 * 86400, 'https://x.test/a.mp3', \
                 'e' || ?1, 'h' || ?1, 0, 0)",
            )
            .bind(id)
            .bind(podcast)
            .execute(app.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
                 downloaded_at, verified_at) \
                 VALUES (?1, 'p/episodes/e' || ?1 || '/media.mp3', 1, 'b', 'audio/mpeg', 's', 0, 0)",
            )
            .bind(id)
            .execute(app.pool())
            .await
            .unwrap();
        }
        let token = app.combined_feed_token().await.unwrap();
        assert_eq!(app.combined_feed_token().await.unwrap(), token);

        let request = Request::get(format!("/feed/all/{token}.xml"))
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(header::ETAG));
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let feed = uguisu_core::feed::parse(&body).unwrap();
        let titles: Vec<&str> = feed.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Other Cast – Episode", "Test Cast – Episode"]);
        assert_ne!(feed.items[0].guid, feed.items[1].guid);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("/media/other-token/2/media.mp3"));
        assert!(text.contains("/media/secret-token/1/media.mp3"));

        let wrong = Request::get("/feed/all/wrong.xml")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(wrong).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_interface_gets_reduced_covers() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        // A cover of 2000 by 1000 pixels in the archive.
        let dir = data.path().join("archive/test");
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbImage::from_pixel(2000, 1000, image::Rgb([90, 120, 60]))
            .save(dir.join("cover.png"))
            .unwrap();
        sqlx::query("UPDATE podcasts SET cover_file = 'cover.png'")
            .execute(app.pool())
            .await
            .unwrap();
        let session = crate::auth::create_session(app.pool()).await.unwrap();
        let get = |address: &'static str| {
            let request = Request::get(address)
                .header(header::COOKIE, format!("uguisu_session={session}"))
                .body(Body::empty())
                .unwrap();
            let router = router.clone();
            async move {
                let response = router.oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{address}");
                let kind = response.headers()[header::CONTENT_TYPE].clone();
                let body = axum::body::to_bytes(response.into_body(), 64 << 20)
                    .await
                    .unwrap();
                (kind, body)
            }
        };

        let (kind, original) = get("/cover/1").await;
        assert_eq!(kind, "image/png");
        let (kind, small) = get("/cover/1?size=small").await;
        assert_eq!(kind, "image/jpeg");
        let decoded = image::load_from_memory(&small).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (600, 300));
        assert_eq!(
            image::load_from_memory(&original).unwrap().width(),
            2000,
            "the archived cover is untouched"
        );
        // Made once, then served from the cache directory.
        let cached = std::fs::read_dir(data.path().join("cache/covers"))
            .unwrap()
            .count();
        assert_eq!(cached, 1);
        get("/cover/1?size=small").await;

        // Something that is no image: the original is served instead of an error.
        std::fs::write(dir.join("cover.png"), "not an image, and of another size").unwrap();
        let (kind, body) = get("/cover/1?size=small").await;
        assert_eq!(kind, "image/png");
        assert_eq!(&body[..], b"not an image, and of another size");
    }

    #[tokio::test]
    async fn the_episode_page_shows_what_is_archived() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO episodes (id, podcast_id, guid, title, description, published_at, \
             enclosure_url, dir, meta_hash, first_seen_at, last_seen_at) \
             VALUES (7, 1, 'ep-7', 'Seventh <heaven>', '<p>Notes &amp; <script>x()</script></p>', \
             86400, 'https://x.test/a.mp3', 'e7', 'new', 0, 0)",
        )
        .execute(app.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
             downloaded_at, verified_at) \
             VALUES (7, 'test/episodes/e7/media.mp3', 2048, 'abc123', 'audio/mpeg', 's', 0, 0)",
        )
        .execute(app.pool())
        .await
        .unwrap();
        for (seen, hash, title) in [(100, "old", "Sevnth"), (200, "new", "Seventh")] {
            sqlx::query(
                "INSERT INTO episode_revisions (episode_id, seen_at, meta_hash, raw_xml) \
                 VALUES (7, ?, ?, '<item><title>' || ? || '</title><guid>ep-7</guid></item>')",
            )
            .bind(seen)
            .bind(hash)
            .bind(title)
            .execute(app.pool())
            .await
            .unwrap();
        }
        let session = crate::auth::create_session(app.pool()).await.unwrap();
        let get = |address: &'static str, cookie: bool| {
            let mut request = Request::get(address);
            if cookie {
                request = request.header(header::COOKIE, format!("uguisu_session={session}"));
            }
            router.clone().oneshot(request.body(Body::empty()).unwrap())
        };

        let response = get("/podcasts/1/episodes/7", true).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let page = String::from_utf8_lossy(&body);
        assert!(
            page.contains("Seventh &lt;heaven&gt;"),
            "titles are escaped"
        );
        assert!(page.contains("media.mp3") && page.contains("abc123"));
        assert!(page.contains("Notes &amp; x()"), "show notes as text");
        assert!(!page.contains("<script>x()"), "never as markup");
        assert!(page.contains("− &lt;title&gt;Sevnth&lt;/title&gt;"));
        assert!(page.contains("+ &lt;title&gt;Seventh&lt;/title&gt;"));

        // Not without a login, and not under another podcast.
        let response = get("/podcasts/1/episodes/7", false).await.unwrap();
        assert_ne!(response.status(), StatusCode::OK);
        let response = get("/podcasts/2/episodes/7", true).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_password_can_be_changed_and_devices_signed_out() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        let pool = app.pool();
        crate::auth::set_password(pool, "the first password".into())
            .await
            .unwrap();
        let here = crate::auth::create_session(pool).await.unwrap();
        let elsewhere = crate::auth::create_session(pool).await.unwrap();
        let post = |address: &'static str, body: &'static str| {
            let request = Request::post(address)
                .header(header::COOKIE, format!("uguisu_session={here}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(body))
                .unwrap();
            router.clone().oneshot(request)
        };

        // A wrong current password, a short one and a mismatch change nothing.
        for body in [
            "current=wrong&password=the+second+password&confirm=the+second+password",
            "current=the+first+password&password=short&confirm=short",
            "current=the+first+password&password=the+second+password&confirm=something+else",
        ] {
            let response = post("/settings/password", body).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{body}"
            );
        }
        assert!(
            crate::auth::check_password(pool, "the first password".into())
                .await
                .unwrap()
        );
        assert!(
            crate::auth::session_is_valid(pool, &elsewhere)
                .await
                .unwrap()
        );

        let response = post(
            "/settings/password",
            "current=the+first+password&password=the+second+password&confirm=the+second+password",
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            crate::auth::check_password(pool, "the second password".into())
                .await
                .unwrap()
        );
        assert!(
            !crate::auth::check_password(pool, "the first password".into())
                .await
                .unwrap()
        );
        assert!(crate::auth::session_is_valid(pool, &here).await.unwrap());
        assert!(
            !crate::auth::session_is_valid(pool, &elsewhere)
                .await
                .unwrap(),
            "other devices are signed out"
        );

        // Signing out other devices on its own.
        let another = crate::auth::create_session(pool).await.unwrap();
        let response = post("/settings/sessions", "").await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(!crate::auth::session_is_valid(pool, &another).await.unwrap());
        assert!(crate::auth::session_is_valid(pool, &here).await.unwrap());

        // The owner's way back in after losing the password.
        crate::auth::reset_password(pool).await.unwrap();
        assert!(!crate::auth::password_is_set(pool).await.unwrap());
        assert!(!crate::auth::session_is_valid(pool, &here).await.unwrap());
    }

    #[tokio::test]
    async fn the_archive_can_be_searched_from_the_interface() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO episodes (id, podcast_id, guid, title, description, published_at, dir, \
             meta_hash, first_seen_at, last_seen_at) \
             VALUES (3, 1, 'ep-3', 'Dawn chorus', '<p>Robins &amp; <i>wrens</i></p>', 0, 'e3', 'h', 0, 0)",
        )
        .execute(app.pool())
        .await
        .unwrap();
        app.sync_search_index().await.unwrap();
        let session = crate::auth::create_session(app.pool()).await.unwrap();
        let get = |address: &'static str| {
            let request = Request::get(address)
                .header(header::COOKIE, format!("uguisu_session={session}"))
                .body(Body::empty())
                .unwrap();
            let router = router.clone();
            async move {
                let response = router.oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{address}");
                let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                    .await
                    .unwrap();
                String::from_utf8_lossy(&body).into_owned()
            }
        };
        let page = get("/find?q=wrens").await;
        assert!(page.contains("/podcasts/1/episodes/3"));
        assert!(page.contains("Dawn chorus"));
        assert!(page.contains("Robins &amp; <mark>wrens</mark>"), "{page}");
        assert!(get("/find?q=nothing-like-it").await.contains("No episode"));
        // Nothing typed, and something that would be query syntax elsewhere.
        get("/find").await;
        get("/find?q=%22%3Cscript%3E+OR+*").await;
    }

    #[tokio::test]
    async fn the_status_page_shows_what_needs_attention() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        let pool = app.pool();
        sqlx::query("UPDATE podcasts SET last_fetch_error = 'the feed server answered 404'")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO episodes (id, podcast_id, guid, title, published_at, enclosure_url, dir, \
             meta_hash, first_seen_at, last_seen_at, download_attempts, download_error) VALUES \
             (1, 1, 'a', 'Gave up', 0, 'https://x.test/a.mp3', 'e1', 'h', 0, 0, 5, 'the media server answered 410'), \
             (2, 1, 'b', 'Broken copy', 0, 'https://x.test/b.mp3', 'e2', 'h', 0, 0, 0, NULL)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO media_files (episode_id, path, size, blake3, mime, source_url, \
             downloaded_at, verified_at, status) \
             VALUES (2, 'test/episodes/e2/media.mp3', 1, 'b', 'audio/mpeg', 's', 0, 0, 'corrupt')",
        )
        .execute(pool)
        .await
        .unwrap();
        let session = crate::auth::create_session(pool).await.unwrap();
        let send = |request: axum::http::request::Builder| {
            let request = request
                .header(header::COOKIE, format!("uguisu_session={session}"))
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap();
            router.clone().oneshot(request)
        };

        let response = send(Request::get("/status")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let page = String::from_utf8_lossy(&body);
        for expected in [
            "the feed server answered 404",
            "Gave up",
            "the media server answered 410",
            "Broken copy",
            "/podcasts/1/episodes/2",
        ] {
            assert!(page.contains(expected), "missing: {expected}");
        }

        // Trying again gives the episode its attempts back.
        let response = send(Request::post("/podcasts/1/episodes/1/retry"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let (attempts, error): (i64, Option<String>) =
            sqlx::query_as("SELECT download_attempts, download_error FROM episodes WHERE id = 1")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!((attempts, error), (0, None));
        let response = send(Request::post("/podcasts/2/episodes/1/retry"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = send(Request::post("/status/retry")).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }

    /// A password set with an earlier version of the hashing library still opens the door.
    #[tokio::test]
    async fn passwords_from_earlier_versions_still_work() {
        let (data, _router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        // Written by Uguisu 0.3.0 for the password below.
        let stored = "$argon2id$v=19$m=19456,t=2,p=1$MTMxMGU2Yjg2ZTExNGU5NWRjNWE1MjI0MmUzMDhmYzU$x9kdARS62eRvYWZC5gOt8ATDRVVDdr1xLJ6nstnyvrk";
        uguisu_core::db::set_setting(app.pool(), "admin_password_hash", stored)
            .await
            .unwrap();
        assert!(
            crate::auth::check_password(app.pool(), "correct horse battery staple".into())
                .await
                .unwrap()
        );
        assert!(
            !crate::auth::check_password(app.pool(), "correct horse battery stapler".into())
                .await
                .unwrap()
        );
    }

    #[test]
    fn guessing_the_password_gets_slower() {
        let start = std::time::Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let wait = |guard: &LoginGuard, seconds: u64| guard.wait(at(seconds)).map(|w| w.as_secs());
        let mut guard = LoginGuard::default();
        for _ in 0..4 {
            guard.failed(at(0));
            assert_eq!(wait(&guard, 0), None, "the first failures cost nothing");
        }
        guard.failed(at(0));
        assert_eq!(wait(&guard, 0), Some(4));
        assert_eq!(wait(&guard, 3), Some(1));
        assert_eq!(wait(&guard, 4), None);
        guard.failed(at(4));
        assert_eq!(wait(&guard, 4), Some(8));
        for _ in 0..20 {
            guard.failed(at(12));
        }
        assert_eq!(wait(&guard, 12), Some(60), "never more than a minute");
        assert_eq!(wait(&guard, 72), None);
        // After a quiet quarter of an hour the count starts over.
        guard.failed(at(2000));
        assert_eq!(wait(&guard, 2000), None);
    }

    #[tokio::test]
    async fn the_login_refuses_attempts_that_come_too_fast() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        crate::auth::set_password(app.pool(), "the right password".into())
            .await
            .unwrap();
        let attempt = |password: &'static str| {
            let request = Request::post("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("password={password}")))
                .unwrap();
            router.clone().oneshot(request)
        };
        for _ in 0..5 {
            let response = attempt("wrong").await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        // The sixth comes within the pause: it is not even checked, right or wrong.
        let response = attempt("the+right+password").await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key(header::RETRY_AFTER));
        // After the pause the right password works and clears the count.
        tokio::time::sleep(Duration::from_millis(4100)).await;
        let response = attempt("the+right+password").await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let response = attempt("wrong").await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_history_of_a_podcast_is_shown() {
        let (data, router, _) = archive().await;
        let app = uguisu_core::Uguisu::open(uguisu_core::Config::new(data.path()))
            .await
            .unwrap();
        let pool = app.pool();
        std::fs::create_dir_all(data.path().join("archive/test/feeds")).unwrap();
        std::fs::write(
            data.path()
                .join("archive/test/feeds/20240101T000000Z-abcd1234.xml"),
            "<rss>the feed as it was</rss>",
        )
        .unwrap();
        sqlx::query(
            "INSERT INTO feed_snapshots (id, podcast_id, fetched_at, blake3, size, path) \
             VALUES (4, 1, 1704067200, 'abcd1234', 29, 'test/feeds/20240101T000000Z-abcd1234.xml')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO episodes (id, podcast_id, guid, title, dir, meta_hash, first_seen_at, \
             last_seen_at, removed_at) VALUES \
             (1, 1, 'a', 'Stayed and changed', 'e1', 'h2', 100, 100, NULL), \
             (2, 1, 'b', 'Left the feed', 'e2', 'h', 100, 100, 300)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO episode_revisions (episode_id, seen_at, meta_hash, raw_xml) VALUES \
             (1, 100, 'h1', '<item/>'), (1, 200, 'h2', '<item/>')",
        )
        .execute(pool)
        .await
        .unwrap();

        let events = app.history(1, 50).await.unwrap();
        let summary: Vec<(i64, &str, &str)> = events
            .iter()
            .map(|event| (event.at, event.kind.as_str(), event.title.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (300, "removed", "Left the feed"),
                (200, "changed", "Stayed and changed"),
                (100, "added", "Left the feed"),
                (100, "added", "Stayed and changed"),
            ]
        );

        let session = crate::auth::create_session(pool).await.unwrap();
        let get = |address: &'static str, cookie: bool| {
            let mut request = Request::get(address);
            if cookie {
                request = request.header(header::COOKIE, format!("uguisu_session={session}"));
            }
            router.clone().oneshot(request.body(Body::empty()).unwrap())
        };
        let response = get("/podcasts/1/history", true).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let page = String::from_utf8_lossy(&body);
        for expected in [
            "Left the feed",
            "Stayed and changed",
            "/podcasts/1/snapshots/4",
        ] {
            assert!(page.contains(expected), "missing: {expected}");
        }

        // The stored feed is handed out as a file, byte for byte, and only to the owner.
        let response = get("/podcasts/1/snapshots/4", true).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment")
        );
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<rss>the feed as it was</rss>");
        let response = get("/podcasts/1/snapshots/4", false).await.unwrap();
        assert_ne!(response.status(), StatusCode::OK);
        let response = get("/podcasts/2/snapshots/4", true).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_wrong_feed_address_reveals_nothing() {
        let (_data, router, _) = archive().await;
        let request = Request::get("/feed/not-the-token.xml")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
