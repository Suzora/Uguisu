//! Human- and machine-readable output plus exit codes.
#![allow(clippy::format_push_string)] // building report text line by line reads best with format!

use std::process::ExitCode;

use serde::Serialize;
use uguisu_discovery::{ResolveError, ResolveFailure, ResolvedFeed, SearchOutcome, SearchResponse};

/// Documented exit codes (`docs/CLI.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Success.
    Ok,
    /// Unexpected error.
    Error,
    /// Usage error, unknown id or confirmation required.
    Usage,
    /// Search completed without results.
    NoResults,
    /// Every provider failed or none is enabled.
    AllProvidersFailed,
    /// Results found but the feed could not be resolved.
    Unresolvable,
    /// Refused by the network policy.
    Blocked,
    /// The feed could not be parsed or is not a podcast feed.
    FeedError,
    /// The feed could not be fetched.
    NetworkError,
    /// The refresh succeeded with problems (truncated or malformed items).
    Partial,
    /// A download failed for a non-network reason (validation, length
    /// mismatch, I/O, target exists, attempts exhausted).
    DownloadFailed,
    /// The disk is full or the queue was paused by the engine.
    DiskFull,
    /// `serve` refused a network address with no credential to protect it.
    InsecureExposure,
}

impl Exit {
    /// Numeric code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Error => 1,
            Self::Usage => 2,
            Self::NoResults => 3,
            Self::AllProvidersFailed => 4,
            Self::Unresolvable => 5,
            Self::Blocked => 6,
            Self::FeedError => 7,
            Self::NetworkError => 8,
            Self::Partial => 9,
            Self::DownloadFailed => 10,
            Self::DiskFull => 11,
            Self::InsecureExposure => 12,
        }
    }

    /// Exit code for a search outcome.
    pub const fn for_outcome(outcome: SearchOutcome) -> Self {
        match outcome {
            SearchOutcome::Results => Self::Ok,
            SearchOutcome::NoResults => Self::NoResults,
            SearchOutcome::AllProvidersFailed | SearchOutcome::NoProvidersEnabled => {
                Self::AllProvidersFailed
            }
        }
    }

    /// Exit code for a resolution failure.
    pub const fn for_resolve_error(error: &ResolveError) -> Self {
        match error {
            ResolveError::BlockedByPolicy { .. } => Self::Blocked,
            ResolveError::NotAUrl { .. } => Self::Usage,
            _ => Self::Unresolvable,
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(e: Exit) -> Self {
        Self::from(e.code())
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    command: &'a str,
    message: &'a str,
    // Always null since every command is built; kept because `--json`
    // error bodies are a contract.
    phase: Option<u8>,
}

/// Prints an error either as JSON or as a plain line on stderr.
#[allow(clippy::print_stderr)]
pub fn error(json: bool, command: &str, message: &str) {
    if json {
        let body = ErrorBody {
            error: "failed",
            command,
            message,
            phase: None,
        };
        eprintln!("{}", serde_json::to_string(&body).unwrap_or_default());
    } else {
        eprintln!("uguisu {command}: {message}");
    }
}

/// Prints one indented follow-up line on stderr, beneath an error.
#[allow(clippy::print_stderr)]
pub fn hint(message: &str) {
    eprintln!("  {message}");
}

/// Prints any serializable value as JSON on stdout.
///
/// Compact, not pretty-printed: `--json` is the machine path, where the
/// indentation is 40-60 % more bytes and no reader. `jq` formats it for a
/// person who wants to look.
#[allow(clippy::print_stdout)]
pub fn json<T: Serialize>(value: &T) {
    println!("{}", serde_json::to_string(value).unwrap_or_default());
}

/// Renders a search response as text.
pub fn render_search(resp: &SearchResponse, explain: bool) -> String {
    let mut out = String::new();
    match resp.outcome {
        SearchOutcome::Results => {
            let found: Vec<&str> = resp
                .relaxed
                .iter()
                .filter(|q| q.providers.iter().any(|p| p.candidates > 0))
                .map(|q| q.query.as_str())
                .collect();
            if !found.is_empty() {
                out.push_str(&format!(
                    "No results for `{}`; showing results for `{}`.\n",
                    resp.query.raw,
                    found.join("`, `")
                ));
            }
        }
        SearchOutcome::NoResults => {
            out.push_str(&format!("No results for `{}`.\n", resp.query.raw));
        }
        SearchOutcome::AllProvidersFailed => out.push_str("All providers failed.\n"),
        SearchOutcome::NoProvidersEnabled => out.push_str("No discovery provider is enabled.\n"),
    }
    for r in &resp.results {
        let c = &r.candidate;
        out.push_str(&format!("{}. {}\n", r.rank, c.title));
        if let Some(a) = &c.author {
            out.push_str(&format!("   {a}\n"));
        }
        match &c.feed_url {
            Some(f) => out.push_str(&format!("   Feed: {f}\n")),
            None => out.push_str("   Feed: (not provided by the directory)\n"),
        }
        if let Some(w) = &c.website {
            out.push_str(&format!("   Website: {w}\n"));
        }
        let sources: Vec<String> = c.providers().iter().map(ToString::to_string).collect();
        out.push_str(&format!("   Source: {}\n", sources.join(", ")));
        if let Some(n) = c.episode_count {
            out.push_str(&format!("   Episodes: {n}\n"));
        }
        out.push_str(&format!("   Confidence: {:.2}\n", r.confidence));
        for a in &r.ambiguities {
            out.push_str(&format!(
                "   Possibly the same as `{}` ({:.2}) — {}\n",
                a.other_title, a.similarity, a.reason
            ));
        }
        if explain {
            out.push_str(&format!(
                "   Score {:.2} / {:.2}\n",
                r.explanation.total, r.explanation.max_total
            ));
            for s in &r.explanation.signals {
                out.push_str(&format!(
                    "     {:<15} {:>5.2} × {:>4.2} = {:>5.2}  {}\n",
                    s.name, s.value, s.weight, s.contribution, s.note
                ));
            }
        }
        out.push('\n');
    }
    for p in &resp.providers {
        let status = format!("{:?}", p.status).to_ascii_lowercase();
        let mut line = format!(
            "provider {:<13} {status:<9} {:>3} candidate(s) {:>5} ms",
            p.provider, p.candidates, p.latency_ms
        );
        if p.from_cache {
            line.push_str("  (cache)");
        }
        if let Some(e) = &p.error {
            line.push_str(&format!("  {e}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    if !resp.attribution.is_empty() {
        out.push_str(&format!("{}\n", resp.attribution.join(" · ")));
    }
    out
}

/// Renders a resolved feed as text.
pub fn render_resolved(feed: &ResolvedFeed) -> String {
    let mut out = String::new();
    out.push_str(&format!("Feed: {}\n", feed.feed_url));
    if let Some(c) = &feed.canonical_url {
        out.push_str(&format!("Canonical: {c}\n"));
    }
    if let Some(m) = &feed.moved_to {
        out.push_str(&format!("Moved to: {m}\n"));
    }
    if let Some(t) = &feed.title {
        out.push_str(&format!("Title: {t}\n"));
    }
    if let Some(a) = &feed.author {
        out.push_str(&format!("Author: {a}\n"));
    }
    if let Some(w) = &feed.website {
        out.push_str(&format!("Website: {w}\n"));
    }
    if let Some(g) = &feed.podcast_guid {
        out.push_str(&format!("GUID: {g}\n"));
    }
    out.push_str(&format!(
        "Items: {} ({} with media)\n",
        feed.item_count, feed.items_with_media
    ));
    if let Some(n) = feed.newest_item {
        out.push_str(&format!("Newest: {}\n", n.date()));
    }
    if feed.locked == Some(true) {
        out.push_str("Locked: yes (podcast:locked)\n");
    }
    for w in &feed.warnings {
        out.push_str(&format!("Warning: {w}\n"));
    }
    out.push_str("Steps:\n");
    for s in &feed.provenance {
        out.push_str(&format!(
            "  {} {:?} {} {}\n",
            if s.ok { "✓" } else { "✗" },
            s.kind,
            s.url.as_ref().map(ToString::to_string).unwrap_or_default(),
            s.detail
        ));
    }
    out
}

/// Renders a resolution failure as text.
pub fn render_failure(f: &ResolveFailure) -> String {
    let mut out = format!(
        "Could not resolve a feed: {}\nSuggestion: {}\n",
        f.error,
        f.error.suggestion()
    );
    if let ResolveError::NoFeedLinkFound { tried, .. } = &f.error {
        for t in tried {
            out.push_str(&format!("  tried {t}\n"));
        }
    }
    out.push_str("Steps:\n");
    for s in &f.provenance {
        out.push_str(&format!(
            "  {} {:?} {} {}\n",
            if s.ok { "✓" } else { "✗" },
            s.kind,
            s.url.as_ref().map(ToString::to_string).unwrap_or_default(),
            s.detail
        ));
    }
    out
}

/// JSON body for a resolution failure.
#[derive(Serialize)]
pub struct FailureBody<'a> {
    /// The error (tagged by `kind`).
    pub error: &'a ResolveError,
    /// Stable kind.
    pub kind: &'static str,
    /// Suggested next step.
    pub suggestion: &'static str,
    /// Steps taken.
    pub provenance: &'a [uguisu_discovery::ResolutionStep],
}

impl<'a> FailureBody<'a> {
    /// Builds the body.
    pub fn new(f: &'a ResolveFailure) -> Self {
        Self {
            error: &f.error,
            kind: f.error.kind(),
            suggestion: f.error.suggestion(),
            provenance: &f.provenance,
        }
    }
}

// Library and feed output.

use time::OffsetDateTime;
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchiveErrorKind, ArchiveFile, ArchivePolicy, VerificationState};
use uguisu_core::download::{
    DownloadAttempt, DownloadControl, DownloadErrorKind, DownloadJob, DownloadState,
};
use uguisu_core::feed::{FeedFetch, FeedUrlStatus, FetchErrorKind, RefreshOutcome, RefreshReport};
use uguisu_core::model::PodcastSource;
use uguisu_download::{DownloadStats, EnqueueOutcome, EnqueueSummary, JobDetail, ReconcileReport};
use uguisu_engine::archive::{
    ArchiveReconcileReport, PathPreview, Relocation, VerifiedFile, VerifySummary,
};
use uguisu_engine::library::{AddOutcome, EpisodeDetail, PodcastDetail};
use uguisu_engine::migration::{FeedMove, MoveOptions};
use uguisu_engine::opml::{OpmlAction, OpmlImport};
use uguisu_engine::{Inspection, RefreshAllEntry};

impl Exit {
    /// Exit code for a refresh report: `0` fetched or not modified, `9`
    /// partial (truncated or malformed items), `7` feed error, `8` network
    /// error, `6` blocked.
    pub const fn for_report(report: &RefreshReport) -> Self {
        match &report.outcome {
            RefreshOutcome::Fetched => {
                if report.is_partial() {
                    Self::Partial
                } else {
                    Self::Ok
                }
            }
            RefreshOutcome::NotModified { .. } => Self::Ok,
            RefreshOutcome::Failed { kind, .. } => Self::for_fetch_error(*kind),
        }
    }

    /// Exit code for a fetch error kind.
    pub const fn for_fetch_error(kind: FetchErrorKind) -> Self {
        match kind {
            FetchErrorKind::BlockedByPolicy => Self::Blocked,
            FetchErrorKind::Cancelled => Self::Error,
            k if k.is_content() => Self::FeedError,
            _ => Self::NetworkError,
        }
    }

    /// Exit code for an engine error.
    pub fn for_engine_error(e: &UguisuError) -> Self {
        match e {
            UguisuError::NotFound { .. } | UguisuError::Invalid(_) => Self::Usage,
            UguisuError::Unresolvable { .. } => Self::Unresolvable,
            UguisuError::BlockedByPolicy(_) => Self::Blocked,
            UguisuError::Network { kind, .. } | UguisuError::Feed { kind, .. } => {
                Self::for_fetch_error(*kind)
            }
            UguisuError::DiskFull { .. } => Self::DiskFull,
            // An archive problem is either bad input (usage) or a real
            // artifact/file-system failure.
            UguisuError::Archive { kind, .. } => match kind {
                ArchiveErrorKind::ArchiveNotFound
                | ArchiveErrorKind::PathInvalid
                | ArchiveErrorKind::TemplateInvalid
                | ArchiveErrorKind::PolicyInvalid => Self::Usage,
                _ => Self::Error,
            },
            UguisuError::Conflict(_)
            | UguisuError::Storage(_)
            | UguisuError::Config(_)
            | UguisuError::Locked(_)
            | UguisuError::Cancelled(_)
            | UguisuError::Io { .. }
            | UguisuError::Internal(_) => Self::Error,
        }
    }

    /// Exit code for a download job that stopped (`download <id> --wait`,
    /// `download run`): network causes map to 8, policy to 6, a full disk
    /// to 11 and everything else the job could not fix itself to 10.
    pub fn for_download_job(job: &DownloadJob) -> Self {
        match job.state {
            DownloadState::Completed => Self::Ok,
            DownloadState::Failed => match job.last_error_kind {
                Some(DownloadErrorKind::PolicyBlocked) => Self::Blocked,
                Some(DownloadErrorKind::DiskFull) => Self::DiskFull,
                Some(k) if k.is_network() => Self::NetworkError,
                _ => Self::DownloadFailed,
            },
            DownloadState::Paused if job.state_reason.as_deref() == Some("disk_full") => {
                Self::DiskFull
            }
            DownloadState::Cancelled
            | DownloadState::Paused
            | DownloadState::Queued
            | DownloadState::Downloading
            | DownloadState::Finalizing
            | DownloadState::Retrying => Self::Error,
        }
    }

    /// Severity rank for aggregating several results (`refresh --all`).
    const fn severity(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::NoResults => 1,
            Self::Partial => 2,
            Self::FeedError => 3,
            Self::NetworkError => 4,
            Self::DownloadFailed => 5,
            Self::Unresolvable | Self::AllProvidersFailed => 6,
            Self::Blocked => 7,
            Self::DiskFull => 8,
            Self::Usage => 9,
            // Only `serve` can produce it, and `serve` is never one of several
            // results — but the rank has to exist, and a refusal to start is
            // the worst outcome there is.
            Self::InsecureExposure => 10,
            Self::Error => 11,
        }
    }

    /// The worst of several exit codes.
    pub fn worst(codes: impl IntoIterator<Item = Self>) -> Self {
        codes.into_iter().fold(Self::Ok, |acc, c| {
            if c.severity() > acc.severity() {
                c
            } else {
                acc
            }
        })
    }
}

/// Prints any serializable value as JSON with `schema: 1` at the top level.
#[allow(clippy::print_stdout)]
pub fn json_with_schema<T: Serialize>(value: &T) {
    let mut v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut v {
        map.insert("schema".to_owned(), 1.into());
    }
    println!("{}", serde_json::to_string(&v).unwrap_or_default());
}

fn ts(t: OffsetDateTime) -> String {
    t.replace_nanosecond(0)
        .unwrap_or(t)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn opt_ts(t: Option<OffsetDateTime>) -> String {
    t.map_or_else(|| "-".to_owned(), ts)
}

fn ago(t: Option<OffsetDateTime>) -> String {
    let Some(t) = t else {
        return "never".to_owned();
    };
    let secs = (OffsetDateTime::now_utc() - t).whole_seconds();
    if secs < 0 {
        return ts(t);
    }
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn duration(secs: Option<u32>) -> String {
    let Some(s) = secs else {
        return "-".to_owned();
    };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn trunc(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// Text for `podcast list`.
pub fn render_podcasts(list: &[PodcastDetail]) -> String {
    if list.is_empty() {
        return "No podcasts yet. Add one with `uguisu podcast add <feed-url>`.\n".to_owned();
    }
    let mut out = format!(
        "{:<26} {:<40} {:>8} {:<8} {:<12} {}\n",
        "ID", "TITLE", "EPISODES", "STATUS", "REFRESHED", "FEED"
    );
    for d in list {
        let feed = d
            .source
            .as_ref()
            .map_or_else(|| "-".to_owned(), |s| s.fetch.state.as_str().to_owned());
        out += &format!(
            "{:<26} {:<40} {:>8} {:<8} {:<12} {}\n",
            d.podcast.id,
            trunc(&d.podcast.title, 40),
            d.episodes_present,
            d.podcast.status.as_str(),
            ago(d.podcast.last_refresh_at),
            feed
        );
    }
    out
}

/// Text for `episode list`: newest first, one line per episode.
pub fn render_episodes(list: &[uguisu_core::model::Episode]) -> String {
    if list.is_empty() {
        return "No episodes. `uguisu podcast refresh <id>` looks for some.\n".to_owned();
    }
    let mut out = format!(
        "{:<26} {:<10} {:<10} {:>8} {}\n",
        "ID", "PUBLISHED", "ARCHIVE", "DURATION", "TITLE"
    );
    for e in list {
        let published = e
            .published_at
            .map_or_else(|| "-".to_owned(), |t| ts(t).chars().take(10).collect());
        out += &format!(
            "{:<26} {:<10} {:<10} {:>8} {}\n",
            e.id,
            published,
            e.archive_state.as_str(),
            duration(e.duration_secs),
            trunc(&e.title, 60)
        );
    }
    out
}

/// Text for `episode show`.
pub fn render_episode(d: &EpisodeDetail) -> String {
    let e = &d.episode;
    let mut out = format!("{}\n", e.title);
    out += &format!("  ID:          {}\n", e.id);
    out += &format!("  Podcast:     {} ({})\n", d.podcast_title, e.podcast_id);
    out += &format!("  Published:   {}\n", opt_ts(e.published_at));
    out += &format!("  Duration:    {}\n", duration(e.duration_secs));
    if let Some(g) = &e.guid {
        out += &format!("  GUID:        {g}\n");
    }
    out += &format!(
        "  Identity:    {} ({})\n",
        e.identity.key,
        e.identity.source.as_str()
    );
    out += &format!("  Archive:     {}", e.archive_state.as_str());
    if let Some(r) = &e.skip_reason {
        out += &format!(" ({r})");
    }
    out.push('\n');
    if let Some(original) = e.duplicate_of_episode_id {
        out += &format!(
            "  Candidate:   probably the same as {original} ({}); `uguisu episode resolve`\n",
            e.duplicate_reasons.join(", ").replace('_', " ")
        );
    }
    if let Some(at) = e.removed_from_feed_at {
        out += &format!("  Removed:     from the feed, noticed {}\n", ts(at));
    }
    for enclosure in e.enclosures.iter().filter(|x| x.is_primary) {
        out += &format!(
            "  Enclosure:   {} ({}, {})\n",
            enclosure.url,
            enclosure.mime_type.as_deref().unwrap_or("unknown type"),
            enclosure.length_bytes.map_or_else(
                || "length not declared".to_owned(),
                |n| format!("{n} bytes")
            )
        );
    }
    if let Some(j) = &d.job {
        out += &format!("  Download:    {} (job {})\n", job_state(j), j.id);
    }
    if let Some(a) = &d.archive {
        out += &format!("  File:        {}\n", a.relative_path);
        out += &format!(
            "               {} bytes, {}:{}\n",
            a.size_bytes, a.hash_algo, a.hash_value
        );
        out += &format!(
            "  Verified:    {} ({})\n",
            a.verification_state.as_str(),
            opt_ts(a.verified_at)
        );
    }
    out
}

/// Text for `podcast show`.
pub fn render_podcast(d: &PodcastDetail) -> String {
    let p = &d.podcast;
    let mut out = format!("{}\n", p.title);
    out += &format!("  ID:          {}\n", p.id);
    out += &format!("  Status:      {}\n", p.status.as_str());
    if let Some(a) = &p.author {
        out += &format!("  Author:      {a}\n");
    }
    if let Some(w) = &p.website {
        out += &format!("  Website:     {w}\n");
    }
    if let Some(g) = &p.podcast_guid {
        out += &format!("  GUID:        {g}\n");
    }
    if let Some(l) = &p.language {
        out += &format!("  Language:    {l}\n");
    }
    out += &format!("  Feed kind:   {}\n", p.feed_kind.as_str());
    out += &format!(
        "  Episodes:    {} ({} not detected as removed)\n",
        d.episodes_total, d.episodes_present
    );
    out += &format!("  Refreshed:   {}\n", opt_ts(p.last_refresh_at));
    out += &format!("  Next:        {}\n", opt_ts(p.next_refresh_at));
    if let Some(e) = &p.last_error {
        out += &format!("  Last error:  {e}\n");
    }
    if let Some(s) = &d.source {
        out += &format!("  Source:      {} ({})\n", s.feed_url, s.id);
        out += &render_fetch_state(s, "               ");
    }
    if let Some(a) = &d.announced {
        out += &format!(
            "  Announced:   {} (since {}; not verified: {}: {})\n",
            a.feed_url,
            opt_ts(Some(a.discovered_at)),
            a.fetch.last_error_kind.map_or("-", FetchErrorKind::as_str),
            a.fetch.last_error_detail.as_deref().unwrap_or("-")
        );
        out += &format!(
            "               move with `uguisu podcast move-feed {} {}`\n",
            p.id, a.feed_url
        );
    }
    if let Some(f) = &d.last_fetch {
        out += &format!("  Last fetch:  {}\n", render_fetch_line(f));
    }
    if let Some(t) = &p.description_text {
        out += &format!("\n  {}\n", trunc(t, 400));
    }
    out
}

fn render_fetch_state(s: &PodcastSource, indent: &str) -> String {
    let f = &s.fetch;
    let mut out = format!(
        "{indent}state {} · failures {} · etag {} · last-modified {}\n",
        f.state.as_str(),
        f.consecutive_failures,
        f.etag.as_deref().unwrap_or("-"),
        f.last_modified.as_deref().unwrap_or("-")
    );
    out += &format!(
        "{indent}attempt {} · success {} · not-modified {}\n",
        opt_ts(f.last_attempt_at),
        opt_ts(f.last_success_at),
        opt_ts(f.last_not_modified_at)
    );
    if let Some(k) = f.last_error_kind {
        out += &format!(
            "{indent}last error {} at {}: {}\n",
            k.as_str(),
            opt_ts(f.last_error_at),
            f.last_error_detail.as_deref().unwrap_or("-")
        );
    }
    out
}

fn render_fetch_line(f: &FeedFetch) -> String {
    let outcome = match &f.outcome {
        RefreshOutcome::Fetched => "fetched".to_owned(),
        RefreshOutcome::NotModified { reason } => {
            format!("not modified ({reason:?})").to_lowercase()
        }
        RefreshOutcome::Failed { kind, detail } => {
            format!("failed ({}): {}", kind.as_str(), detail)
        }
    };
    format!(
        "{} {} · +{} ~{} ={} !{} -{} · {} ms",
        ts(f.fetched_at),
        outcome,
        f.episodes.added,
        f.episodes.updated,
        f.episodes.unchanged,
        f.episodes.malformed,
        f.episodes.removed_detected,
        f.duration_ms
    )
}

/// Text for `feed status`.
pub fn render_feed_status(s: &PodcastSource, last: Option<&FeedFetch>) -> String {
    let mut out = format!("Source {}\n", s.id);
    out += &format!("  Podcast:     {}\n", s.podcast_id);
    out += &format!("  Feed URL:    {}\n", s.feed_url);
    if let Some(c) = &s.canonical_url {
        out += &format!("  Canonical:   {c}\n");
    }
    out += &format!(
        "  Current:     {}{}\n",
        s.is_current,
        s.replaced_by_source_id
            .map(|r| format!(" (replaced by {r})"))
            .unwrap_or_default()
    );
    out += &format!("  Verified:    {}\n", opt_ts(s.verified_at));
    out += &format!(
        "  Fingerprint: {}\n",
        s.fetch
            .content_fingerprint
            .as_deref()
            .map_or_else(|| "-".to_owned(), |f| trunc(f, 16))
    );
    out += &render_fetch_state(s, "  ");
    if let Some(f) = last {
        out += &format!("  Last fetch:  {}\n", render_fetch_line(f));
        for w in &f.warnings {
            out += &format!("    ! {w}\n");
        }
    }
    out
}

/// Text for a refresh report (brief §56).
pub fn render_report(title: &str, r: &RefreshReport) -> String {
    let mut out = String::new();
    match &r.outcome {
        RefreshOutcome::Fetched => {
            out += &format!(
                "{title}: fetched · {} seen · {} added · {} updated · {} unchanged · {} malformed · {} removed · {} ms\n",
                r.episodes.seen,
                r.episodes.added,
                r.episodes.updated,
                r.episodes.unchanged,
                r.episodes.malformed,
                r.episodes.removed_detected,
                r.duration_ms
            );
            if r.episodes.ambiguous > 0 {
                out += &format!(
                    "  {} probable duplicate(s) stored as skipped candidates\n",
                    r.episodes.ambiguous
                );
            }
            if !r.podcast_changed_fields.is_empty() {
                out += &format!(
                    "  podcast metadata changed: {}\n",
                    r.podcast_changed_fields.join(", ")
                );
            }
            if r.truncated {
                out += "  feed truncated: removal detection skipped\n";
            }
            if let Some(why) = &r.removal_suppressed {
                out += &format!("  removal detection suppressed: {why}\n");
            }
        }
        RefreshOutcome::NotModified { reason } => {
            let via = match reason {
                uguisu_core::feed::NotModifiedReason::Http304 => "HTTP 304",
                uguisu_core::feed::NotModifiedReason::Fingerprint => "same body fingerprint",
            };
            out += &format!("{title}: not modified ({via}) · {} ms\n", r.duration_ms);
        }
        RefreshOutcome::Failed { kind, detail } => {
            out += &format!("{title}: failed ({}): {detail}\n", kind.as_str());
        }
    }
    if let Some(status) = r.http.status {
        out += &format!(
            "  http {status}{}{}{}\n",
            if r.http.conditional {
                " (conditional)"
            } else {
                ""
            },
            r.http
                .bytes
                .map(|b| format!(" · {b} bytes"))
                .unwrap_or_default(),
            if r.http.redirects > 0 {
                format!(" · {} redirect(s)", r.http.redirects)
            } else {
                String::new()
            }
        );
    }
    match &r.feed_url {
        FeedUrlStatus::Unchanged => {}
        FeedUrlStatus::ChangeDetected { announced, reason } => {
            out += &format!("  feed announces a new URL {announced} (not adopted: {reason})\n");
            out += &format!(
                "  move with `uguisu podcast move-feed {} {announced}`\n",
                r.podcast_id
            );
        }
        FeedUrlStatus::Changed { from, to, via } => {
            out += &format!("  feed URL changed: {from} → {to} ({})\n", via.as_str());
        }
    }
    for w in r.warnings.iter().take(20) {
        out += &format!("  ! {w}\n");
    }
    if r.warnings.len() > 20 {
        out += &format!("  … {} more warning(s)\n", r.warnings.len() - 20);
    }
    out
}

/// Text for `podcast refresh --all`.
pub fn render_refresh_all(entries: &[RefreshAllEntry]) -> String {
    if entries.is_empty() {
        return "No podcasts to refresh.\n".to_owned();
    }
    let mut out = String::new();
    for e in entries {
        match (&e.report, &e.error) {
            (Some(r), _) => out += &render_report(&e.title, r),
            (None, Some(err)) => out += &format!("{}: error: {err}\n", e.title),
            (None, None) => out += &format!("{}: no result\n", e.title),
        }
    }
    let ok = entries
        .iter()
        .filter(|e| e.report.as_ref().is_some_and(|r| r.outcome.is_success()))
        .count();
    out += &format!("\n{ok} of {} podcast(s) refreshed\n", entries.len());
    out
}

/// Text for `podcast add`.
pub fn render_add(a: &AddOutcome) -> String {
    let mut out = if a.created {
        format!("Added {} ({})\n", a.podcast.title, a.podcast.id)
    } else {
        format!(
            "Already in the library: {} ({})\n",
            a.podcast.title, a.podcast.id
        )
    };
    out += &format!("  Feed: {}\n", a.source.feed_url);
    if let Some(r) = &a.report {
        out += &render_report("  First refresh", r);
    }
    out
}

/// Text for `podcast move-feed`.
pub fn render_move(m: &FeedMove, options: MoveOptions) -> String {
    if m.from == m.to {
        return format!("Podcast {} already uses {}\n", m.podcast_id, m.to);
    }
    let mut out = if m.moved {
        format!(
            "Moved podcast {} from {} to {}\n",
            m.podcast_id, m.from, m.to
        )
    } else if options.dry_run && (m.verified || options.force) {
        format!(
            "Would move podcast {} to {} (dry run)\n",
            m.podcast_id, m.to
        )
    } else {
        format!("Not moved: podcast {} stays at {}\n", m.podcast_id, m.from)
    };
    out += &format!(
        "  check: {}: {}\n",
        if m.verified {
            "same podcast"
        } else {
            "not verified"
        },
        m.check
    );
    if let Some(r) = &m.report {
        out += &render_report("  Refresh", r);
    }
    out
}

/// Text for `episode duplicates`: each candidate, the episode it probably
/// duplicates and why, then how to resolve them.
pub fn render_duplicates(pairs: &[uguisu_engine::duplicates::DuplicatePair]) -> String {
    if pairs.is_empty() {
        return "No candidate duplicates\n".to_owned();
    }
    let mut out = String::new();
    for pair in pairs {
        let c = &pair.candidate;
        out += &format!("{}  {}\n", c.id, c.title);
        match &pair.original {
            Some(o) => out += &format!("  probably the same as {}  {}\n", o.id, o.title),
            None => out += "  names an episode that no longer exists\n",
        }
        if !c.duplicate_reasons.is_empty() {
            out += &format!(
                "  because: {}\n",
                c.duplicate_reasons.join(", ").replace('_', " ")
            );
        }
    }
    out += "Resolve each with `uguisu episode resolve <id> --same` (merge) or `--separate` (keep both).\n";
    out
}

/// Text for `episode resolve`.
pub fn render_resolution(r: &uguisu_engine::duplicates::DuplicateResolved) -> String {
    match r.resolution {
        uguisu_core::model::DuplicateResolution::Same => {
            format!("Merged episode {} into {}\n", r.candidate, r.original)
        }
        uguisu_core::model::DuplicateResolution::Separate => format!(
            "Separated episode {} from {}{}\n",
            r.candidate,
            r.original,
            if r.queued {
                "; queued for download"
            } else {
                ""
            }
        ),
    }
}

/// Text for `podcast import`: the counts, every outline that was neither
/// added nor already there with why, and what happens next.
pub fn render_opml_import(r: &OpmlImport) -> String {
    let c = &r.counts;
    let mut out = if r.applied {
        format!("Added {} of {} feeds\n", c.add, r.items.len())
    } else {
        format!("Would add {} of {} feeds (dry run)\n", c.add, r.items.len())
    };
    for (label, n) in [
        ("already present", c.already_present),
        ("duplicate", c.duplicate),
        ("invalid", c.invalid),
        ("needs review", c.needs_review),
        ("conflict", c.conflict),
        ("failed", c.failed),
    ] {
        if n > 0 {
            out += &format!("  {label}: {n}\n");
        }
    }
    for item in &r.items {
        if matches!(item.action, OpmlAction::Add | OpmlAction::AlreadyPresent) {
            continue;
        }
        out += &format!("  {}: {}", item.action.as_str(), item.xml_url);
        if let Some(detail) = &item.detail {
            out += &format!(" ({detail})");
        }
        out.push('\n');
    }
    if c.add > 0 {
        out += if r.applied {
            "Their episodes arrive with the next scheduled refresh; `uguisu podcast refresh --all` fetches them now.\n"
        } else {
            "Run again with --apply to add them.\n"
        };
    }
    out
}

/// Text for `feed inspect`.
pub fn render_inspection(i: &Inspection) -> String {
    let mut out = format!("{}\n", i.title);
    out += &format!("  URL:         {}\n", i.url);
    if let Some(f) = &i.http.final_url
        && *f != i.url
    {
        out += &format!("  Final URL:   {f} ({} redirect(s))\n", i.http.redirects);
    }
    out += &format!(
        "  HTTP:        {}{} · etag {} · last-modified {}\n",
        i.http.status.unwrap_or(0),
        i.http
            .bytes
            .map(|b| format!(" · {b} bytes"))
            .unwrap_or_default(),
        i.http.etag.as_deref().unwrap_or("-"),
        i.http.last_modified.as_deref().unwrap_or("-")
    );
    out += &format!("  Kind:        {} ({})\n", i.kind.as_str(), i.encoding);
    if let Some(a) = &i.author {
        out += &format!("  Author:      {a}\n");
    }
    if let Some(g) = &i.podcast_guid {
        out += &format!("  GUID:        {g}\n");
    }
    if let Some(n) = &i.new_feed_url {
        out += &format!("  New feed:    {n}\n");
    }
    if let Some(l) = i.locked {
        out += &format!("  Locked:      {l}\n");
    }
    out += &format!(
        "  Items:       {} ({} with enclosure, {} malformed{}){}\n",
        i.items,
        i.items_with_enclosure,
        i.malformed_items,
        if i.truncated { ", truncated" } else { "" },
        if i.looks_like_podcast {
            ""
        } else {
            " — does not look like a podcast feed"
        }
    );
    let sources: Vec<String> = i
        .identity_sources
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    if !sources.is_empty() {
        out += &format!("  Identity:    {}\n", sources.join(", "));
    }
    out += &format!("  Warnings:    {}\n", i.warning_count);
    if !i.preview.is_empty() {
        out += "\n  First items:\n";
        for it in &i.preview {
            out += &format!(
                "    {:>3}. {}  [{}]  {}  {}\n",
                it.index + 1,
                trunc(&it.title, 50),
                duration(it.duration_secs),
                opt_ts(it.published_at),
                it.enclosure_url
                    .as_ref()
                    .map_or_else(|| "(no enclosure)".to_owned(), ToString::to_string)
            );
        }
    }
    for w in i.warnings.iter().take(20) {
        out += &format!("  ! {w}\n");
    }
    if i.warning_count > 20 {
        out += &format!("  … {} more warning(s)\n", i.warning_count - 20);
    }
    out += &format!("  Parsed in {} ms\n", i.duration_ms);
    out
}

/// Human-readable byte count (`12.3 MiB`).
#[allow(clippy::cast_precision_loss)] // display only
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `bytes/total (pct)` or just the bytes when the total is unknown.
#[allow(clippy::cast_precision_loss)] // display only
pub fn progress_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!(
            "{} / {} ({:.0}%)",
            bytes(done),
            bytes(t),
            (done as f64 / t as f64) * 100.0
        ),
        _ => bytes(done),
    }
}

fn eta(secs: Option<u64>) -> String {
    let Some(s) = secs else {
        return "-".to_owned();
    };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// One progress line for stderr (`42% 8.4 MiB / 20.0 MiB 12.3 MiB/s eta 0:31`).
pub fn progress_line(
    done: u64,
    total: Option<u64>,
    speed_bps: u64,
    eta_secs: Option<u64>,
) -> String {
    format!(
        "{}  {}/s  eta {}",
        progress_text(done, total),
        bytes(speed_bps),
        eta(eta_secs)
    )
}

fn job_state(j: &DownloadJob) -> String {
    match &j.state_reason {
        Some(r) if !r.is_empty() => format!("{}({r})", j.state.as_str()),
        _ => j.state.as_str().to_owned(),
    }
}

/// Text for `download list`.
pub fn render_jobs(jobs: &[DownloadJob], next_after: Option<&str>) -> String {
    if jobs.is_empty() {
        return "No download jobs. Queue one with `uguisu download <episode-id>`.\n".to_owned();
    }
    let mut out = format!(
        "{:<26} {:<26} {:<24} {:<32} {:>8} {}\n",
        "JOB", "EPISODE", "STATE", "PROGRESS", "ATTEMPTS", "UPDATED"
    );
    for j in jobs {
        out += &format!(
            "{:<26} {:<26} {:<24} {:<32} {:>8} {}\n",
            j.id,
            j.episode_id,
            trunc(&job_state(j), 24),
            progress_text(j.bytes_downloaded, j.total_bytes),
            format!("{}/{}", j.attempt_count, j.max_attempts),
            ago(Some(j.updated_at))
        );
    }
    if let Some(after) = next_after {
        out += &format!("More: `uguisu download list --after {after}`\n");
    }
    out
}

#[allow(clippy::cast_precision_loss)] // display only
fn render_attempt(a: &DownloadAttempt) -> String {
    let outcome = a
        .outcome
        .map_or_else(|| "running".to_owned(), |o| o.as_str().to_owned());
    let mut line = format!(
        "  #{:<3} {:<16} {:<20} from {:>12}  {:>12} in {:>8}",
        a.attempt_no,
        ts(a.started_at),
        outcome,
        bytes(a.range_start),
        bytes(a.bytes_received),
        format!("{:.1}s", a.duration_ms as f64 / 1000.0)
    );
    if let Some(status) = a.http_status {
        line += &format!("  http {status}");
    }
    if let Some(kind) = a.error_kind {
        line += &format!("  {kind}");
    }
    if let Some(detail) = &a.error_detail {
        line += &format!(": {}", trunc(detail, 80));
    }
    line.push('\n');
    line
}

/// Text for `download show`.
pub fn render_job(d: &JobDetail) -> String {
    let j = &d.job;
    let mut out = format!("Job {}\n", j.id);
    out += &format!("  Episode:     {}\n", j.episode_id);
    out += &format!("  Podcast:     {}\n", j.podcast_id);
    out += &format!("  State:       {}\n", job_state(j));
    out += &format!("  Priority:    {}\n", j.priority.as_str());
    out += &format!("  URL:         {}\n", j.source_url);
    out += &format!("  Target:      {}\n", j.target_path);
    out += &format!(
        "  Progress:    {}\n",
        progress_text(j.bytes_downloaded, j.total_bytes)
    );
    if let Some(p) = &d.progress {
        out += &format!(
            "  Live:        {}\n",
            progress_line(p.bytes_downloaded, p.total_bytes, p.speed_bps, p.eta_secs)
        );
    }
    out += &format!("  Attempts:    {}/{}\n", j.attempt_count, j.max_attempts);
    if let Some(next) = j.next_attempt_at {
        out += &format!("  Next try:    {}\n", ts(next));
    }
    if let Some(h) = &j.hash_value {
        out += &format!("  Hash:        {}:{h}\n", j.hash_algo);
    }
    if let Some(ct) = &j.content_type {
        out += &format!("  Content:     {ct}");
        if let Some(sniffed) = &j.sniffed_type {
            out += &format!(" (looks like {sniffed})");
        }
        out.push('\n');
    }
    if j.etag.is_some() || j.last_modified.is_some() || j.accept_ranges.is_some() {
        out += &format!(
            "  Resume:      ranges {}, etag {}, last-modified {}\n",
            match j.accept_ranges {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            },
            j.etag.as_deref().unwrap_or("-"),
            j.last_modified.as_deref().unwrap_or("-")
        );
    }
    if let Some(kind) = j.last_error_kind {
        out += &format!(
            "  Last error:  {kind}{}{}\n",
            j.last_http_status
                .map_or_else(String::new, |s| format!(" (http {s})")),
            j.last_error_detail
                .as_deref()
                .map_or_else(String::new, |d| format!(": {}", trunc(d, 120)))
        );
    }
    out += &format!("  Created:     {}\n", ts(j.created_at));
    out += &format!("  Started:     {}\n", opt_ts(j.started_at));
    out += &format!("  Finished:    {}\n", opt_ts(j.finished_at));
    if !d.attempts.is_empty() {
        out.push_str("  Attempt history:\n");
        for a in &d.attempts {
            out += &render_attempt(a);
        }
    }
    out
}

/// Text for `download <episode-id>`.
pub fn render_enqueue(o: &EnqueueOutcome) -> String {
    let j = o.job();
    match o {
        EnqueueOutcome::Created(_) => format!(
            "Queued episode {} as job {} (priority {}).\nWorkers run in `uguisu serve` or `uguisu download run`; add --wait to download now.\n",
            j.episode_id,
            j.id,
            j.priority.as_str()
        ),
        EnqueueOutcome::Existing(_) => {
            format!(
                "Job {} for episode {} already exists ({}).\n",
                j.id,
                j.episode_id,
                job_state(j)
            )
        }
        EnqueueOutcome::Requeued(_) => format!(
            "Re-queued job {} for episode {} with a fresh attempt budget.\n",
            j.id, j.episode_id
        ),
        EnqueueOutcome::AlreadyCompleted(_) => format!(
            "Episode {} is already downloaded: {}\n",
            j.episode_id, j.target_path
        ),
    }
}

/// Text for `download podcast <id>`.
pub fn render_summary(s: &EnqueueSummary) -> String {
    let mut out = format!(
        "Queued {} new job(s); {} already pending, {} re-queued, {} already downloaded, {} skipped.\n",
        s.created,
        s.existing,
        s.requeued,
        s.completed,
        s.skipped.len()
    );
    for sk in &s.skipped {
        out += &format!("  skipped {}: {}\n", sk.episode_id, sk.reason);
    }
    out
}

/// Text for `download stats`.
pub fn render_stats(s: &DownloadStats) -> String {
    let mut out = String::from("Download queue\n");
    for state in DownloadState::ALL {
        let n = s.by_state.get(&state).copied().unwrap_or(0);
        if n > 0 {
            out += &format!("  {:<12} {n}\n", state.as_str());
        }
    }
    if s.by_state.values().all(|n| *n == 0) {
        out.push_str("  (empty)\n");
    }
    out += &format!(
        "  workers:     {}{}\n",
        if s.workers_started {
            "started"
        } else {
            "not started"
        },
        if s.running > 0 {
            format!(", {} running", s.running)
        } else {
            String::new()
        }
    );
    if let Some(reason) = s.paused_all {
        out += &format!("  paused:      yes ({})\n", reason.as_str());
    }
    if let Some(next) = s.next_retry_at {
        out += &format!("  next retry:  {}\n", ts(next));
    }
    if s.orphan_parts > 0 {
        out += &format!("  orphan .part files: {}\n", s.orphan_parts);
    }
    out
}

/// Text for `download pause-all` / `resume-all`.
pub fn render_control(c: &DownloadControl) -> String {
    if c.paused {
        format!(
            "Downloads paused ({}) since {}.\n",
            c.paused_reason.map_or("user", |r| r.as_str()),
            opt_ts(c.paused_at)
        )
    } else {
        "Downloads resumed.\n".to_owned()
    }
}

/// Text for `download reconcile`.
pub fn render_reconcile(r: &ReconcileReport) -> String {
    let mut out = String::from("Reconciliation\n");
    out += &format!("  recovered (downloading -> queued): {}\n", r.recovered);
    out += &format!("  finalized:                        {}\n", r.finalized);
    out += &format!(
        "  finalization lost:                {}\n",
        r.finalization_lost
    );
    out += &format!(
        "  finalization conflicts:           {}\n",
        r.finalization_conflicts
    );
    out += &format!(
        "  finalization failed:              {}\n",
        r.finalization_failed
    );
    out += &format!(
        "  missing targets:                  {}\n",
        r.missing_targets
    );
    out += &format!(
        "  unexpected targets:               {}\n",
        r.unexpected_targets
    );
    out += &format!(
        "  orphan .part files:               {}\n",
        r.orphan_parts.len()
    );
    for p in &r.orphan_parts {
        out += &format!("    {p}\n");
    }
    out
}

// Archive output.

/// Text for `archive list`, `archive missing` and `archive invalid`.
#[must_use]
pub fn render_archive_files(files: &[ArchiveFile], what: &str) -> String {
    if files.is_empty() {
        return format!("No {what} archived files.\n");
    }
    let mut out = format!(
        "{:<26} {:<10} {:>10} {:<18} {}\n",
        "EPISODE", "STATE", "SIZE", "CHECKED", "PATH"
    );
    for f in files {
        out += &format!(
            "{:<26} {:<10} {:>10} {:<18} {}\n",
            f.episode_id,
            f.verification_state.as_str(),
            bytes(f.size_bytes),
            ago(f.verified_at),
            trunc(&f.relative_path, 60)
        );
    }
    out
}

/// Text for `archive show`.
#[must_use]
pub fn render_archive_file(f: &ArchiveFile) -> String {
    let mut out = format!("Episode  {}\n", f.episode_id);
    out += &format!("Podcast  {}\n", f.podcast_id);
    out += &format!("Path     {}\n", f.relative_path);
    out += &format!(
        "Size     {} ({} bytes)\n",
        bytes(f.size_bytes),
        f.size_bytes
    );
    out += &format!("Hash     {}:{}\n", f.hash_algo, f.hash_value);
    if let Some(ct) = &f.content_type {
        out += &format!("Type     {ct}");
        if let Some(sniffed) = &f.sniffed_type {
            out += &format!(" (looks like {sniffed})");
        }
        out.push('\n');
    }
    out += &format!(
        "State    {}{}\n",
        f.verification_state.as_str(),
        f.verification_reason
            .as_deref()
            .map_or_else(String::new, |r| format!(" ({r})"))
    );
    out += &format!("Checked  {}\n", ago(f.verified_at));
    out += &format!("Archived {}\n", ts(f.registered_at));
    out
}

/// Text for one verification.
#[must_use]
pub fn render_verified(v: &VerifiedFile) -> String {
    let mut out = format!(
        "{} {} ({})\n",
        v.state.as_str(),
        v.file.relative_path,
        v.reason
    );
    if let Some(detail) = &v.detail {
        out += &format!("  {detail}\n");
    }
    match v.state {
        VerificationState::Missing => {
            out += "  The record is kept: put the file back, or download it again.\n";
        }
        VerificationState::Invalid => {
            out += "  Nothing was changed or deleted. Download it again to replace it.\n";
        }
        _ => {}
    }
    out
}

/// Text for a verification run.
#[must_use]
pub fn render_verify_summary(s: &VerifySummary) -> String {
    let mut out = format!(
        "Checked {} file(s) ({} pass): {} verified",
        s.checked,
        s.depth.as_str(),
        s.verified
    );
    if s.missing > 0 {
        out += &format!(", {} missing", s.missing);
    }
    if s.invalid > 0 {
        out += &format!(", {} invalid", s.invalid);
    }
    if s.unchecked > 0 {
        out += &format!(", {} could not be read", s.unchecked);
    }
    out.push('\n');
    if s.has_problems() {
        out += "Nothing was deleted or repaired. See `uguisu archive missing` and `uguisu archive invalid`.\n";
    }
    out
}

/// Text for `archive path-preview`.
#[must_use]
pub fn render_path_preview(p: &PathPreview) -> String {
    let mut out = format!("Template  {}\n", p.template);
    out += &format!("Renders   {}\n", p.rendered);
    if p.resolved != p.rendered {
        out += &format!("Resolved  {} (the rendered path is taken)\n", p.resolved);
    }
    match &p.current {
        Some(current) if p.would_move => {
            out += &format!("Currently {current}\n");
            out += "Run `uguisu archive relocate` to move it there.\n";
        }
        Some(_) => out += "The file is already where the template puts it.\n",
        None => out += "Nothing is archived for this episode yet.\n",
    }
    out
}

/// Text for `archive relocate`.
#[must_use]
pub fn render_relocations(moves: &[Relocation], dry_run: bool) -> String {
    let changed: Vec<&Relocation> = moves.iter().filter(|m| m.from != m.to).collect();
    if changed.is_empty() {
        return format!(
            "Nothing to move: {} file(s) are already where the template puts them.\n",
            moves.len()
        );
    }
    let mut out = String::new();
    for m in &changed {
        out += &format!("{}\n  -> {}\n", m.from, m.to);
    }
    out += &format!(
        "{} {} file(s).\n",
        if dry_run { "Would move" } else { "Moved" },
        changed.len()
    );
    out
}

/// Text for `archive policy show`.
#[must_use]
pub fn render_policy(value: &serde_json::Value) -> String {
    let effective = &value["effective"];
    let str_of = |v: &serde_json::Value| v.as_str().unwrap_or("?").to_owned();
    let mut out = format!("Podcast      {}\n", str_of(&value["podcast_id"]));
    out += &format!("Mode         {}\n", str_of(&effective["mode"]));
    out += &format!(
        "Backlog      {}\n",
        match effective["max_backlog"].as_u64() {
            Some(0) | None => "unlimited".to_owned(),
            Some(n) => n.to_string(),
        }
    );
    out += &format!(
        "Max age      {}\n",
        match effective["max_age_days"].as_u64() {
            Some(0) | None => "no limit".to_owned(),
            Some(n) => format!("{n} day(s)"),
        }
    );
    out += &format!("Priority     {}\n", str_of(&effective["priority"]));
    if value["stored"].is_null() {
        out += "No override stored: these are the global defaults.\n";
    } else {
        out += "Stored as an override for this podcast.\n";
    }
    out
}

/// Text for `archive policy list`.
#[must_use]
pub fn render_policies(policies: &[ArchivePolicy]) -> String {
    if policies.is_empty() {
        return "No per-podcast policies. The global defaults apply everywhere.\n".to_owned();
    }
    let mut out = format!(
        "{:<26} {:<8} {:>8} {:>10} {}\n",
        "PODCAST", "MODE", "BACKLOG", "MAX AGE", "PRIORITY"
    );
    for p in policies {
        let opt = |v: Option<u32>| v.map_or_else(|| "-".to_owned(), |n| n.to_string());
        out += &format!(
            "{:<26} {:<8} {:>8} {:>10} {}\n",
            p.podcast_id,
            match p.mode {
                uguisu_core::archive::PolicyMode::Auto => "auto",
                uguisu_core::archive::PolicyMode::Manual => "manual",
            },
            opt(p.max_backlog),
            opt(p.max_age_days),
            p.priority.map_or_else(|| "-".to_owned(), |x| x.to_string()),
        );
    }
    out += "A `-` means the global default applies.\n";
    out
}

/// Text for `archive reconcile`.
#[must_use]
pub fn render_archive_reconcile(r: &ArchiveReconcileReport) -> String {
    let mut out = format!(
        "Registered {} finished download(s); checked {} file(s).\n",
        r.registered, r.checked
    );
    if r.unregisterable > 0 {
        out += &format!(
            "{} finished download(s) could not be registered (their files were not found).\n",
            r.unregisterable
        );
    }
    if r.missing > 0 || r.invalid > 0 {
        out += &format!(
            "{} missing, {} invalid. Nothing was deleted.\n",
            r.missing, r.invalid
        );
    }
    out
}

/// `scheduler status`.
#[must_use]
pub fn render_scheduler(s: &uguisu_engine::scheduler::SchedulerStatus) -> String {
    let mut out = String::new();
    let state = if !s.enabled {
        "disabled".to_owned()
    } else if s.paused {
        match &s.paused_reason {
            Some(reason) => format!("paused ({reason})"),
            None => "paused".to_owned(),
        }
    } else if s.running {
        "running".to_owned()
    } else {
        "idle (no serve process here)".to_owned()
    };
    out.push_str(&format!("scheduler   {state}\n"));
    out.push_str(&format!(
        "interval    {}s, {} at a time\n",
        s.interval_secs, s.concurrency
    ));
    out.push_str(&format!(
        "due now     {} ({} refreshing)\n",
        s.due_now, s.inflight
    ));
    if let Some(at) = s.next_due_at {
        out.push_str(&format!("next due    {at}\n"));
    }
    if let Some(at) = s.last_maintenance_at {
        out.push_str(&format!("maintenance {at}\n"));
    }
    out
}

/// `config list` / `config validate`.
#[must_use]
pub fn render_settings(report: &uguisu_engine::settings::SettingsReport) -> String {
    let mut out = String::new();
    let width = report.keys.iter().map(|d| d.key.len()).max().unwrap_or(0);
    for d in &report.keys {
        let mut note = String::new();
        if d.pinned {
            note.push_str("  pinned: the stored value is ignored");
        } else if !d.persistable {
            note.push_str("  (environment only)");
        } else if !d.live {
            note.push_str("  (restart required to change)");
        }
        out.push_str(&format!(
            "{:width$}  {}  {}{}\n",
            d.key,
            d.value,
            d.origin,
            note,
            width = width
        ));
    }
    if !report.rejected.is_empty() {
        out.push_str("\nignored stored values (kept, not applied):\n");
        for r in &report.rejected {
            out.push_str(&format!("  {} = {}  — {}\n", r.key, r.value, r.message));
        }
    }
    if !report.unused.is_empty() {
        out.push_str("\nstored but not in force:\n");
        for u in &report.unused {
            out.push_str(&format!("  {} = {}  — {}\n", u.key, u.value, u.reason));
        }
    }
    out
}

/// `search library`.
#[must_use]
pub fn render_library_search(r: &uguisu_engine::search::SearchResults, explain: bool) -> String {
    let mut out = String::new();
    match r.outcome {
        uguisu_engine::search::SearchOutcome::EmptyQuery => {
            return "nothing to search for\n".to_owned();
        }
        uguisu_engine::search::SearchOutcome::IndexStale => {
            out.push_str("the search index has not been built; run `uguisu search reindex`\n\n");
        }
        uguisu_engine::search::SearchOutcome::IndexBuilding => {
            out.push_str(&format!(
                "the search index is still building ({} podcasts, {} episodes so far)\n\n",
                r.index.podcasts, r.index.episodes
            ));
        }
        uguisu_engine::search::SearchOutcome::NoResults
        | uguisu_engine::search::SearchOutcome::Ok => {}
    }
    if r.truncated {
        out.push_str("(the query was shortened)\n");
    }
    if !r.podcasts.is_empty() {
        out.push_str("podcasts\n");
        for p in &r.podcasts {
            out.push_str(&format!(
                "  {:.2}  {}  {}\n",
                p.score, p.hit.title, p.hit.podcast_id
            ));
            if explain {
                out.push_str(&render_signals(&p.signals));
            }
        }
    }
    if !r.episodes.is_empty() {
        out.push_str("episodes\n");
        for e in &r.episodes {
            out.push_str(&format!(
                "  {:.2}  {}  — {}  {}\n",
                e.score, e.hit.title, e.hit.podcast_title, e.hit.episode_id
            ));
            if !e.hit.snippet.is_empty() {
                out.push_str(&format!("        {}\n", e.hit.snippet));
            }
            if explain {
                out.push_str(&render_signals(&e.signals));
            }
        }
    }
    if r.podcasts.is_empty() && r.episodes.is_empty() {
        out.push_str("no matches\n");
    }
    out
}

fn render_signals(signals: &[uguisu_engine::search::SearchSignal]) -> String {
    let mut out = String::new();
    for s in signals {
        out.push_str(&format!(
            "        {:<12} {:.2} × {:.2} = {:.3}  {}\n",
            s.name, s.weight, s.value, s.contribution, s.note
        ));
    }
    out
}

// Authentication output.

/// Prints a one-line confirmation, or the same fact as JSON.
#[allow(clippy::print_stdout)]
pub fn plain(json: bool, command: &str, message: &str) {
    if json {
        json_with_schema(&serde_json::json!({ "command": command, "message": message }));
    } else {
        println!("{message}");
    }
}

/// Prints a token and its secret.
///
/// The secret appears here and nowhere else, ever — not in `token list`, not
/// in the database, not in a log — so the text form says so rather than
/// leaving the reader to find that out later.
#[allow(clippy::print_stdout)]
pub fn token_created(json: bool, token: &serde_json::Value, secret: &str) {
    if json {
        json_with_schema(&serde_json::json!({ "token": token, "secret": secret }));
        return;
    }
    let name = token["name"].as_str().unwrap_or("-");
    let id = token["id"].as_str().unwrap_or("-");
    let scope = token["scope"].as_str().unwrap_or("-");
    println!("Token {id} ({name}, {scope}).");
    println!("{secret}");
    println!("This is the only time the secret is shown; store it now.");
}

/// Prints the token list. Never a secret: there is none to print.
#[allow(clippy::print_stdout)]
pub fn tokens(json: bool, tokens: &serde_json::Value) {
    if json {
        json_with_schema(&serde_json::json!({ "tokens": tokens }));
        return;
    }
    let Some(rows) = tokens.as_array() else {
        println!("no tokens");
        return;
    };
    if rows.is_empty() {
        println!("no tokens");
        return;
    }
    for row in rows {
        let state = if row["revoked_at"].is_string() {
            "revoked"
        } else if row["expires_at"].is_string() {
            "expires"
        } else {
            "active"
        };
        println!(
            "{}  {:<20} {:<5} {}  last used {}",
            row["id"].as_str().unwrap_or("-"),
            row["name"].as_str().unwrap_or("-"),
            row["scope"].as_str().unwrap_or("-"),
            state,
            row["last_used_at"].as_str().unwrap_or("never"),
        );
    }
}
