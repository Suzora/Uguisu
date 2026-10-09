//! OPML import and export (ADR 0049).
//!
//! An import plans every outline from the document and the library alone,
//! without a request. Applying it verifies each new feed the way
//! `podcast add` does and stores it in its own transaction, so an import
//! cut short keeps what it added and a second run continues. Nothing is
//! refreshed: the scheduler fetches never-fetched podcasts first.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchivePolicy, PolicyMode};
use uguisu_core::download::Priority;
use uguisu_core::ids::PodcastId;
use uguisu_discovery::dedup::feed_key;
use uguisu_discovery::resolve::{ResolvedFeed, StepKind};
use uguisu_feed::opml::{self, Outline};
use uguisu_http::CancellationToken;
use uguisu_storage::sources;
use url::Url;

use crate::Engine;

pub use uguisu_feed::opml::MAX_BYTES;

/// Longest feed URL an import accepts.
const MAX_URL_BYTES: usize = 4096;

/// The policy an import stores with every podcast it adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDefaults {
    /// Manual or automatic.
    pub mode: PolicyMode,
    /// Episodes that may await archiving at once; `None` is the global default.
    pub max_backlog: Option<u32>,
    /// Episodes older than this are left alone; `None` is the global default.
    pub max_age_days: Option<u32>,
    /// Priority of the jobs the policy creates; `None` is the global default.
    pub priority: Option<Priority>,
}

impl PolicyDefaults {
    pub(crate) const fn for_podcast(
        self,
        podcast_id: PodcastId,
        now: OffsetDateTime,
    ) -> ArchivePolicy {
        ArchivePolicy {
            podcast_id,
            mode: self.mode,
            max_backlog: self.max_backlog,
            max_age_days: self.max_age_days,
            priority: self.priority,
            updated_at: now,
        }
    }
}

/// What an import does beyond planning.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpmlOptions {
    /// Verify and add the new feeds; without it nothing is fetched or stored.
    pub apply: bool,
    /// Stored with each podcast the import adds.
    pub policy: Option<PolicyDefaults>,
}

/// What became, or would become, of one outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpmlAction {
    /// A new feed: added, or to be added when applied.
    Add,
    /// A podcast already has this feed, now or as a former source.
    AlreadyPresent,
    /// The same feed appeared earlier in the document.
    Duplicate,
    /// Not an `http` or `https` URL with a host.
    Invalid,
    /// The URL led to a page that points at a feed, not to a feed.
    NeedsReview,
    /// Another podcast already carries this feed's `podcast:guid`.
    Conflict,
    /// The feed could not be verified or stored.
    Failed,
}

impl OpmlAction {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::AlreadyPresent => "already_present",
            Self::Duplicate => "duplicate",
            Self::Invalid => "invalid",
            Self::NeedsReview => "needs_review",
            Self::Conflict => "conflict",
            Self::Failed => "failed",
        }
    }
}

/// One feed outline of the document and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpmlItem {
    /// The outline's title, else its text.
    pub title: Option<String>,
    /// The outline's `xmlUrl`, as written.
    pub xml_url: String,
    /// What became of it.
    pub action: OpmlAction,
    /// The podcast it is, or became.
    pub podcast_id: Option<PodcastId>,
    /// Why, for anything but `add` and `already_present`.
    pub detail: Option<String>,
}

/// How many outlines ended in each action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpmlCounts {
    /// New feeds, added or to be added.
    pub add: u64,
    /// Feeds the library already has.
    pub already_present: u64,
    /// Repeats within the document.
    pub duplicate: u64,
    /// Unusable URLs.
    pub invalid: u64,
    /// Pages rather than feeds.
    pub needs_review: u64,
    /// Second feeds of a show already in the library.
    pub conflict: u64,
    /// Feeds that could not be verified or stored.
    pub failed: u64,
}

/// The plan of an import, or what applying it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpmlImport {
    /// Whether the new feeds were added.
    pub applied: bool,
    /// Outlines per action.
    pub counts: OpmlCounts,
    /// Every feed outline, in document order.
    pub items: Vec<OpmlItem>,
}

type Outcome = (OpmlAction, Option<PodcastId>, Option<String>);

impl Engine {
    /// Plans an OPML import and, with [`OpmlOptions::apply`], adds the new
    /// feeds, at most `UGUISU_FEED_REFRESH_CONCURRENCY` resolving at once.
    ///
    /// A document that is not OPML is [`UguisuError::Invalid`]; a feed that
    /// fails is reported in its item and stops nothing else.
    pub async fn import_opml(
        &self,
        text: &str,
        options: OpmlOptions,
        cancel: CancellationToken,
    ) -> Result<OpmlImport, UguisuError> {
        let outlines = opml::parse(text).map_err(|e| UguisuError::Invalid(e.to_string()))?;
        let mut known: HashMap<String, PodcastId> = {
            let mut reader = self.storage().reader().await?;
            sources::known_urls(&mut reader)
                .await?
                .into_iter()
                .map(|(id, url)| (feed_key(&url), id))
                .collect()
        };

        let mut seen = HashSet::new();
        let mut pending = Vec::new();
        let mut items: Vec<OpmlItem> = outlines
            .into_iter()
            .enumerate()
            .map(|(index, outline)| {
                let (action, podcast_id, detail) = match usable_url(&outline.xml_url) {
                    Err(why) => (OpmlAction::Invalid, None, Some(why)),
                    Ok(url) => {
                        let key = feed_key(&url);
                        if !seen.insert(key.clone()) {
                            (OpmlAction::Duplicate, known.get(&key).copied(), None)
                        } else if let Some(id) = known.get(&key) {
                            (OpmlAction::AlreadyPresent, Some(*id), None)
                        } else {
                            pending.push((index, url));
                            (OpmlAction::Add, None, None)
                        }
                    }
                };
                OpmlItem {
                    title: outline.title,
                    xml_url: outline.xml_url,
                    action,
                    podcast_id,
                    detail,
                }
            })
            .collect();

        if options.apply {
            // Resolution runs in parallel; the check against the library and
            // the insert hold this lock together, so two spellings of one
            // feed resolving at the same moment still add one podcast.
            let known = Arc::new(Mutex::new(std::mem::take(&mut known)));
            let limit = Arc::new(Semaphore::new(
                self.config().feed.refresh_concurrency.max(1),
            ));
            let mut set = JoinSet::new();
            for (index, url) in pending {
                let engine = self.clone();
                let known = Arc::clone(&known);
                let limit = Arc::clone(&limit);
                let cancel = cancel.clone();
                set.spawn(async move {
                    let _permit = limit.acquire_owned().await;
                    let outcome = if cancel.is_cancelled() {
                        (OpmlAction::Failed, None, Some("cancelled".to_owned()))
                    } else {
                        engine
                            .import_one(&url, &known, options.policy, cancel)
                            .await
                    };
                    (index, outcome)
                });
            }
            while let Some(joined) = set.join_next().await {
                let (index, (action, podcast_id, detail)) = joined
                    .map_err(|e| UguisuError::Internal(format!("opml import task failed: {e}")))?;
                let item = &mut items[index];
                item.action = action;
                item.podcast_id = podcast_id;
                item.detail = detail;
            }
        }

        let mut counts = OpmlCounts::default();
        for item in &items {
            *match item.action {
                OpmlAction::Add => &mut counts.add,
                OpmlAction::AlreadyPresent => &mut counts.already_present,
                OpmlAction::Duplicate => &mut counts.duplicate,
                OpmlAction::Invalid => &mut counts.invalid,
                OpmlAction::NeedsReview => &mut counts.needs_review,
                OpmlAction::Conflict => &mut counts.conflict,
                OpmlAction::Failed => &mut counts.failed,
            } += 1;
        }
        if options.apply {
            tracing::info!(
                outlines = items.len(),
                added = counts.add,
                already_present = counts.already_present,
                needs_review = counts.needs_review,
                conflict = counts.conflict,
                failed = counts.failed,
                "opml import applied"
            );
        }
        Ok(OpmlImport {
            applied: options.apply,
            counts,
            items,
        })
    }

    async fn import_one(
        &self,
        url: &Url,
        known: &Mutex<HashMap<String, PodcastId>>,
        policy: Option<PolicyDefaults>,
        cancel: CancellationToken,
    ) -> Outcome {
        let (resolved, record) = match self.resolve_input_recorded(url.as_str(), cancel).await {
            Ok(r) => r,
            Err(e) => return (OpmlAction::Failed, None, Some(e.to_string())),
        };
        if !served_directly(&resolved) {
            return (
                OpmlAction::NeedsReview,
                None,
                Some(format!(
                    "this is a page, not a feed; it points to {}",
                    resolved.feed_url
                )),
            );
        }
        let keys: Vec<String> = [
            Some(&resolved.feed_url),
            resolved.canonical_url.as_ref(),
            resolved.moved_to.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(feed_key)
        .collect();
        let mut known = known.lock().await;
        if let Some(id) = keys.iter().find_map(|k| known.get(k)) {
            return (OpmlAction::AlreadyPresent, Some(*id), None);
        }
        let outcome = match self.insert_resolved(resolved, "opml", policy).await {
            Ok(outcome) => outcome,
            Err(UguisuError::Conflict(why)) => return (OpmlAction::Conflict, None, Some(why)),
            Err(e) => return (OpmlAction::Failed, None, Some(e.to_string())),
        };
        let id = outcome.podcast.id;
        for key in keys {
            known.insert(key, id);
        }
        drop(known);
        self.attach_resolution(record, id).await;
        let action = if outcome.created {
            OpmlAction::Add
        } else {
            OpmlAction::AlreadyPresent
        };
        (action, Some(id), None)
    }

    /// Every podcast, whatever its status, as a flat OPML 2.0 list sorted
    /// by title. A podcast without a current source has no feed to list.
    pub async fn export_opml(&self) -> Result<String, UguisuError> {
        let outlines: Vec<Outline> = self
            .list_podcasts()
            .await?
            .into_iter()
            .filter_map(|detail| {
                let source = detail.source?;
                Some(Outline {
                    title: Some(detail.podcast.title),
                    xml_url: source.feed_url.to_string(),
                    html_url: detail.podcast.website.map(|u| u.to_string()),
                })
            })
            .collect();
        Ok(opml::write("Uguisu subscriptions", &outlines))
    }
}

pub(crate) fn usable_url(raw: &str) -> Result<Url, String> {
    if raw.len() > MAX_URL_BYTES {
        return Err(format!("the URL is longer than {MAX_URL_BYTES} bytes"));
    }
    let url = Url::parse(raw).map_err(|e| format!("not a URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{}: is not http or https", url.scheme()));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".to_owned());
    }
    Ok(url)
}

/// Whether the URL served the feed itself, rather than a page or a
/// directory entry that pointed at one. A redirect or the HTTPS upgrade is
/// still the feed itself.
fn served_directly(feed: &ResolvedFeed) -> bool {
    !feed.provenance.iter().any(|step| {
        matches!(
            step.kind,
            StepKind::Autodiscovery
                | StepKind::PlatformPattern
                | StepKind::WellKnownPath
                | StepKind::ProviderLookup
        )
    })
}
