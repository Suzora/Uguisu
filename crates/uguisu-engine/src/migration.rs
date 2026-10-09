//! Feed URL migration (`docs/FEED_ENGINE.md`, ADR 0016).
//!
//! A feed can announce a new home in two ways: a permanent redirect chain
//! (301/308) that ends somewhere else, or an `itunes:new-feed-url` that
//! differs from the current URL. Uguisu never follows an announcement
//! blindly. The announced feed is fetched and parsed, and it must be the
//! same show: an equal `podcast:guid`, or the same normalized title and
//! at least half of the known episodes present. Only then is the current
//! source replaced atomically (old row kept as history, new row current,
//! validators carried from the verification fetch). Anything else is
//! reported as `feed.url.change_detected`, stored as the podcast's
//! announced source and checked again on the next refresh (ADR 0052).
//!
//! Any redirect — permanent or not — is checked the same way before the
//! fetched items are written: a feed that redirects to a different podcast
//! is a failed refresh, not a silent merge.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::feed::{FetchErrorKind, HttpSummary, RefreshReport};
use uguisu_core::ids::{PodcastId, SourceId};
use uguisu_core::model::{FetchState, FetchStatus, Podcast, PodcastSource, ReplacementReason};
use uguisu_core::{Event, EventKind, UguisuError};
use uguisu_feed::identity::{normalize_title, resolve_identities, signals};
use uguisu_feed::normalize::{normalize_channel, normalize_item};
use uguisu_feed::{ParsedFeed, parse};
use uguisu_http::{CancellationToken, GetOptions, Response};
use uguisu_storage::{episodes, events, podcasts, sources};
use url::Url;

use crate::Engine;
use crate::opml::usable_url;
use crate::refresh::{RefreshOptions, error_kind, looks_like_xml, parse_error_kind};

/// A new location the feed announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Where the feed says it lives now.
    pub url: Url,
    /// How it said so.
    pub via: ReplacementReason,
    /// Human-readable trigger.
    pub reason: String,
}

/// Why a feed is not, or not yet, a podcast's new home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unverified {
    /// A fetch error's kind, or `invalid_podcast_feed` for a feed that is
    /// not this podcast.
    pub kind: FetchErrorKind,
    /// What went wrong.
    pub detail: String,
    /// The status the feed answered with, if it answered.
    pub http_status: Option<u16>,
}

impl Unverified {
    /// A feed that loaded but failed the same-show check.
    #[must_use]
    pub fn not_this_podcast(detail: String, cand: &Candidate) -> Self {
        Self {
            kind: FetchErrorKind::InvalidPodcastFeed,
            detail,
            http_status: cand.http.status,
        }
    }
}

impl std::fmt::Display for Unverified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.detail)
    }
}

/// What a person asked a move to do (ADR 0052).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoveOptions {
    /// Check, and never move.
    pub dry_run: bool,
    /// Move even when the same-show check fails.
    pub force: bool,
}

/// What [`Engine::move_feed`] found and did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FeedMove {
    /// The podcast.
    pub podcast_id: PodcastId,
    /// Its feed URL before the move.
    pub from: Url,
    /// The feed URL it moves to, after redirects.
    pub to: Url,
    /// Whether the same-show check passed.
    pub verified: bool,
    /// Why it passed, or why not.
    pub check: String,
    /// Whether the podcast now uses `to`.
    pub moved: bool,
    /// The refresh that followed the move.
    pub report: Option<RefreshReport>,
}

/// What a feed looks like for the same-show check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// URL the feed was fetched from (after redirects).
    pub url: Url,
    /// Its `podcast:guid`, if any.
    pub guid: Option<String>,
    /// Its channel title.
    pub title: String,
    /// Identity keys of its items.
    pub keys: Vec<String>,
    /// Validators and fingerprint of the fetch that produced it.
    pub fetch: FetchStatus,
    /// HTTP summary of that fetch.
    pub http: HttpSummary,
}

/// The URL a feed announces as its new home, if any.
#[must_use]
pub fn announcement(
    source: &PodcastSource,
    response: &Response,
    new_feed_url: Option<&Url>,
) -> Option<Announcement> {
    if response.permanent_redirect && response.url != source.feed_url {
        return Some(Announcement {
            url: response.url.clone(),
            via: ReplacementReason::Redirect,
            reason: format!(
                "permanent redirect ({} hop{})",
                response.redirects.len(),
                if response.redirects.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        });
    }
    if let Some(new) = new_feed_url
        && *new != source.feed_url
        && Some(new) != source.canonical_url.as_ref()
        && *new != response.url
    {
        return Some(Announcement {
            url: new.clone(),
            via: ReplacementReason::NewFeedUrl,
            reason: "itunes:new-feed-url".to_owned(),
        });
    }
    None
}

/// Identity keys and channel facts of a parsed feed.
#[must_use]
pub fn candidate_from_parsed(
    parsed: &ParsedFeed,
    url: &Url,
    fetch: FetchStatus,
    http: HttpSummary,
    now: OffsetDateTime,
) -> Candidate {
    let channel = normalize_channel(&parsed.channel);
    let normalized: Vec<_> = parsed
        .items
        .iter()
        .map(|i| normalize_item(i, uguisu_core::ids::EpisodeId::new(), now))
        .collect();
    let sigs: Vec<_> = parsed
        .items
        .iter()
        .zip(&normalized)
        .map(|(p, n)| signals(p, n))
        .collect();
    let keys = resolve_identities(&sigs)
        .into_iter()
        .map(|i| i.key)
        .collect();
    Candidate {
        url: url.clone(),
        guid: channel.podcast_guid,
        title: channel.title,
        keys,
        fetch,
        http,
    }
}

/// The same-show check. `stored_keys` are the identity keys of the
/// episodes currently stored (not detected as removed).
pub fn verify(
    stored: &Podcast,
    stored_keys: &[String],
    cand: &Candidate,
) -> Result<String, String> {
    match (&stored.podcast_guid, &cand.guid) {
        (Some(a), Some(b)) if a == b => return Ok(format!("same podcast:guid {a}")),
        (Some(a), Some(b)) => {
            return Err(format!("podcast:guid differs ({a} vs {b})"));
        }
        _ => {}
    }
    let (a, b) = (normalize_title(&stored.title), normalize_title(&cand.title));
    if a.is_empty() || a != b {
        return Err(format!(
            "title differs (`{}` vs `{}`) and no podcast:guid to compare",
            stored.title, cand.title
        ));
    }
    if stored_keys.is_empty() {
        return Ok("same title; no stored episodes to compare".to_owned());
    }
    let ours: HashSet<&str> = stored_keys.iter().map(String::as_str).collect();
    let shared = cand
        .keys
        .iter()
        .filter(|k| ours.contains(k.as_str()))
        .count();
    let basis = stored_keys.len().min(cand.keys.len()).max(1);
    if shared * 2 >= basis {
        Ok(format!(
            "same title; {shared} of {basis} comparable episodes present"
        ))
    } else {
        Err(format!(
            "same title but only {shared} of {basis} comparable episodes present"
        ))
    }
}

/// The source row that replaces the current one after a verified move.
#[must_use]
pub fn replacement_source(
    old: &PodcastSource,
    cand: &Candidate,
    now: OffsetDateTime,
) -> PodcastSource {
    PodcastSource {
        id: SourceId::new(),
        podcast_id: old.podcast_id,
        feed_url: cand.url.clone(),
        canonical_url: None,
        website_url: old.website_url.clone(),
        provider: old.provider.clone(),
        provider_ref: old.provider_ref.clone(),
        discovered_at: now,
        verified_at: Some(now),
        is_current: true,
        replaced_by_source_id: None,
        replacement_reason: None,
        fetch: cand.fetch.clone(),
        created_at: now,
        updated_at: now,
    }
}

/// The announced source after a check that did not verify `url`. The row
/// already recorded for `url` carries on and counts the failures; a row for
/// another URL is not continued.
#[must_use]
pub fn announced_source(
    current: &PodcastSource,
    previous: Option<&PodcastSource>,
    url: &Url,
    why: &Unverified,
    now: OffsetDateTime,
) -> PodcastSource {
    let previous = previous.filter(|p| p.feed_url == *url);
    let fetch = previous.map_or_else(FetchStatus::default, |p| p.fetch.clone());
    PodcastSource {
        id: previous.map_or_else(SourceId::new, |p| p.id),
        podcast_id: current.podcast_id,
        feed_url: url.clone(),
        canonical_url: None,
        website_url: current.website_url.clone(),
        provider: current.provider.clone(),
        provider_ref: current.provider_ref.clone(),
        discovered_at: previous.map_or(now, |p| p.discovered_at),
        verified_at: None,
        is_current: false,
        replaced_by_source_id: None,
        replacement_reason: None,
        fetch: FetchStatus {
            state: FetchState::Failed,
            last_attempt_at: Some(now),
            last_error_at: Some(now),
            consecutive_failures: fetch.consecutive_failures.saturating_add(1),
            last_http_status: why.http_status,
            last_error_kind: Some(why.kind),
            last_error_detail: Some(why.detail.clone()),
            ..fetch
        },
        created_at: previous.map_or(now, |p| p.created_at),
        updated_at: now,
    }
}

/// A feed that cannot be moved to, as the error `podcast add` would give.
fn unusable(url: &Url, why: &Unverified) -> UguisuError {
    let detail = format!("{url}: {}", why.detail);
    match why.kind {
        FetchErrorKind::BlockedByPolicy => UguisuError::BlockedByPolicy(detail),
        kind if kind.is_content() => UguisuError::Feed { kind, detail },
        kind => UguisuError::Network { kind, detail },
    }
}

impl Engine {
    /// Moves podcast `id` to the feed at `url` (ADR 0052). The feed is
    /// fetched and checked first; the move happens only when the check
    /// passed or `force` says so, and never with `dry_run`. Episodes keep
    /// their ids; the refresh that follows a move is in the result.
    pub async fn move_feed(
        &self,
        id: PodcastId,
        url: &str,
        options: MoveOptions,
        cancel: CancellationToken,
    ) -> Result<FeedMove, UguisuError> {
        let url = usable_url(url.trim()).map_err(UguisuError::Invalid)?;
        let detail = self.podcast(id).await?;
        let source = detail
            .source
            .ok_or_else(|| UguisuError::Storage(format!("podcast {id} has no current source")))?;
        let unmoved = |to: Url, verified: bool, check: String| FeedMove {
            podcast_id: id,
            from: source.feed_url.clone(),
            to,
            verified,
            check,
            moved: false,
            report: None,
        };
        let already = || "already this podcast's feed".to_owned();
        if url == source.feed_url {
            return Ok(unmoved(url, true, already()));
        }
        self.refuse_other_feed(id, &url).await?;
        let cand = self
            .fetch_candidate(&url, cancel.clone())
            .await
            .map_err(|why| unusable(&url, &why))?;
        if cand.url == source.feed_url {
            return Ok(unmoved(cand.url, true, already()));
        }
        self.refuse_other_feed(id, &cand.url).await?;
        let stored_keys = {
            let mut conn = self.storage().reader().await?;
            if let Some(guid) = &cand.guid
                && let Some(other) = podcasts::find_by_guid(&mut conn, guid)
                    .await?
                    .into_iter()
                    .find(|p| p.id != id)
            {
                return Err(UguisuError::Conflict(format!(
                    "{} carries podcast:guid {guid}, which is podcast {} ({})",
                    cand.url, other.id, other.title
                )));
            }
            episodes::index(&mut conn, id)
                .await?
                .into_iter()
                .filter(|e| e.removed_from_feed_at.is_none())
                .map(|e| e.identity_key)
                .collect::<Vec<_>>()
        };
        let (verified, check) = match verify(&detail.podcast, &stored_keys, &cand) {
            Ok(why) => (true, why),
            Err(why) => (false, why),
        };
        if options.dry_run || !(verified || options.force) {
            return Ok(unmoved(cand.url, verified, check));
        }

        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        // A refresh may have moved the podcast while the feed was checked.
        if sources::current(&mut tx, id).await?.map(|s| s.id) != Some(source.id) {
            return Err(UguisuError::Conflict(format!(
                "the feed of podcast {id} changed while {} was checked",
                cand.url
            )));
        }
        let new = replacement_source(&source, &cand, now);
        sources::clear_announced(&mut tx, id).await?;
        sources::replace_current(&mut tx, source.id, &new, ReplacementReason::Manual, now).await?;
        let event = Event::now(
            Some(id),
            None,
            EventKind::FeedUrlChanged {
                from_source_id: source.id,
                to_source_id: new.id,
                from: source.feed_url.clone(),
                to: new.feed_url.clone(),
                via: ReplacementReason::Manual,
            },
        );
        events::insert_all(&mut tx, std::slice::from_ref(&event)).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(std::slice::from_ref(&event));

        let report = self
            .refresh_podcast(
                id,
                RefreshOptions {
                    force: false,
                    cancel: Some(cancel),
                },
            )
            .await?;
        Ok(FeedMove {
            moved: true,
            report: Some(report),
            ..unmoved(new.feed_url, verified, check)
        })
    }

    /// Refuses a URL that is another podcast's current feed: joining two
    /// podcasts is not a move.
    async fn refuse_other_feed(&self, id: PodcastId, url: &Url) -> Result<(), UguisuError> {
        let mut conn = self.storage().reader().await?;
        if let Some(other) = sources::find_current_by_urls(&mut conn, &[url.to_string()])
            .await?
            .into_iter()
            .find(|s| s.podcast_id != id)
        {
            return Err(UguisuError::Conflict(format!(
                "{url} is the feed of podcast {}",
                other.podcast_id
            )));
        }
        Ok(())
    }

    /// Fetches and parses an announced feed for verification.
    pub(crate) async fn fetch_candidate(
        &self,
        url: &Url,
        cancel: CancellationToken,
    ) -> Result<Candidate, Unverified> {
        let get = GetOptions {
            max_bytes: Some(self.config().feed.limits.max_bytes),
            cancel: Some(cancel),
            ..GetOptions::default()
        };
        let response = self
            .feed_client()
            .get_with(url, &get)
            .await
            .map_err(|e| Unverified {
                kind: error_kind(&e),
                detail: e.to_string(),
                http_status: None,
            })?;
        let status = Some(response.status.as_u16());
        let failed = |kind, detail: String| Unverified {
            kind,
            detail,
            http_status: status,
        };
        if !response.status.is_success() {
            let kind = FetchErrorKind::from_status(response.status.as_u16())
                .unwrap_or(FetchErrorKind::HttpClientError);
            return Err(failed(
                kind,
                format!("http status {}", response.status.as_u16()),
            ));
        }
        if !looks_like_xml(&response.body) {
            return Err(failed(
                FetchErrorKind::InvalidContentType,
                "body is not XML".to_owned(),
            ));
        }
        let parsed = parse(&response.body, &self.config().feed.limits)
            .map_err(|e| failed(parse_error_kind(&e), e.to_string()))?;
        if !parsed.looks_like_podcast() {
            return Err(failed(
                FetchErrorKind::InvalidPodcastFeed,
                "no item carries an enclosure".to_owned(),
            ));
        }
        let now = OffsetDateTime::now_utc();
        // No validators and no fingerprint: the verification fetch only
        // checked the show, it did not sync its items, so the next refresh
        // of the new source must parse the body in full.
        let fetch = FetchStatus {
            state: FetchState::Fetched,
            last_attempt_at: Some(now),
            last_success_at: Some(now),
            last_content_length: Some(response.body.len() as u64),
            last_http_status: Some(response.status.as_u16()),
            ..FetchStatus::default()
        };
        let http = HttpSummary {
            status: Some(response.status.as_u16()),
            final_url: Some(response.url.clone()),
            redirects: u32::try_from(response.redirects.len()).unwrap_or(u32::MAX),
            etag_changed: false,
            etag: fetch.etag.clone(),
            last_modified: fetch.last_modified.clone(),
            bytes: Some(response.body.len() as u64),
            conditional: false,
        };
        Ok(candidate_from_parsed(
            &parsed,
            &response.url,
            fetch,
            http,
            now,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn podcast(title: &str, guid: Option<&str>) -> Podcast {
        let now = OffsetDateTime::UNIX_EPOCH;
        Podcast {
            id: uguisu_core::ids::PodcastId::new(),
            title: title.to_owned(),
            sort_title: title.to_owned(),
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
            podcast_guid: guid.map(str::to_owned),
            feed_kind: uguisu_core::model::FeedKind::Rss2,
            status: uguisu_core::model::PodcastStatus::Active,
            refresh_interval_secs: None,
            next_refresh_at: None,
            last_refresh_at: None,
            last_error: None,
            directory_name: None,
            metadata_hash: String::new(),
            created_at: now,
            updated_at: now,
        }
    }

    fn cand(title: &str, guid: Option<&str>, keys: &[&str]) -> Candidate {
        Candidate {
            url: Url::parse("https://new.example/feed.xml").unwrap_or_else(|_| unreachable!()),
            guid: guid.map(str::to_owned),
            title: title.to_owned(),
            keys: keys.iter().map(|k| (*k).to_owned()).collect(),
            fetch: FetchStatus::default(),
            http: HttpSummary::default(),
        }
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn guid_decides_when_both_sides_have_one() {
        let p = podcast("Show", Some("g1"));
        assert!(verify(&p, &keys(&["a"]), &cand("Other", Some("g1"), &[])).is_ok());
        assert!(verify(&p, &keys(&["a"]), &cand("Show", Some("g2"), &["a"])).is_err());
    }

    #[test]
    fn title_and_overlap_decide_otherwise() {
        let p = podcast("The Show", None);
        assert!(verify(&p, &[], &cand("the show", Some("g"), &[])).is_ok());
        assert!(
            verify(
                &p,
                &keys(&["a", "b", "c", "d"]),
                &cand("The Show", None, &["a", "b"])
            )
            .is_ok()
        );
        assert!(
            verify(
                &p,
                &keys(&["a", "b", "c", "d"]),
                &cand("The Show", None, &["a", "x", "y", "z"])
            )
            .is_err()
        );
        assert!(verify(&p, &keys(&["a"]), &cand("Different", None, &["a"])).is_err());
        // A host that lists fewer items than we store is compared on its own size.
        assert!(
            verify(
                &p,
                &keys(&["a", "b", "c", "d", "e", "f"]),
                &cand("The Show", None, &["a", "b"])
            )
            .is_ok()
        );
    }
}
