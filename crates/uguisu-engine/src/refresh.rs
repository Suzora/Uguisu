//! The refresh pipeline (`docs/FEED_ENGINE.md`): conditional fetch → parse
//! → normalize → identity → change detection → one transaction →
//! post-commit events → [`RefreshReport`].
//!
//! Failure safety (brief §40): when anything before the final transaction
//! fails, only the source's fetch state, the fetch log and a
//! `podcast.feed.refresh.failed` event are written; podcast and episode
//! rows stay exactly as they were.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uguisu_core::feed::{
    EpisodeCounts, FeedFetch, FeedUrlStatus, FetchErrorKind, HttpSummary, NotModifiedReason,
    RefreshOutcome, RefreshReport,
};
use uguisu_core::ids::{EpisodeId, FetchId, PodcastId};
use uguisu_core::model::{
    Episode, EpisodeChange, FetchState, FetchStatus, Podcast, PodcastSource, PodcastStatus,
    ReplacementReason,
};
use uguisu_core::redact;
use uguisu_core::{Event, EventKind, UguisuError};
use uguisu_feed::normalize::normalize_channel;
use uguisu_feed::{ParseError, ParsedFeed};
use uguisu_http::{CancellationToken, Conditional, GetOptions, HttpError, Response, StatusCode};
use uguisu_storage::{Tx, archive_files, changes, episodes, events, fetches, podcasts, sources};

use uguisu_core::schedule;

use crate::Engine;
use crate::migration::{
    Announcement, Candidate, Unverified, announced_source, announcement, replacement_source, verify,
};
use crate::sync::{SyncPlan, SyncRules, diff_episode, plan};

/// Refresh interval used when a podcast has none of its own and the
/// configuration was not consulted (tests, and the default of
/// `UGUISU_FEED_REFRESH_INTERVAL_SECS`).
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);
/// Longest back-off after repeated failures.
pub const MAX_BACKOFF: Duration = Duration::from_secs(24 * 3600);

/// How a refresh should run.
#[derive(Debug, Clone, Default)]
pub struct RefreshOptions {
    /// Skip validators and the body fingerprint: always parse and sync.
    pub force: bool,
    /// Cooperative cancellation.
    pub cancel: Option<CancellationToken>,
}

/// A feed-URL announcement seen during the fetch, with the verification
/// fetch already done for `itunes:new-feed-url` (redirect targets are the
/// body just fetched and are checked against the stored podcast later).
struct Pending {
    announcement: Announcement,
    candidate: Option<Result<Candidate, Unverified>>,
}

/// What the fetch step produced before persistence.
enum Fetched {
    NotModified {
        reason: NotModifiedReason,
        http: HttpSummary,
        bytes: Option<u64>,
    },
    Body {
        response: Box<Response>,
        fingerprint: String,
        http: HttpSummary,
        parsed: Box<ParsedFeed>,
        pending: Option<Box<Pending>>,
    },
    Failed {
        kind: FetchErrorKind,
        detail: String,
        http: HttpSummary,
    },
}

/// Maps a transport error to the stable error kinds of brief §10.
#[must_use]
pub fn error_kind(err: &HttpError) -> FetchErrorKind {
    fn tls(detail: &str) -> bool {
        let d = detail.to_ascii_lowercase();
        d.contains("tls") || d.contains("certificate") || d.contains("ssl")
    }
    match err {
        HttpError::Policy(_) => FetchErrorKind::BlockedByPolicy,
        HttpError::Dns { .. } => FetchErrorKind::DnsError,
        HttpError::Connect(d) | HttpError::Transport(d) if tls(d) => FetchErrorKind::TlsError,
        HttpError::Timeout(_) => FetchErrorKind::Timeout,
        HttpError::BodyTooLarge { .. } => FetchErrorKind::TooLarge,
        HttpError::Status { status, .. } => {
            FetchErrorKind::from_status(*status).unwrap_or(FetchErrorKind::HttpClientError)
        }
        HttpError::Cancelled => FetchErrorKind::Cancelled,
        HttpError::InvalidUrl(_)
        | HttpError::Connect(_)
        | HttpError::TooManyRedirects { .. }
        | HttpError::BadRedirect(_)
        | HttpError::Body(_)
        | HttpError::MalformedHeader { .. }
        | HttpError::Build(_)
        | HttpError::Transport(_) => FetchErrorKind::NetworkError,
    }
}

/// Maps a parser error to an error kind.
#[must_use]
pub fn parse_error_kind(err: &ParseError) -> FetchErrorKind {
    match err {
        ParseError::NotXml { .. } => FetchErrorKind::InvalidContentType,
        ParseError::Malformed(_) => FetchErrorKind::MalformedXml,
        ParseError::UnsupportedEncoding(_) | ParseError::UnknownRoot(_) => {
            FetchErrorKind::UnsupportedFeed
        }
        ParseError::TooDeep(_) => FetchErrorKind::TooDeep,
        ParseError::TooLarge { .. } => FetchErrorKind::TooLarge,
        ParseError::Empty => FetchErrorKind::InvalidPodcastFeed,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn summary(resp: &Response, stored: &FetchStatus, conditional: bool) -> HttpSummary {
    let etag = resp.etag().map(str::to_owned);
    HttpSummary {
        status: Some(resp.status.as_u16()),
        final_url: Some(resp.url.clone()),
        redirects: u32::try_from(resp.redirects.len()).unwrap_or(u32::MAX),
        etag_changed: etag.is_some() && etag != stored.etag,
        etag,
        last_modified: resp.last_modified().map(str::to_owned),
        bytes: Some(resp.body.len() as u64),
        conditional,
    }
}

fn detail_from_body(resp: &Response) -> String {
    let text = String::from_utf8_lossy(&resp.body[..resp.body.len().min(160)]);
    let text = text.trim();
    if text.is_empty() {
        format!("http status {}", resp.status.as_u16())
    } else {
        format!(
            "http status {}: {}",
            resp.status.as_u16(),
            text.replace(['\n', '\r'], " ")
        )
    }
}

/// Whether the body starts like XML (after a BOM and whitespace).
pub(crate) fn looks_like_xml(body: &[u8]) -> bool {
    let mut b = body;
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        b = &b[3..];
    }
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        return true; // UTF-16 BOM; the decoder decides
    }
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    b[start..].starts_with(b"<")
}

impl Engine {
    /// One refresh run (callers go through [`Engine::refresh_podcast`],
    /// which coalesces concurrent runs). A failed fetch is a report with a
    /// `failed` outcome, not an error; errors are reserved for unknown
    /// ids and storage.
    #[allow(clippy::too_many_lines)] // the pipeline's orchestration, top to bottom
    pub(crate) async fn run_refresh(
        &self,
        id: PodcastId,
        opts: RefreshOptions,
    ) -> Result<RefreshReport, UguisuError> {
        let started = Instant::now();
        let (podcast, source) = {
            let mut conn = self.storage().reader().await?;
            let podcast =
                podcasts::get(&mut conn, id)
                    .await?
                    .ok_or_else(|| UguisuError::NotFound {
                        entity: "podcast".to_owned(),
                        id: id.to_string(),
                    })?;
            if podcast.status == PodcastStatus::Archived {
                return Err(UguisuError::Conflict(format!(
                    "podcast {id} is archived; resume it to refresh it"
                )));
            }
            let source = sources::current(&mut conn, id).await?.ok_or_else(|| {
                UguisuError::Storage(format!("podcast {id} has no current source"))
            })?;
            (podcast, source)
        };
        let fetch_id = FetchId::new();
        let conditional =
            !opts.force && (source.fetch.etag.is_some() || source.fetch.last_modified.is_some());

        // Mark the attempt so `feed status` shows it while the fetch runs.
        {
            let now = OffsetDateTime::now_utc();
            let mut tx = self.storage().begin().await?;
            sources::mark_fetching(&mut tx, source.id, now).await?;
            let started_event = vec![Event::now(
                Some(id),
                None,
                EventKind::FeedRefreshStarted {
                    source_id: source.id,
                    feed_url: source.feed_url.clone(),
                    conditional,
                },
            )];
            events::insert_all(&mut tx, &started_event).await?;
            commit(tx).await?;
            self.bus().publish(&started_event);
        }
        tracing::info!(podcast = %id, source = %source.id, url = %redact::urls(source.feed_url.as_str()), conditional, force = opts.force, "refresh started");

        let cancel = opts.cancel.clone().unwrap_or_default();
        let budget = self.config().feed.refresh_timeout;
        let fetched = tokio::time::timeout(
            budget,
            self.fetch_and_check(&source, conditional, opts.force, cancel.clone()),
        )
        .await
        .unwrap_or_else(|_| {
            cancel.cancel();
            Fetched::Failed {
                kind: FetchErrorKind::Timeout,
                detail: format!("refresh exceeded {budget:?}"),
                http: HttpSummary {
                    conditional,
                    ..HttpSummary::default()
                },
            }
        });

        let report = match fetched {
            Fetched::Failed { kind, detail, http } => {
                self.persist_failure(&podcast, &source, fetch_id, kind, detail, http, started)
                    .await?
            }
            Fetched::Body {
                response,
                fingerprint,
                http,
                parsed,
                pending,
            } => {
                self.persist_fetched(
                    &podcast,
                    &source,
                    fetch_id,
                    &response,
                    fingerprint,
                    http,
                    *parsed,
                    pending,
                    started,
                )
                .await?
            }
            Fetched::NotModified {
                reason,
                http,
                bytes,
            } => {
                self.persist_not_modified(&podcast, &source, fetch_id, reason, http, bytes, started)
                    .await?
            }
        };
        tracing::info!(
            podcast = %id,
            outcome = report.outcome.as_str(),
            added = report.episodes.added,
            updated = report.episodes.updated,
            unchanged = report.episodes.unchanged,
            removed = report.episodes.removed_detected,
            duration_ms = report.duration_ms,
            "refresh finished"
        );
        Ok(report)
    }

    /// Fetches, sniffs, fingerprints and parses the feed, and pre-fetches
    /// an announced new feed URL for verification.
    #[allow(clippy::too_many_lines)] // one linear sequence of checks
    async fn fetch_and_check(
        &self,
        source: &PodcastSource,
        conditional: bool,
        force: bool,
        cancel: CancellationToken,
    ) -> Fetched {
        let stored = &source.fetch;
        let get = GetOptions {
            conditional: conditional.then(|| Conditional {
                etag: stored.etag.clone(),
                last_modified: stored.last_modified.clone(),
            }),
            max_bytes: Some(self.config().feed.limits.max_bytes),
            cancel: Some(cancel.clone()),
            ..GetOptions::default()
        };
        let response = match self.feed_client().get_with(&source.feed_url, &get).await {
            Ok(r) => r,
            Err(e) => {
                let kind = error_kind(&e);
                let http = HttpSummary {
                    status: e.status(),
                    conditional,
                    ..HttpSummary::default()
                };
                return Fetched::Failed {
                    kind,
                    detail: e.to_string(),
                    http,
                };
            }
        };
        let http = summary(&response, stored, conditional);
        if response.status == StatusCode::NOT_MODIFIED {
            return Fetched::NotModified {
                reason: NotModifiedReason::Http304,
                http,
                bytes: None,
            };
        }
        if !response.status.is_success() {
            let kind = FetchErrorKind::from_status(response.status.as_u16())
                .unwrap_or(FetchErrorKind::HttpClientError);
            return Fetched::Failed {
                kind,
                detail: detail_from_body(&response),
                http,
            };
        }
        if response.body.is_empty() {
            return Fetched::Failed {
                kind: FetchErrorKind::InvalidPodcastFeed,
                detail: "empty body".to_owned(),
                http,
            };
        }
        if !looks_like_xml(&response.body) {
            let ct = response.content_type().unwrap_or("unknown").to_owned();
            return Fetched::Failed {
                kind: FetchErrorKind::InvalidContentType,
                detail: format!("body is not XML (content-type {ct})"),
                http,
            };
        }
        let fingerprint = sha256_hex(&response.body);
        if !force && stored.content_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            let bytes = http.bytes;
            return Fetched::NotModified {
                reason: NotModifiedReason::Fingerprint,
                http,
                bytes,
            };
        }
        let parsed = match uguisu_feed::parse(&response.body, &self.config().feed.limits) {
            Ok(p) => p,
            Err(e) => {
                return Fetched::Failed {
                    kind: parse_error_kind(&e),
                    detail: e.to_string(),
                    http,
                };
            }
        };
        if parsed.items.is_empty() && parsed.malformed_items.is_empty() {
            return Fetched::Failed {
                kind: FetchErrorKind::InvalidPodcastFeed,
                detail: "feed has no items".to_owned(),
                http,
            };
        }
        if !parsed.looks_like_podcast() {
            return Fetched::Failed {
                kind: FetchErrorKind::InvalidPodcastFeed,
                detail: format!(
                    "none of the {} items carries an enclosure",
                    parsed.items.len()
                ),
                http,
            };
        }
        let new_feed_url = normalize_channel(&parsed.channel).new_feed_url;
        let pending = match announcement(source, &response, new_feed_url.as_ref()) {
            None => None,
            Some(a) => {
                let candidate = match a.via {
                    ReplacementReason::NewFeedUrl => {
                        tracing::info!(podcast = %source.podcast_id, announced = %redact::urls(a.url.as_str()), "verifying announced feed url");
                        Some(self.fetch_candidate(&a.url, cancel.clone()).await)
                    }
                    _ => None,
                };
                Some(Box::new(Pending {
                    announcement: a,
                    candidate,
                }))
            }
        };
        Fetched::Body {
            response: Box::new(response),
            fingerprint,
            http,
            parsed: Box::new(parsed),
            pending,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_failure(
        &self,
        podcast: &Podcast,
        source: &PodcastSource,
        fetch_id: FetchId,
        kind: FetchErrorKind,
        detail: String,
        http: HttpSummary,
        started: Instant,
    ) -> Result<RefreshReport, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let failures = source.fetch.consecutive_failures.saturating_add(1);
        let fetch = FetchStatus {
            state: FetchState::Failed,
            last_attempt_at: Some(now),
            last_error_at: Some(now),
            consecutive_failures: failures,
            last_http_status: http.status,
            last_error_kind: Some(kind),
            last_error_detail: Some(cap(&detail, 1024)),
            ..source.fetch.clone()
        };
        let mut p = podcast.clone();
        p.last_error = Some(format!("{}: {}", kind.as_str(), cap(&detail, 512)));
        p.last_refresh_at = Some(now);
        p.next_refresh_at = Some(schedule::plan_next(
            podcast.id,
            podcast.next_refresh_at,
            now,
            backoff(podcast, failures, self.config().feed.refresh_interval),
        ));
        if p.status == PodcastStatus::Active && failures >= self.config().feed.error_after_failures
        {
            p.status = PodcastStatus::Error;
        }
        p.updated_at = now;
        let duration_ms = elapsed_ms(started);
        let log = FeedFetch {
            id: fetch_id,
            podcast_id: podcast.id,
            source_id: source.id,
            fetched_at: now,
            outcome: RefreshOutcome::Failed {
                kind,
                detail: detail.clone(),
            },
            http: http.clone(),
            duration_ms,
            partial: false,
            truncated: false,
            fingerprint_changed: false,
            episodes: EpisodeCounts::default(),
            podcast_changed: false,
            url_change_detected: false,
            warnings: Vec::new(),
        };
        let evs = vec![Event::now(
            Some(podcast.id),
            None,
            EventKind::FeedRefreshFailed {
                source_id: source.id,
                fetch_id,
                error_kind: kind,
                detail: cap(&detail, 512),
                http_status: http.status,
                consecutive_failures: failures,
            },
        )];
        let mut tx = self.storage().begin().await?;
        sources::update_fetch(&mut tx, source.id, &fetch, None, None, now).await?;
        podcasts::update(&mut tx, &p).await?;
        fetches::insert(&mut tx, &log).await?;
        fetches::trim(&mut tx, podcast.id, self.config().feed.retain_fetches).await?;
        events::insert_all(&mut tx, &evs).await?;
        commit(tx).await?;
        self.bus().publish(&evs);
        tracing::warn!(podcast = %podcast.id, kind = kind.as_str(), detail = %redact::urls(&detail), failures, "refresh failed");
        Ok(RefreshReport {
            schema: RefreshReport::SCHEMA,
            podcast_id: podcast.id,
            source_id: source.id,
            fetch_id,
            outcome: RefreshOutcome::Failed { kind, detail },
            http,
            podcast_changed_fields: Vec::new(),
            episodes: EpisodeCounts::default(),
            feed_url: FeedUrlStatus::Unchanged,
            warnings: Vec::new(),
            truncated: false,
            removal_suppressed: None,
            duration_ms,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_not_modified(
        &self,
        podcast: &Podcast,
        source: &PodcastSource,
        fetch_id: FetchId,
        reason: NotModifiedReason,
        http: HttpSummary,
        bytes: Option<u64>,
        started: Instant,
    ) -> Result<RefreshReport, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let fetch = FetchStatus {
            state: FetchState::NotModified,
            last_attempt_at: Some(now),
            last_not_modified_at: Some(now),
            consecutive_failures: 0,
            last_http_status: http.status,
            last_error_kind: None,
            last_error_detail: None,
            // A 304 may carry refreshed validators; keep the old ones otherwise.
            etag: http.etag.clone().or_else(|| source.fetch.etag.clone()),
            last_modified: http
                .last_modified
                .clone()
                .or_else(|| source.fetch.last_modified.clone()),
            last_content_length: bytes.or(source.fetch.last_content_length),
            ..source.fetch.clone()
        };
        let mut p = podcast.clone();
        p.last_refresh_at = Some(now);
        p.next_refresh_at = Some(schedule::plan_next(
            podcast.id,
            podcast.next_refresh_at,
            now,
            interval(podcast, None, self.config().feed.refresh_interval),
        ));
        p.last_error = None;
        if p.status == PodcastStatus::Error {
            p.status = PodcastStatus::Active;
        }
        p.updated_at = now;
        let duration_ms = elapsed_ms(started);
        let log = FeedFetch {
            id: fetch_id,
            podcast_id: podcast.id,
            source_id: source.id,
            fetched_at: now,
            outcome: RefreshOutcome::NotModified { reason },
            http: http.clone(),
            duration_ms,
            partial: false,
            truncated: false,
            fingerprint_changed: false,
            episodes: EpisodeCounts::default(),
            podcast_changed: false,
            url_change_detected: false,
            warnings: Vec::new(),
        };
        let evs = vec![Event::now(
            Some(podcast.id),
            None,
            EventKind::FeedNotModified {
                source_id: source.id,
                fetch_id,
                reason,
            },
        )];
        let mut tx = self.storage().begin().await?;
        sources::update_fetch(&mut tx, source.id, &fetch, Some(now), None, now).await?;
        podcasts::update(&mut tx, &p).await?;
        fetches::insert(&mut tx, &log).await?;
        fetches::trim(&mut tx, podcast.id, self.config().feed.retain_fetches).await?;
        events::insert_all(&mut tx, &evs).await?;
        commit(tx).await?;
        self.bus().publish(&evs);
        Ok(RefreshReport {
            schema: RefreshReport::SCHEMA,
            podcast_id: podcast.id,
            source_id: source.id,
            fetch_id,
            outcome: RefreshOutcome::NotModified { reason },
            http,
            podcast_changed_fields: Vec::new(),
            episodes: EpisodeCounts::default(),
            feed_url: FeedUrlStatus::Unchanged,
            warnings: Vec::new(),
            truncated: false,
            removal_suppressed: None,
            duration_ms,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn persist_fetched(
        &self,
        podcast: &Podcast,
        source: &PodcastSource,
        fetch_id: FetchId,
        response: &Response,
        fingerprint: String,
        http: HttpSummary,
        parsed: ParsedFeed,
        pending: Option<Box<Pending>>,
        started: Instant,
    ) -> Result<RefreshReport, UguisuError> {
        let now = OffsetDateTime::now_utc();
        let rules = SyncRules {
            removal_streak: self.config().feed.removal_streak,
            mass_removal_guard_percent: self.config().feed.mass_removal_guard_percent,
        };
        let mut tx = self.storage().begin().await?;
        let index = episodes::index(&mut tx, podcast.id).await?;
        let sync: SyncPlan = plan(podcast, &parsed, &index, rules, now);
        let stored_keys: Vec<String> = index
            .iter()
            .filter(|e| e.removed_from_feed_at.is_none())
            .map(|e| e.identity_key.clone())
            .collect();

        // A redirected fetch must still be this podcast before anything
        // of it is written (a redirect to another show is a failure, not
        // a merge).
        let mut redirect_candidate = None;
        if !response.redirects.is_empty() {
            let cand = Candidate {
                url: response.url.clone(),
                guid: sync.channel_guid.clone(),
                title: sync.channel_title.clone(),
                keys: sync.identity_keys.clone(),
                fetch: FetchStatus::default(),
                http: http.clone(),
            };
            if let Err(reason) = verify(podcast, &stored_keys, &cand) {
                tx.rollback()
                    .await
                    .map_err(uguisu_storage::StorageError::from)?;
                return self
                    .persist_failure(
                        podcast,
                        source,
                        fetch_id,
                        FetchErrorKind::InvalidPodcastFeed,
                        format!(
                            "{} redirects to a different podcast: {reason}",
                            source.feed_url
                        ),
                        http,
                        started,
                    )
                    .await;
            }
            redirect_candidate = Some(cand);
        }

        // Podcast row.
        let mut p = sync.podcast.clone();
        p.last_refresh_at = Some(now);
        p.next_refresh_at = Some(schedule::plan_next(
            podcast.id,
            podcast.next_refresh_at,
            now,
            interval(
                podcast,
                response.cache_max_age(),
                self.config().feed.refresh_interval,
            ),
        ));
        p.last_error = None;
        if p.status == PodcastStatus::Error {
            p.status = PodcastStatus::Active;
        }
        p.updated_at = now;
        podcasts::update(&mut tx, &p).await?;

        // Change log for updated episodes (old rows read before the upsert).
        let mut change_rows: Vec<EpisodeChange> = Vec::new();
        let mut evs: Vec<Event> = Vec::new();
        if !sync.updated.is_empty() {
            let old = episodes::get_many(&mut tx, &sync.updated).await?;
            for new in sync.upserts.iter().filter(|e| sync.updated.contains(&e.id)) {
                let Some(before) = old.iter().find(|o| o.id == new.id) else {
                    continue;
                };
                let diffs = diff_episode(before, new);
                let mut fields: Vec<String> = diffs.iter().map(|(f, _, _)| f.clone()).collect();
                for (field, old_value, new_value) in diffs {
                    change_rows.push(EpisodeChange {
                        id: uguisu_core::ids::ChangeId::new(),
                        episode_id: new.id,
                        podcast_id: podcast.id,
                        fetch_id: Some(fetch_id),
                        changed_at: now,
                        field,
                        old_value,
                        new_value,
                    });
                }
                if let Some(note) = sync.identity_notes.iter().find(|n| n.episode_id == new.id) {
                    change_rows.push(EpisodeChange {
                        id: uguisu_core::ids::ChangeId::new(),
                        episode_id: new.id,
                        podcast_id: podcast.id,
                        fetch_id: Some(fetch_id),
                        changed_at: now,
                        field: "identity_signals".to_owned(),
                        old_value: Some(before.identity.key.clone()),
                        new_value: Some(format!("{} ({})", note.incoming_key, note.reason)),
                    });
                    fields.push("identity_signals".to_owned());
                }
                if fields.iter().any(|f| f == "enclosures")
                    && let Some(event) =
                        source_changed(&mut tx, podcast.id, before, new, now).await?
                {
                    evs.push(event);
                }
                if !fields.is_empty() {
                    evs.push(Event::now(
                        Some(podcast.id),
                        Some(new.id),
                        EventKind::EpisodeUpdated { fields },
                    ));
                }
            }
        }

        episodes::upsert_all(&mut tx, &sync.upserts).await?;
        if !sync.unchanged.is_empty() {
            episodes::mark_seen(&mut tx, &sync.unchanged, now).await?;
        }
        if !sync.missing.is_empty() {
            episodes::mark_missing(&mut tx, &sync.missing, now).await?;
        }
        if !sync.removed.is_empty() {
            let ids: Vec<EpisodeId> = sync.removed.iter().map(|(id, _)| *id).collect();
            episodes::mark_removed(&mut tx, &ids, now).await?;
        }
        changes::insert_all(&mut tx, &change_rows).await?;

        // Source state and validators.
        let fetch = FetchStatus {
            state: FetchState::Fetched,
            last_attempt_at: Some(now),
            last_success_at: Some(now),
            last_not_modified_at: source.fetch.last_not_modified_at,
            last_error_at: source.fetch.last_error_at,
            consecutive_failures: 0,
            last_http_status: http.status,
            last_error_kind: None,
            last_error_detail: None,
            etag: http.etag.clone(),
            last_modified: http.last_modified.clone(),
            content_fingerprint: Some(fingerprint),
            last_content_length: http.bytes,
        };
        sources::update_fetch(&mut tx, source.id, &fetch, Some(now), None, now).await?;

        // Events for new and ambiguous episodes, metadata, removals.
        for e in &sync.upserts {
            if sync.added.contains(&e.id) && !e.malformed {
                evs.push(Event::now(
                    Some(podcast.id),
                    Some(e.id),
                    EventKind::EpisodeDiscovered {
                        title: e.title.clone(),
                        identity_key: e.identity.key.clone(),
                        enclosure_url: e.primary_enclosure().map(|x| x.url.clone()),
                        published_at: e.published_at,
                    },
                ));
            }
        }
        for a in &sync.ambiguous {
            evs.push(Event::now(
                Some(podcast.id),
                Some(a.episode_id),
                EventKind::EpisodeIdentityAmbiguous {
                    duplicate_of: a.duplicate_of,
                    reasons: a.reasons.clone(),
                },
            ));
        }
        for (id, streak) in &sync.removed {
            evs.push(Event::now(
                Some(podcast.id),
                Some(*id),
                EventKind::EpisodeRemovalDetected {
                    missing_streak: *streak,
                },
            ));
        }
        if !sync.podcast_changed_fields.is_empty() {
            evs.push(Event::now(
                Some(podcast.id),
                None,
                EventKind::PodcastMetadataUpdated {
                    fields: sync.podcast_changed_fields.clone(),
                },
            ));
        }
        let mut warnings = sync.warnings.clone();
        let unchecked = |detail: &str| Unverified {
            kind: FetchErrorKind::InvalidPodcastFeed,
            detail: detail.to_owned(),
            http_status: None,
        };
        let feed_url = match pending {
            None => {
                sources::clear_announced(&mut tx, podcast.id).await?;
                FeedUrlStatus::Unchanged
            }
            Some(p) => {
                let via = p.announcement.via;
                let announced = p.announcement.url.clone();
                let candidate = match via {
                    ReplacementReason::Redirect => redirect_candidate.take().map_or_else(
                        || Err(unchecked("redirect target not checked")),
                        |mut c| {
                            c.fetch = fetch.clone();
                            Ok(c)
                        },
                    ),
                    _ => p
                        .candidate
                        .unwrap_or_else(|| Err(unchecked("announced feed was not fetched"))),
                };
                let outcome = match via {
                    // Verified above, before any write.
                    ReplacementReason::Redirect => {
                        candidate.map(|c| (c, "same podcast".to_owned()))
                    }
                    _ => candidate.and_then(|c| match verify(podcast, &stored_keys, &c) {
                        Ok(why) => Ok((c, why)),
                        Err(why) => Err(Unverified::not_this_podcast(why, &c)),
                    }),
                };
                match outcome {
                    Ok((cand, why)) => {
                        sources::clear_announced(&mut tx, podcast.id).await?;
                        let new = replacement_source(source, &cand, now);
                        sources::replace_current(&mut tx, source.id, &new, via, now).await?;
                        evs.push(Event::now(
                            Some(podcast.id),
                            None,
                            EventKind::FeedUrlChanged {
                                from_source_id: source.id,
                                to_source_id: new.id,
                                from: source.feed_url.clone(),
                                to: new.feed_url.clone(),
                                via,
                            },
                        ));
                        warnings.push(format!(
                            "feed url changed to {} ({}; {why})",
                            new.feed_url, p.announcement.reason
                        ));
                        tracing::info!(podcast = %podcast.id, from = %redact::urls(source.feed_url.as_str()), to = %redact::urls(new.feed_url.as_str()), via = via.as_str(), "feed url changed");
                        FeedUrlStatus::Changed {
                            from: source.feed_url.clone(),
                            to: new.feed_url.clone(),
                            via,
                        }
                    }
                    Err(why) => {
                        let previous = sources::announced(&mut tx, podcast.id).await?;
                        let row =
                            announced_source(source, previous.as_ref(), &announced, &why, now);
                        sources::clear_announced(&mut tx, podcast.id).await?;
                        sources::insert(&mut tx, &row).await?;
                        let reason = why.to_string();
                        evs.push(Event::now(
                            Some(podcast.id),
                            None,
                            EventKind::FeedUrlChangeDetected {
                                source_id: source.id,
                                announced: announced.clone(),
                                via,
                                reason: reason.clone(),
                            },
                        ));
                        warnings.push(format!(
                            "feed announces {announced} ({}) but it could not be verified: {reason}",
                            p.announcement.reason
                        ));
                        tracing::warn!(podcast = %podcast.id, announced = %redact::urls(announced.as_str()), reason = %redact::urls(&reason), "feed url change detected but not verified");
                        FeedUrlStatus::ChangeDetected { announced, reason }
                    }
                }
            }
        };
        let url_change_detected = !matches!(feed_url, FeedUrlStatus::Unchanged);
        let truncated = parsed.truncated;
        let partial = parsed.is_partial();
        if truncated {
            warnings.push(format!(
                "feed truncated after {} items; removal detection skipped",
                parsed.items.len()
            ));
        }
        let duration_ms = elapsed_ms(started);
        evs.push(Event::now(
            Some(podcast.id),
            None,
            EventKind::FeedRefreshCompleted {
                source_id: source.id,
                fetch_id,
                episodes: sync.counts,
                podcast_changed: !sync.podcast_changed_fields.is_empty(),
                truncated,
            },
        ));
        let log = FeedFetch {
            id: fetch_id,
            podcast_id: podcast.id,
            source_id: source.id,
            fetched_at: now,
            outcome: RefreshOutcome::Fetched,
            http: http.clone(),
            duration_ms,
            partial,
            truncated,
            fingerprint_changed: true,
            episodes: sync.counts,
            podcast_changed: !sync.podcast_changed_fields.is_empty(),
            url_change_detected,
            warnings: warnings.clone(),
        };
        fetches::insert(&mut tx, &log).await?;
        fetches::trim(&mut tx, podcast.id, self.config().feed.retain_fetches).await?;
        events::insert_all(&mut tx, &evs).await?;
        commit(tx).await?;
        self.bus().publish(&evs);

        // The archive policy runs after the refresh has committed, never
        // inside it: the feed is stored either way, and a queue that
        // refuses must not cost the user the fetch. It is off by default
        // (ADR 0023), so this is a no-op unless it was turned on.
        if let Err(e) = self.apply_policy(podcast.id, &sync.added).await {
            tracing::warn!(podcast = %podcast.id, error = %e, "archive policy could not run");
        }
        // Artwork follows the same rule, and only when asked (ADR 0047): a
        // cover URL this refresh changed, or a podcast with none stored yet.
        // An unchanged URL with an image stored costs no request at all.
        if self.config().archive.artwork_fetch
            && (sync
                .podcast_changed_fields
                .iter()
                .any(|f| f == "artwork_url")
                || matches!(self.current_artwork(podcast.id).await, Ok(None)))
            && let Err(e) = self.fetch_artwork(podcast.id, false).await
        {
            tracing::warn!(podcast = %podcast.id, error = %redact::urls(&e.to_string()), "podcast artwork could not be fetched");
        }

        Ok(RefreshReport {
            schema: RefreshReport::SCHEMA,
            podcast_id: podcast.id,
            source_id: source.id,
            fetch_id,
            outcome: RefreshOutcome::Fetched,
            http,
            podcast_changed_fields: sync.podcast_changed_fields,
            episodes: sync.counts,
            feed_url,
            warnings,
            truncated,
            removal_suppressed: sync.removal_suppressed,
            duration_ms,
        })
    }
}

/// Flags an archived episode whose primary enclosure now has another URL or
/// declared length, and says so; `None` when it changed in neither or the
/// episode has no archived file. Never re-downloads: the file is kept.
async fn source_changed(
    tx: &mut Tx,
    podcast_id: PodcastId,
    before: &Episode,
    after: &Episode,
    now: OffsetDateTime,
) -> Result<Option<Event>, UguisuError> {
    let (Some(old), Some(new)) = (before.primary_enclosure(), after.primary_enclosure()) else {
        return Ok(None);
    };
    if old.url == new.url && old.length_bytes == new.length_bytes {
        return Ok(None);
    }
    let Some(file) = archive_files::get_by_episode(tx, after.id).await? else {
        return Ok(None);
    };
    archive_files::mark_source_changed(tx, after.id, now).await?;
    Ok(Some(Event::now(
        Some(podcast_id),
        Some(after.id),
        EventKind::ArchiveSourceChanged {
            archive_file_id: file.id,
            path: file.relative_path,
            old_url: old.url.to_string(),
            new_url: new.url.to_string(),
            old_length: old.length_bytes,
            new_length: new.length_bytes,
        },
    )))
}

async fn commit(tx: Tx) -> Result<(), UguisuError> {
    tx.commit()
        .await
        .map_err(uguisu_storage::StorageError::from)?;
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// The podcast's refresh interval, never shorter than what the origin's
/// `Cache-Control` allows.
///
/// `configured` is `UGUISU_FEED_REFRESH_INTERVAL_SECS`; a podcast's own
/// interval still wins over it, because that is a decision about this
/// feed rather than about the library.
fn interval(podcast: &Podcast, max_age: Option<Duration>, configured: Duration) -> Duration {
    let own = podcast
        .refresh_interval_secs
        .map_or(configured, Duration::from_secs);
    match max_age {
        Some(age) if age > own => age.min(MAX_BACKOFF),
        _ => own,
    }
}

/// Exponential back-off after `failures` consecutive failures.
fn backoff(podcast: &Podcast, failures: u32, configured: Duration) -> Duration {
    let base = interval(podcast, None, configured);
    let factor = 2u32.saturating_pow(failures.saturating_sub(1).min(6));
    base.saturating_mul(factor).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_errors_map_to_stable_kinds() {
        assert_eq!(
            error_kind(&HttpError::Timeout(Duration::from_secs(1))),
            FetchErrorKind::Timeout
        );
        assert_eq!(
            error_kind(&HttpError::Dns {
                host: "x".into(),
                detail: "nx".into()
            }),
            FetchErrorKind::DnsError
        );
        assert_eq!(
            error_kind(&HttpError::Connect("tls handshake failed".into())),
            FetchErrorKind::TlsError
        );
        assert_eq!(
            error_kind(&HttpError::Connect("refused".into())),
            FetchErrorKind::NetworkError
        );
        assert_eq!(
            error_kind(&HttpError::BodyTooLarge { limit: 1 }),
            FetchErrorKind::TooLarge
        );
        assert_eq!(error_kind(&HttpError::Cancelled), FetchErrorKind::Cancelled);
        assert_eq!(
            parse_error_kind(&ParseError::Malformed("x".into())),
            FetchErrorKind::MalformedXml
        );
        assert_eq!(
            parse_error_kind(&ParseError::NotXml {
                looks_like_html: true
            }),
            FetchErrorKind::InvalidContentType
        );
    }

    #[test]
    fn xml_sniffing_tolerates_bom_and_whitespace() {
        assert!(looks_like_xml(b"\xEF\xBB\xBF\n  <?xml"));
        assert!(looks_like_xml(b"<rss>"));
        assert!(!looks_like_xml(b"{\"json\":1}"));
        assert!(!looks_like_xml(b""));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let mut p = Podcast {
            id: PodcastId::new(),
            title: String::new(),
            sort_title: String::new(),
            subtitle: None,
            author: None,
            publisher: None,
            owner_name: None,
            owner_email: None,
            description_html: None,
            description_text: None,
            website: None,
            artwork_url: None,
            language: None,
            categories: vec![],
            explicit: None,
            copyright: None,
            podcast_guid: None,
            feed_kind: uguisu_core::model::FeedKind::Rss2,
            status: PodcastStatus::Active,
            refresh_interval_secs: Some(600),
            next_refresh_at: None,
            last_refresh_at: None,
            last_error: None,
            directory_name: None,
            metadata_hash: String::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        let default = DEFAULT_REFRESH_INTERVAL;
        assert_eq!(backoff(&p, 1, default), Duration::from_secs(600));
        assert_eq!(backoff(&p, 3, default), Duration::from_secs(2400));
        assert_eq!(
            backoff(&p, 40, default),
            Duration::from_secs(600 * 64),
            "factor caps at 64"
        );
        assert_eq!(
            interval(&p, Some(Duration::from_secs(1800)), default),
            Duration::from_secs(1800)
        );
        p.refresh_interval_secs = None;
        assert_eq!(
            interval(&p, Some(Duration::ZERO), default),
            DEFAULT_REFRESH_INTERVAL
        );
        assert_eq!(
            interval(&p, None, Duration::from_secs(900)),
            Duration::from_secs(900),
            "the configured interval applies when the podcast has none"
        );
        assert_eq!(backoff(&p, 40, default), MAX_BACKOFF);
        assert_eq!(cap("héllo", 2), "h…");
    }
}
