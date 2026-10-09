//! Talks to a running Uguisu server's API (`--server`).

use uguisu_core::archive::{ArchiveFile, ArchivePolicy, VerifyDepth};
use uguisu_core::download::{DownloadControl, DownloadJob, Priority};
use uguisu_core::secret::Secret;
use uguisu_discovery::{ResolveFailure, ResolvedFeed, SearchRequest, SearchResponse};
use uguisu_download::{
    DownloadStats, EnqueueOutcome, EnqueueSummary, JobDetail, JobPage, ReconcileReport,
};
use uguisu_engine::archive::{
    ArchiveReconcileReport, PathPreview, Relocation, VerifiedFile, VerifySummary,
};
use uguisu_http::{ClientConfig, GetOptions, HeaderName, HeaderValue, HttpClient, Profile, Url};

/// Errors talking to the server.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Bad server URL.
    #[error("invalid server url: {0}")]
    Url(String),
    /// Transport.
    #[error("server request failed: {0}")]
    Http(#[from] uguisu_http::HttpError),
    /// Unexpected status.
    #[error("server answered http {0}: {1}")]
    Status(u16, String),
    /// Body could not be parsed.
    #[error("unexpected response: {0}")]
    Decode(String),
    /// The server answered with a structured error.
    #[error("{message}")]
    Api {
        /// HTTP status.
        status: u16,
        /// Error kind.
        kind: String,
        /// Message.
        message: String,
    },
}

/// A thin client for the server API (`/api/v1/*`).
pub struct ApiClient {
    http: HttpClient,
    base: Url,
    /// Pre-built `Authorization` header, so the token is turned into one value
    /// once — at construction, where a bad one can still be reported.
    authorization: Option<HeaderValue>,
}

impl ApiClient {
    /// Creates a client for `server` (e.g. `http://127.0.0.1:8484`).
    ///
    /// A token that cannot become a header value is an error rather than a
    /// silently anonymous client: the old code dropped it with `.ok()` and
    /// every request then went out unauthenticated, which looks exactly like a
    /// server that does not need a credential.
    pub fn new(server: &str, token: Option<&Secret<String>>) -> Result<Self, ClientError> {
        let base = Url::parse(&format!("{}/", server.trim_end_matches('/')))
            .map_err(|e| ClientError::Url(e.to_string()))?;
        let authorization = match token {
            None => None,
            Some(token) => Some(
                HeaderValue::from_str(&format!("Bearer {}", token.expose())).map_err(|_| {
                    // Never the value, not even the length.
                    ClientError::Url(
                        "the API token contains characters a header cannot carry".to_owned(),
                    )
                })?,
            ),
        };
        let http = HttpClient::new(Profile::Trusted, ClientConfig::default())?;
        Ok(Self {
            http,
            base,
            authorization,
        })
    }

    fn headers(&self) -> Vec<(HeaderName, HeaderValue)> {
        self.authorization
            .clone()
            .map(|v| vec![(HeaderName::from_static("authorization"), v)])
            .unwrap_or_default()
    }

    /// `GET /api/v1/discovery/search`.
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError> {
        let mut url = self
            .base
            .join("api/v1/discovery/search")
            .map_err(|e| ClientError::Url(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("q", &req.query);
            if let Some(p) = &req.providers {
                q.append_pair(
                    "providers",
                    &p.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            if let Some(l) = req.limit {
                q.append_pair("limit", &l.to_string());
            }
            if let Some(c) = &req.country {
                q.append_pair("country", c);
            }
            if req.no_cache {
                q.append_pair("no_cache", "true");
            }
        }
        let resp = self
            .http
            .get_with(
                &url,
                &GetOptions {
                    headers: self.headers(),
                    ..GetOptions::default()
                },
            )
            .await?;
        if !resp.status.is_success() {
            return Err(ClientError::Status(
                resp.status.as_u16(),
                String::from_utf8_lossy(&resp.body).into_owned(),
            ));
        }
        serde_json::from_slice(&resp.body).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// `GET /api/v1/discovery/resolve?input=` (the API also accepts POST; GET keeps this client read-only).
    pub async fn resolve(
        &self,
        input: &str,
    ) -> Result<Result<ResolvedFeed, ResolveFailure>, ClientError> {
        let mut url = self
            .base
            .join("api/v1/discovery/resolve")
            .map_err(|e| ClientError::Url(e.to_string()))?;
        url.query_pairs_mut().append_pair("input", input);
        let resp = self
            .http
            .get_with(
                &url,
                &GetOptions {
                    headers: self.headers(),
                    ..GetOptions::default()
                },
            )
            .await?;
        if resp.status.is_success() {
            return serde_json::from_slice::<ResolvedFeed>(&resp.body)
                .map(Ok)
                .map_err(|e| ClientError::Decode(e.to_string()));
        }
        // The route answers the standard envelope, and carries the tagged
        // failure under `error.detail` so remote output matches embedded
        // output exactly - `render_failure` reads fields no message has.
        if let Ok(envelope) = serde_json::from_slice::<ResolveFailureEnvelope>(&resp.body) {
            return Ok(Err(ResolveFailure {
                error: envelope.error.detail,
                provenance: envelope.provenance,
            }));
        }
        Err(ClientError::Status(
            resp.status.as_u16(),
            String::from_utf8_lossy(&resp.body).into_owned(),
        ))
    }
}

#[derive(serde::Deserialize)]
struct ResolveFailureEnvelope {
    error: ResolveErrorEnvelope,
    provenance: Vec<uguisu_discovery::ResolutionStep>,
}

#[derive(serde::Deserialize)]
struct ResolveErrorEnvelope {
    detail: uguisu_discovery::ResolveError,
}

// Library and feed API.

/// An error body returned by the server.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ServerError {
    /// Error kind, from the one vocabulary the API answers with (ADR 0038).
    pub kind: String,
    /// Message.
    pub message: String,
}

#[derive(serde::Deserialize)]
struct ErrorWrapper {
    error: ServerError,
}

/// How long an import over the API may take (ADR 0049, ADR 0050).
const IMPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// The server's error envelope, or the status and body when it sent none.
fn failure(resp: &uguisu_http::Response) -> ClientError {
    if let Ok(w) = serde_json::from_slice::<ErrorWrapper>(&resp.body) {
        return ClientError::Api {
            status: resp.status.as_u16(),
            kind: w.error.kind,
            message: w.error.message,
        };
    }
    ClientError::Status(
        resp.status.as_u16(),
        String::from_utf8_lossy(&resp.body).into_owned(),
    )
}

impl ApiClient {
    /// `GET /api/v1/health`, which needs no credential.
    pub async fn health(&self) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint("api/v1/health")?;
        self.request("GET", &url, None).await
    }

    fn endpoint(&self, path: &str) -> Result<Url, ClientError> {
        self.base
            .join(path)
            .map_err(|e| ClientError::Url(e.to_string()))
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        url: &Url,
        body: Option<serde_json::Value>,
    ) -> Result<T, ClientError> {
        let headers = self.headers();
        let resp = if method == "GET" {
            self.http
                .get_with(
                    url,
                    &GetOptions {
                        headers,
                        ..GetOptions::default()
                    },
                )
                .await?
        } else {
            let payload = body.unwrap_or(serde_json::Value::Null).to_string();
            self.http
                .post(
                    url,
                    uguisu_http::Bytes::from(payload),
                    "application/json",
                    &GetOptions {
                        headers,
                        retry: Some(false),
                        ..GetOptions::default()
                    },
                )
                .await?
        };
        if resp.status.is_success() {
            return serde_json::from_slice(&resp.body)
                .map_err(|e| ClientError::Decode(e.to_string()));
        }
        Err(failure(&resp))
    }

    /// Posts where success is `204`, so there is no body to decode.
    async fn no_content(
        &self,
        url: &Url,
        body: Option<serde_json::Value>,
    ) -> Result<(), ClientError> {
        let payload = body.unwrap_or(serde_json::Value::Null).to_string();
        let resp = self
            .http
            .post(
                url,
                uguisu_http::Bytes::from(payload),
                "application/json",
                &GetOptions {
                    headers: self.headers(),
                    retry: Some(false),
                    ..GetOptions::default()
                },
            )
            .await?;
        if resp.status.is_success() {
            return Ok(());
        }
        Err(failure(&resp))
    }

    /// `POST /api/v1/podcasts/opml`. Waits up to [`IMPORT_TIMEOUT`]: applying
    /// resolves every new feed, which takes minutes for a few hundred.
    pub async fn import_opml(
        &self,
        body: &serde_json::Value,
    ) -> Result<uguisu_engine::opml::OpmlImport, ClientError> {
        self.long_request("api/v1/podcasts/opml", Some(body)).await
    }

    /// `POST /api/v1/archive/import`. Waits up to [`IMPORT_TIMEOUT`]: the
    /// server scans, hashes and copies a whole archive before it answers.
    pub async fn import_archive(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        self.long_request("api/v1/archive/import", Some(body)).await
    }

    /// `POST /api/v1/archive/restore`. Waits up to [`IMPORT_TIMEOUT`]: the
    /// server hashes candidate files and copies them back before it answers.
    pub async fn restore_archive(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        self.long_request("api/v1/archive/restore", Some(body))
            .await
    }

    /// `POST /api/v1/archive/{episode_id}/redownload`.
    pub async fn redownload(&self, episode_id: &str) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(&format!("api/v1/archive/{episode_id}/redownload"))?;
        self.request("POST", &url, None).await
    }

    /// `GET /api/v1/archive/orphans`. Waits up to [`IMPORT_TIMEOUT`]: the
    /// server walks the whole media directory before it answers.
    pub async fn archive_orphans(&self) -> Result<serde_json::Value, ClientError> {
        self.long_request("api/v1/archive/orphans", None).await
    }

    /// A request that may take as long as an import does, on its own client
    /// so every other request keeps the default timeout: a POST of `body`,
    /// or a GET without one.
    async fn long_request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, ClientError> {
        let url = self.endpoint(path)?;
        let http = HttpClient::new(
            Profile::Trusted,
            ClientConfig {
                request_timeout: IMPORT_TIMEOUT,
                ..ClientConfig::default()
            },
        )?;
        let options = GetOptions {
            headers: self.headers(),
            retry: Some(false),
            ..GetOptions::default()
        };
        let resp = match body {
            Some(body) => {
                http.post(
                    &url,
                    uguisu_http::Bytes::from(body.to_string()),
                    "application/json",
                    &options,
                )
                .await?
            }
            None => http.get_with(&url, &options).await?,
        };
        if resp.status.is_success() {
            return serde_json::from_slice(&resp.body)
                .map_err(|e| ClientError::Decode(e.to_string()));
        }
        Err(failure(&resp))
    }

    /// `GET /api/v1/podcasts/opml`: the document as the server wrote it.
    pub async fn export_opml(&self) -> Result<String, ClientError> {
        let url = self.endpoint("api/v1/podcasts/opml")?;
        let resp = self
            .http
            .get_with(
                &url,
                &GetOptions {
                    headers: self.headers(),
                    ..GetOptions::default()
                },
            )
            .await?;
        if resp.status.is_success() {
            return String::from_utf8(resp.body.to_vec())
                .map_err(|e| ClientError::Decode(e.to_string()));
        }
        Err(failure(&resp))
    }

    /// `POST /api/v1/podcasts`.
    pub async fn add_podcast(
        &self,
        input: &str,
    ) -> Result<uguisu_engine::library::AddOutcome, ClientError> {
        let url = self.endpoint("api/v1/podcasts")?;
        self.request("POST", &url, Some(serde_json::json!({ "input": input })))
            .await
    }

    /// `POST /api/v1/podcasts/{id}/move-feed`.
    pub async fn move_feed(
        &self,
        id: &str,
        feed_url: &str,
        options: uguisu_engine::migration::MoveOptions,
    ) -> Result<uguisu_engine::migration::FeedMove, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{id}/move-feed"))?;
        self.request(
            "POST",
            &url,
            Some(serde_json::json!({
                "url": feed_url,
                "dry_run": options.dry_run,
                "force": options.force,
            })),
        )
        .await
    }

    /// A podcast's episodes, newest first, following the cursor to the end (ADR 0040).
    pub async fn episodes(
        &self,
        podcast_id: &str,
    ) -> Result<Vec<uguisu_core::model::Episode>, ClientError> {
        let mut episodes = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut url = self.endpoint(&format!("api/v1/podcasts/{podcast_id}/episodes"))?;
            {
                let mut q = url.query_pairs_mut();
                if let Some(a) = &after {
                    q.append_pair("after", a);
                }
                q.append_pair("limit", &uguisu_core::page::MAX.to_string());
            }
            let page: uguisu_engine::library::EpisodePage = self.request("GET", &url, None).await?;
            episodes.extend(page.episodes);
            match page.next_after {
                Some(cursor) => after = Some(cursor.to_string()),
                None => return Ok(episodes),
            }
        }
    }

    /// `GET /api/v1/episodes/{id}`.
    pub async fn episode(
        &self,
        id: &str,
    ) -> Result<uguisu_engine::library::EpisodeDetail, ClientError> {
        let url = self.endpoint(&format!("api/v1/episodes/{id}"))?;
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/db/backup`: the copy, in the server's `backups/`.
    pub async fn db_backup(&self) -> Result<uguisu_engine::db::DbBackup, ClientError> {
        self.long_request("api/v1/db/backup", Some(&serde_json::json!({})))
            .await
    }

    /// `POST /api/v1/db/check`.
    pub async fn db_check(&self) -> Result<uguisu_engine::db::DbCheck, ClientError> {
        self.long_request("api/v1/db/check", Some(&serde_json::json!({})))
            .await
    }

    /// `POST /api/v1/db/vacuum`.
    pub async fn db_vacuum(&self) -> Result<uguisu_engine::db::DbVacuum, ClientError> {
        self.long_request("api/v1/db/vacuum", Some(&serde_json::json!({})))
            .await
    }

    /// `POST /api/v1/podcasts/{id}/archive`: the podcast's status afterwards.
    pub async fn archive_podcast(
        &self,
        id: &str,
    ) -> Result<uguisu_core::model::PodcastStatus, ClientError> {
        #[derive(serde::Deserialize)]
        struct Archived {
            status: uguisu_core::model::PodcastStatus,
        }
        let url = self.endpoint(&format!("api/v1/podcasts/{id}/archive"))?;
        let archived: Archived = self.request("POST", &url, None).await?;
        Ok(archived.status)
    }

    /// `POST /api/v1/podcasts/{id}/remove` (the `DELETE` route's alias).
    pub async fn remove_podcast(
        &self,
        id: &str,
    ) -> Result<uguisu_engine::library::PodcastRemoval, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{id}/remove"))?;
        self.request("POST", &url, None).await
    }

    /// Every podcast, following `GET /api/v1/podcasts`'s cursor to the end (ADR 0040).
    pub async fn podcasts(
        &self,
    ) -> Result<Vec<uguisu_engine::library::PodcastDetail>, ClientError> {
        let mut podcasts = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut url = self.endpoint("api/v1/podcasts")?;
            {
                let mut q = url.query_pairs_mut();
                if let Some(a) = &after {
                    q.append_pair("after", a);
                }
                q.append_pair("limit", &uguisu_core::page::MAX.to_string());
            }
            let page: uguisu_engine::library::PodcastPage = self.request("GET", &url, None).await?;
            podcasts.extend(page.podcasts);
            match page.next_after {
                Some(cursor) => after = Some(cursor.to_string()),
                None => return Ok(podcasts),
            }
        }
    }

    /// `GET /api/v1/episodes/duplicates`, every page.
    pub async fn duplicates(
        &self,
        podcast: Option<&str>,
    ) -> Result<Vec<uguisu_engine::duplicates::DuplicatePair>, ClientError> {
        let mut duplicates = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut url = self.endpoint("api/v1/episodes/duplicates")?;
            {
                let mut q = url.query_pairs_mut();
                if let Some(p) = podcast {
                    q.append_pair("podcast", p);
                }
                if let Some(a) = &after {
                    q.append_pair("after", a);
                }
                q.append_pair("limit", &uguisu_core::page::MAX.to_string());
            }
            let page: uguisu_engine::duplicates::DuplicatePage =
                self.request("GET", &url, None).await?;
            duplicates.extend(page.duplicates);
            match page.next_after {
                Some(cursor) => after = Some(cursor.to_string()),
                None => return Ok(duplicates),
            }
        }
    }

    /// `POST /api/v1/episodes/{id}/resolve`.
    pub async fn resolve_duplicate(
        &self,
        id: &str,
        resolution: uguisu_core::model::DuplicateResolution,
    ) -> Result<uguisu_engine::duplicates::DuplicateResolved, ClientError> {
        let url = self.endpoint(&format!("api/v1/episodes/{id}/resolve"))?;
        self.request(
            "POST",
            &url,
            Some(serde_json::json!({ "resolution": resolution })),
        )
        .await
    }

    /// `GET /api/v1/podcasts/{id}`.
    pub async fn podcast(
        &self,
        id: &str,
    ) -> Result<uguisu_engine::library::PodcastDetail, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{id}"))?;
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/podcasts/{id}/refresh`.
    pub async fn refresh(
        &self,
        id: &str,
        force: bool,
    ) -> Result<uguisu_core::feed::RefreshReport, ClientError> {
        let mut url = self.endpoint(&format!("api/v1/podcasts/{id}/refresh"))?;
        if force {
            url.query_pairs_mut().append_pair("force", "true");
        }
        self.request("POST", &url, None).await
    }

    /// `POST /api/v1/podcasts/refresh`.
    pub async fn refresh_all(
        &self,
        force: bool,
    ) -> Result<Vec<uguisu_engine::RefreshAllEntry>, ClientError> {
        #[derive(serde::Deserialize)]
        struct List {
            entries: Vec<uguisu_engine::RefreshAllEntry>,
        }
        let mut url = self.endpoint("api/v1/podcasts/refresh")?;
        if force {
            url.query_pairs_mut().append_pair("force", "true");
        }
        let list: List = self.request("POST", &url, None).await?;
        Ok(list.entries)
    }

    /// `GET /api/v1/feeds/{source_id}/status`.
    pub async fn feed_status(
        &self,
        source_id: &str,
    ) -> Result<uguisu_server::FeedStatus, ClientError> {
        let url = self.endpoint(&format!("api/v1/feeds/{source_id}/status"))?;
        self.request("GET", &url, None).await
    }

    /// `GET /api/v1/feeds/inspect?url=`.
    pub async fn inspect(&self, feed_url: &str) -> Result<uguisu_engine::Inspection, ClientError> {
        let mut url = self.endpoint("api/v1/feeds/inspect")?;
        url.query_pairs_mut().append_pair("url", feed_url);
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/downloads`.
    pub async fn enqueue(
        &self,
        episode_id: &str,
        priority: Priority,
    ) -> Result<EnqueueOutcome, ClientError> {
        let url = self.endpoint("api/v1/downloads")?;
        self.request(
            "POST",
            &url,
            Some(serde_json::json!({ "episode_id": episode_id, "priority": priority })),
        )
        .await
    }

    /// `POST /api/v1/podcasts/{id}/downloads`.
    pub async fn enqueue_podcast(
        &self,
        podcast_id: &str,
        priority: Priority,
    ) -> Result<EnqueueSummary, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{podcast_id}/downloads"))?;
        self.request(
            "POST",
            &url,
            Some(serde_json::json!({ "priority": priority })),
        )
        .await
    }

    /// `GET /api/v1/downloads?state=&podcast=&after=&limit=`.
    pub async fn downloads(
        &self,
        state: Option<&str>,
        podcast: Option<&str>,
        after: Option<&str>,
        limit: u32,
    ) -> Result<JobPage, ClientError> {
        let mut url = self.endpoint("api/v1/downloads")?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(s) = state {
                q.append_pair("state", s);
            }
            if let Some(p) = podcast {
                q.append_pair("podcast", p);
            }
            if let Some(a) = after {
                q.append_pair("after", a);
            }
            q.append_pair("limit", &limit.to_string());
        }
        self.request("GET", &url, None).await
    }

    /// `GET /api/v1/downloads/{id}`.
    pub async fn download(&self, id: &str) -> Result<JobDetail, ClientError> {
        let url = self.endpoint(&format!("api/v1/downloads/{id}"))?;
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/downloads/{id}/{cancel|pause|resume|retry}`.
    pub async fn download_command(
        &self,
        id: &str,
        command: &str,
    ) -> Result<DownloadJob, ClientError> {
        #[derive(serde::Deserialize)]
        struct Body {
            job: DownloadJob,
        }
        let url = self.endpoint(&format!("api/v1/downloads/{id}/{command}"))?;
        let body: Body = self.request("POST", &url, None).await?;
        Ok(body.job)
    }

    /// `POST /api/v1/downloads/retry-failed`.
    pub async fn retry_failed(&self) -> Result<u64, ClientError> {
        #[derive(serde::Deserialize)]
        struct Body {
            requeued: u64,
        }
        let url = self.endpoint("api/v1/downloads/retry-failed")?;
        let body: Body = self.request("POST", &url, None).await?;
        Ok(body.requeued)
    }

    /// `POST /api/v1/downloads/{pause|resume}`.
    pub async fn download_control(&self, command: &str) -> Result<DownloadControl, ClientError> {
        #[derive(serde::Deserialize)]
        struct Body {
            control: DownloadControl,
        }
        let url = self.endpoint(&format!("api/v1/downloads/{command}"))?;
        let body: Body = self.request("POST", &url, None).await?;
        Ok(body.control)
    }

    /// `GET /api/v1/downloads/stats`.
    pub async fn download_stats(&self) -> Result<DownloadStats, ClientError> {
        let url = self.endpoint("api/v1/downloads/stats")?;
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/downloads/reconcile?deep=`.
    pub async fn reconcile(&self, deep: bool) -> Result<ReconcileReport, ClientError> {
        let mut url = self.endpoint("api/v1/downloads/reconcile")?;
        if deep {
            url.query_pairs_mut().append_pair("deep", "true");
        }
        self.request("POST", &url, None).await
    }
    // Archive endpoints.

    /// `GET /api/v1/archive?state=&podcast=&limit=`.
    pub async fn archive_list(
        &self,
        state: Option<&str>,
        podcast: Option<&str>,
        source_changed: bool,
        limit: u32,
    ) -> Result<Vec<ArchiveFile>, ClientError> {
        #[derive(serde::Deserialize)]
        struct Body {
            files: Vec<ArchiveFile>,
        }
        let mut url = self.endpoint("api/v1/archive")?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(s) = state {
                q.append_pair("state", s);
            }
            if let Some(p) = podcast {
                q.append_pair("podcast", p);
            }
            if source_changed {
                q.append_pair("source_changed", "true");
            }
            q.append_pair("limit", &limit.to_string());
        }
        let body: Body = self.request("GET", &url, None).await?;
        Ok(body.files)
    }

    /// `GET /api/v1/archive/{episode_id}`.
    pub async fn archive_file(&self, episode_id: &str) -> Result<ArchiveFile, ClientError> {
        let url = self.endpoint(&format!("api/v1/archive/{episode_id}"))?;
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/archive/{episode_id}/verify`.
    pub async fn archive_verify_one(
        &self,
        episode_id: &str,
        depth: VerifyDepth,
    ) -> Result<VerifiedFile, ClientError> {
        let url = self.endpoint(&format!("api/v1/archive/{episode_id}/verify"))?;
        self.request("POST", &url, Some(serde_json::json!({ "depth": depth })))
            .await
    }

    /// `POST /api/v1/archive/verify`.
    pub async fn archive_verify_all(
        &self,
        depth: VerifyDepth,
        podcast: Option<&str>,
        state: Option<&str>,
    ) -> Result<VerifySummary, ClientError> {
        let url = self.endpoint("api/v1/archive/verify")?;
        self.request(
            "POST",
            &url,
            Some(serde_json::json!({
                "depth": depth,
                "podcast": podcast,
                "state": state,
            })),
        )
        .await
    }

    /// `POST /api/v1/archive/{episode_id}/path-preview`.
    pub async fn archive_path_preview(&self, episode_id: &str) -> Result<PathPreview, ClientError> {
        let url = self.endpoint(&format!("api/v1/archive/{episode_id}/path-preview"))?;
        self.request("POST", &url, None).await
    }

    /// `POST /api/v1/archive/{episode_id}/relocate`.
    pub async fn archive_relocate(
        &self,
        episode_id: &str,
        dry_run: bool,
    ) -> Result<Relocation, ClientError> {
        let url = self.endpoint(&format!("api/v1/archive/{episode_id}/relocate"))?;
        self.request(
            "POST",
            &url,
            Some(serde_json::json!({ "dry_run": dry_run })),
        )
        .await
    }

    /// `POST /api/v1/archive/reconcile?deep=`.
    pub async fn archive_reconcile(
        &self,
        deep: bool,
    ) -> Result<ArchiveReconcileReport, ClientError> {
        let mut url = self.endpoint("api/v1/archive/reconcile")?;
        if deep {
            url.query_pairs_mut().append_pair("deep", "true");
        }
        self.request("POST", &url, None).await
    }

    /// The archive-asset endpoints, as the JSON the server sent.
    ///
    /// These return the body verbatim rather than a typed struct. The
    /// shapes are reports - counts and findings - that the CLI renders
    /// and does not compute with, and a second set of mirror structs here
    /// would be one more place for the wire format to drift out of step.
    pub async fn archive_get_json(&self, path: &str) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(path)?;
        self.request("GET", &url, None).await
    }

    /// `GET` an authentication endpoint, returning the body verbatim.
    pub async fn auth_get_json(&self, path: &str) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(path)?;
        self.request("GET", &url, None).await
    }

    /// `POST` an authentication endpoint, returning the body verbatim.
    pub async fn auth_post_json(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(path)?;
        self.request("POST", &url, body).await
    }

    /// `POST` an authentication endpoint that answers `204`.
    pub async fn auth_post(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(), ClientError> {
        let url = self.endpoint(path)?;
        self.no_content(&url, body).await
    }

    /// `POST /api/v1/auth/tokens/{id}/revoke` (the `DELETE` route's alias).
    pub async fn auth_revoke_token(&self, id: &str) -> Result<(), ClientError> {
        let url = self.endpoint(&format!("api/v1/auth/tokens/{id}/revoke"))?;
        self.no_content(&url, None).await
    }

    /// `POST` to an archive-asset endpoint, returning the body verbatim.
    pub async fn archive_post_json(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(path)?;
        self.request("POST", &url, body).await
    }

    /// `GET /api/v1/archive/policies`.
    pub async fn archive_policies(&self) -> Result<Vec<ArchivePolicy>, ClientError> {
        #[derive(serde::Deserialize)]
        struct Body {
            policies: Vec<ArchivePolicy>,
        }
        let url = self.endpoint("api/v1/archive/policies")?;
        let body: Body = self.request("GET", &url, None).await?;
        Ok(body.policies)
    }

    /// `GET /api/v1/podcasts/{id}/policy`.
    pub async fn policy_show(&self, podcast_id: &str) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{podcast_id}/policy"))?;
        self.request("GET", &url, None).await
    }

    /// `POST /api/v1/podcasts/{id}/policy` (the `PUT` route's alias).
    pub async fn policy_set(
        &self,
        podcast_id: &str,
        update: &serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{podcast_id}/policy"))?;
        self.request("POST", &url, Some(update.clone())).await
    }

    /// `POST /api/v1/podcasts/{id}/policy/clear` (the `DELETE` route's).
    pub async fn policy_clear(&self, podcast_id: &str) -> Result<serde_json::Value, ClientError> {
        let url = self.endpoint(&format!("api/v1/podcasts/{podcast_id}/policy/clear"))?;
        self.request("POST", &url, None).await
    }
}
