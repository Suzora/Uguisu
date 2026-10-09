//! Synchronisation planning: a parsed feed plus the stored episode index
//! become a plan of inserts, updates, unchanged rows and removal
//! candidates. This module is pure (no I/O) so the rules of ADR 0014
//! (identity fallback and GUID changes) and ADR 0017 (conservative
//! removal) can be tested without a database.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uguisu_core::feed::EpisodeCounts;
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_core::model::{
    ArchiveState, DateQuality, Episode, EpisodeIdentity, IdentitySource, Podcast,
};
use uguisu_feed::identity::{
    MatchFacts, comparable_hash, normalize_guid, probable_same_episode, resolve_identities, signals,
};
use uguisu_feed::normalize::{
    NormalizedChannel, NormalizedItem, normalize_channel, normalize_item,
};
use uguisu_feed::{ParsedFeed, ParsedItem};
use uguisu_storage::episodes::EpisodeIndex;
use url::Url;

/// Removal rules (from `FeedConfig`).
#[derive(Debug, Clone, Copy)]
pub struct SyncRules {
    /// Complete fetches without an episode before it counts as removed.
    pub removal_streak: u32,
    /// Percentage of present episodes that may go missing in one fetch
    /// before removal detection is suppressed for that fetch.
    pub mass_removal_guard_percent: u8,
}

/// A stored episode the incoming item was matched to by secondary
/// signals (ADR 0014): the stored identity key is kept and the change is
/// logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityNote {
    /// Episode whose identity signals changed.
    pub episode_id: EpisodeId,
    /// The identity the item would have had on its own.
    pub incoming_key: String,
    /// Why it was matched anyway.
    pub reason: String,
}

/// A new episode that is probably a duplicate of a stored one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguity {
    /// The new (skipped) episode.
    pub episode_id: EpisodeId,
    /// The stored episode it probably duplicates.
    pub duplicate_of: EpisodeId,
    /// Match reasons.
    pub reasons: Vec<String>,
}

/// Everything the refresh transaction has to write.
#[derive(Debug, Clone)]
pub struct SyncPlan {
    /// The podcast row with channel metadata applied.
    pub podcast: Podcast,
    /// Podcast fields that changed (empty when the metadata hash is equal).
    pub podcast_changed_fields: Vec<String>,
    /// Rows to insert or update (added, updated, duplicate candidates,
    /// malformed placeholders).
    pub upserts: Vec<Episode>,
    /// Ids of `upserts` that are new.
    pub added: Vec<EpisodeId>,
    /// Ids of `upserts` that already existed and changed.
    pub updated: Vec<EpisodeId>,
    /// Existing episodes seen again without a change.
    pub unchanged: Vec<EpisodeId>,
    /// Identity matches by secondary signals.
    pub identity_notes: Vec<IdentityNote>,
    /// Probable duplicates surfaced as new skipped episodes.
    pub ambiguous: Vec<Ambiguity>,
    /// Existing episodes absent from a complete fetch (streak + 1).
    pub missing: Vec<EpisodeId>,
    /// Episodes whose streak now reaches the threshold, with the new streak.
    pub removed: Vec<(EpisodeId, u32)>,
    /// Why removal detection was skipped this time, if it was.
    pub removal_suppressed: Option<String>,
    /// Warnings for the report and the fetch log.
    pub warnings: Vec<String>,
    /// Counters for the report.
    pub counts: EpisodeCounts,
    /// `itunes:new-feed-url`, when the channel announces one.
    pub new_feed_url: Option<Url>,
    /// Identity keys of every item in this fetch.
    pub identity_keys: Vec<String>,
    /// The channel's own `podcast:guid` (not the stored fallback).
    pub channel_guid: Option<String>,
    /// The channel title.
    pub channel_title: String,
}

/// Field names of a podcast that the metadata hash covers.
const PODCAST_FIELDS: [&str; 17] = [
    "title",
    "subtitle",
    "author",
    "publisher",
    "owner_name",
    "owner_email",
    "description_html",
    "description_text",
    "website",
    "artwork_url",
    "language",
    "categories",
    "explicit",
    "copyright",
    "podcast_guid",
    "feed_kind",
    "locked",
];

fn sha256_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

fn opt(s: Option<&str>) -> String {
    s.unwrap_or("").to_owned()
}

/// Applies channel metadata to the podcast and reports what changed.
#[allow(clippy::too_many_lines)] // a field table, one row per column
fn apply_channel(
    podcast: &Podcast,
    ch: &NormalizedChannel,
    kind: uguisu_core::model::FeedKind,
    now: OffsetDateTime,
) -> (Podcast, Vec<String>) {
    let mut p = podcast.clone();
    // A channel that lost its guid keeps the stored one: the guid is the
    // podcast's identity, not a mutable field.
    let podcast_guid = ch
        .podcast_guid
        .clone()
        .or_else(|| podcast.podcast_guid.clone());
    let values: Vec<(&str, String, String)> = vec![
        ("title", podcast.title.clone(), ch.title.clone()),
        (
            "subtitle",
            opt(podcast.subtitle.as_deref()),
            opt(ch.subtitle.as_deref()),
        ),
        (
            "author",
            opt(podcast.author.as_deref()),
            opt(ch.author.as_deref()),
        ),
        (
            "publisher",
            opt(podcast.publisher.as_deref()),
            opt(ch.publisher.as_deref()),
        ),
        (
            "owner_name",
            opt(podcast.owner_name.as_deref()),
            opt(ch.owner_name.as_deref()),
        ),
        (
            "owner_email",
            opt(podcast.owner_email.as_deref()),
            opt(ch.owner_email.as_deref()),
        ),
        (
            "description_html",
            opt(podcast.description_html.as_deref()),
            opt(ch.description_html.as_deref()),
        ),
        (
            "description_text",
            opt(podcast.description_text.as_deref()),
            opt(ch.description_text.as_deref()),
        ),
        (
            "website",
            opt(podcast.website.as_ref().map(Url::as_str)),
            opt(ch.website.as_ref().map(Url::as_str)),
        ),
        (
            "artwork_url",
            opt(podcast.artwork_url.as_ref().map(Url::as_str)),
            opt(ch.artwork_url.as_ref().map(Url::as_str)),
        ),
        (
            "language",
            opt(podcast.language.as_deref()),
            opt(ch.language.as_deref()),
        ),
        (
            "categories",
            podcast.categories.join("\n"),
            ch.categories.join("\n"),
        ),
        (
            "explicit",
            podcast.explicit.map_or_else(String::new, |b| b.to_string()),
            ch.explicit.map_or_else(String::new, |b| b.to_string()),
        ),
        (
            "copyright",
            opt(podcast.copyright.as_deref()),
            opt(ch.copyright.as_deref()),
        ),
        (
            "podcast_guid",
            opt(podcast.podcast_guid.as_deref()),
            opt(podcast_guid.as_deref()),
        ),
        (
            "feed_kind",
            podcast.feed_kind.as_str().to_owned(),
            kind.as_str().to_owned(),
        ),
        (
            "locked",
            String::new(),
            ch.locked.map_or_else(String::new, |b| b.to_string()),
        ),
    ];
    debug_assert_eq!(values.len(), PODCAST_FIELDS.len());
    let new_values: Vec<&str> = values.iter().map(|(_, _, n)| n.as_str()).collect();
    let hash = sha256_hex(&new_values);
    let changed: Vec<String> = if hash == podcast.metadata_hash {
        Vec::new()
    } else {
        values
            .iter()
            .filter(|(name, old, new)| old != new && *name != "locked")
            .map(|(name, _, _)| (*name).to_owned())
            .collect()
    };
    p.title.clone_from(&ch.title);
    p.sort_title.clone_from(&ch.sort_title);
    p.subtitle.clone_from(&ch.subtitle);
    p.author.clone_from(&ch.author);
    p.publisher.clone_from(&ch.publisher);
    p.owner_name.clone_from(&ch.owner_name);
    p.owner_email.clone_from(&ch.owner_email);
    p.description_html.clone_from(&ch.description_html);
    p.description_text.clone_from(&ch.description_text);
    p.website.clone_from(&ch.website);
    p.artwork_url.clone_from(&ch.artwork_url);
    p.language.clone_from(&ch.language);
    p.categories.clone_from(&ch.categories);
    p.explicit = ch.explicit;
    p.copyright.clone_from(&ch.copyright);
    p.podcast_guid = podcast_guid;
    p.feed_kind = kind;
    p.metadata_hash = hash;
    p.updated_at = now;
    (p, changed)
}

/// Builds the episode row for one normalized item.
#[allow(clippy::too_many_arguments)]
fn episode_row(
    id: EpisodeId,
    podcast_id: PodcastId,
    parsed: &ParsedItem,
    n: &NormalizedItem,
    identity: EpisodeIdentity,
    first_seen_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Episode {
    let mut enclosures = n.enclosures.clone();
    for e in &mut enclosures {
        e.episode_id = id;
    }
    let published_at = n.published.value;
    Episode {
        id,
        podcast_id,
        guid: parsed
            .guid
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .map(str::to_owned),
        guid_is_permalink: parsed.guid_is_permalink,
        identity,
        title: n.title.clone(),
        subtitle: n.subtitle.clone(),
        sort_title: n.sort_title.clone(),
        description_html: n.description_html.clone(),
        description_text: n.description_text.clone(),
        link: n.link.clone(),
        published_at,
        published_at_raw: (!n.published.raw.is_empty()).then(|| n.published.raw.clone()),
        published_at_quality: n.published.quality,
        updated_at_source: n.updated_at_source,
        duration_secs: n.duration_secs,
        duration_raw: n.duration_raw.clone(),
        season: n.season,
        episode_number: n.episode_number,
        episode_type: n.episode_type.clone(),
        explicit: n.explicit,
        artwork_url: n.artwork_url.clone(),
        author: n.author.clone(),
        content_hash: comparable_hash(n),
        archive_state: ArchiveState::Expected,
        skip_reason: None,
        malformed: false,
        malformed_reason: None,
        duplicate_of_episode_id: None,
        duplicate_reasons: Vec::new(),
        missing_streak: 0,
        first_seen_at,
        last_seen_in_feed_at: now,
        removed_from_feed_at: None,
        sort_at: published_at.unwrap_or(first_seen_at),
        source_metadata: None,
        enclosures,
        extras: n.extras.clone(),
        created_at: now,
        updated_at: now,
    }
}

/// Where an incoming item lands after the stored index is consulted.
enum Placement {
    /// Same identity key as a stored episode.
    Existing(usize),
    /// Matched a stored episode by secondary signals without contradiction.
    Matched(usize, String),
    /// Contradicts a stored episode that shares strong signals.
    Duplicate(usize),
    /// Nothing stored looks like it.
    New,
}

struct Lookup<'a> {
    index: &'a [EpisodeIndex],
    by_key: HashMap<&'a str, usize>,
    by_guid: HashMap<&'a str, Vec<usize>>,
    by_url: HashMap<&'a str, Vec<usize>>,
    by_fp: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Lookup<'a> {
    fn new(index: &'a [EpisodeIndex]) -> Self {
        let mut by_key = HashMap::with_capacity(index.len());
        let mut by_guid: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut by_url: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut by_fp: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, e) in index.iter().enumerate() {
            by_key.insert(e.identity_key.as_str(), i);
            if let Some(g) = &e.guid_key {
                by_guid.entry(g.as_str()).or_default().push(i);
            }
            if let Some(u) = &e.enclosure_key {
                by_url.entry(u.as_str()).or_default().push(i);
            }
            if let Some(f) = &e.fingerprint_key {
                by_fp.entry(f.as_str()).or_default().push(i);
            }
        }
        Self {
            index,
            by_key,
            by_guid,
            by_url,
            by_fp,
        }
    }

    fn place(&self, identity: &EpisodeIdentity, claimed: &HashSet<usize>) -> Placement {
        if let Some(&i) = self.by_key.get(identity.key.as_str()) {
            return if claimed.contains(&i) {
                Placement::New
            } else {
                Placement::Existing(i)
            };
        }
        // Candidates in order of strength: guid, enclosure url, fingerprint.
        let mut candidates: Vec<(usize, &'static str)> = Vec::new();
        for (key, map, why) in [
            (&identity.guid_key, &self.by_guid, "same guid"),
            (&identity.enclosure_key, &self.by_url, "same enclosure url"),
            (&identity.fingerprint_key, &self.by_fp, "same fingerprint"),
        ] {
            if let Some(k) = key
                && let Some(list) = map.get(k.as_str())
            {
                for &i in list {
                    if !claimed.contains(&i) && !candidates.iter().any(|(c, _)| *c == i) {
                        candidates.push((i, why));
                    }
                }
            }
        }
        let Some(&(i, why)) = candidates.first() else {
            return Placement::New;
        };
        let stored = &self.index[i];
        // Contradiction (ADR 0014): both sides carry unique, different
        // GUIDs, or both identities are enclosure-based on different URLs
        // and only the fingerprint agrees.
        let guids_differ = matches!(
            (&stored.guid_key, &identity.guid_key),
            (Some(a), Some(b)) if a != b
        );
        let urls_differ_fp_only = stored.identity_source == IdentitySource::EnclosureUrl
            && identity.source == IdentitySource::EnclosureUrl
            && stored.enclosure_key != identity.enclosure_key
            && why == "same fingerprint";
        if guids_differ || urls_differ_fp_only {
            Placement::Duplicate(i)
        } else {
            let reason = match (&stored.guid_key, &identity.guid_key) {
                (None, Some(_)) => format!("{why}; item gained a guid"),
                (Some(_), None) => format!("{why}; item lost its guid"),
                _ => why.to_owned(),
            };
            Placement::Matched(i, reason)
        }
    }
}

fn facts_from_index(e: &EpisodeIndex) -> MatchFacts {
    MatchFacts {
        enclosure_key: e.enclosure_key.clone(),
        fingerprint_key: e.fingerprint_key.clone(),
        title: e.title.clone(),
        published_at: e.published_at,
        enclosure_length: None,
    }
}

fn facts_from_item(identity: &EpisodeIdentity, n: &NormalizedItem) -> MatchFacts {
    MatchFacts {
        enclosure_key: identity.enclosure_key.clone(),
        fingerprint_key: identity.fingerprint_key.clone(),
        title: n.title.clone(),
        published_at: n.published.value,
        enclosure_length: n
            .enclosures
            .iter()
            .find(|e| e.is_primary)
            .and_then(|e| e.length_bytes),
    }
}

/// Builds the plan. `complete` says whether the fetch saw the whole feed
/// (not truncated, no malformed item) and removal detection may run.
#[must_use]
#[allow(clippy::too_many_lines)] // one pass over the items, then removal
pub fn plan(
    podcast: &Podcast,
    parsed: &ParsedFeed,
    index: &[EpisodeIndex],
    rules: SyncRules,
    now: OffsetDateTime,
) -> SyncPlan {
    let channel = normalize_channel(&parsed.channel);
    let (podcast_row, podcast_changed_fields) = apply_channel(podcast, &channel, parsed.kind, now);
    let mut warnings: Vec<String> = parsed.warnings.clone();
    warnings.extend(channel.warnings.iter().cloned());

    // Normalize every item once; identities need the whole feed.
    let normalized: Vec<NormalizedItem> = parsed
        .items
        .iter()
        .map(|item| normalize_item(item, EpisodeId::new(), now))
        .collect();
    let sigs: Vec<_> = parsed
        .items
        .iter()
        .zip(&normalized)
        .map(|(p, n)| signals(p, n))
        .collect();
    let identities = resolve_identities(&sigs);

    let lookup = Lookup::new(index);
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut claimed_keys: HashSet<String> = HashSet::new();
    let mut plan = SyncPlan {
        podcast: podcast_row,
        podcast_changed_fields,
        upserts: Vec::new(),
        added: Vec::new(),
        updated: Vec::new(),
        unchanged: Vec::new(),
        identity_notes: Vec::new(),
        ambiguous: Vec::new(),
        missing: Vec::new(),
        removed: Vec::new(),
        removal_suppressed: None,
        warnings: Vec::new(),
        counts: EpisodeCounts::default(),
        new_feed_url: channel.new_feed_url.clone(),
        identity_keys: Vec::new(),
        channel_guid: channel.podcast_guid.clone(),
        channel_title: channel.title.clone(),
    };
    plan.counts.seen = u32::try_from(parsed.items.len()).unwrap_or(u32::MAX);

    for ((item, n), identity) in parsed.items.iter().zip(&normalized).zip(identities) {
        for w in &n.warnings {
            warnings.push(format!("item {}: {w}", item.index));
        }
        if !claimed_keys.insert(identity.key.clone()) {
            warnings.push(format!(
                "item {}: identical to an earlier item in this feed ({}); skipped",
                item.index, identity.key
            ));
            continue;
        }
        match lookup.place(&identity, &claimed) {
            Placement::Existing(i) => {
                claimed.insert(i);
                let stored = &index[i];
                let hash = comparable_hash(n);
                if stored.content_hash == hash && !stored.malformed {
                    plan.unchanged.push(stored.id);
                    plan.counts.unchanged += 1;
                } else {
                    let row = episode_row(
                        stored.id,
                        podcast.id,
                        item,
                        n,
                        identity,
                        stored.first_seen_at,
                        now,
                    );
                    plan.updated.push(stored.id);
                    plan.upserts.push(row);
                    plan.counts.updated += 1;
                }
            }
            Placement::Matched(i, reason) => {
                claimed.insert(i);
                let stored = &index[i];
                let incoming_key = identity.key.clone();
                // Signals already recorded on an earlier refresh: the item
                // is a plain unchanged/updated episode, not a new match.
                let same_signals = stored.guid_key == identity.guid_key
                    && stored.enclosure_key == identity.enclosure_key
                    && stored.fingerprint_key == identity.fingerprint_key;
                if same_signals {
                    let hash = comparable_hash(n);
                    if stored.content_hash == hash && !stored.malformed {
                        plan.unchanged.push(stored.id);
                        plan.counts.unchanged += 1;
                        continue;
                    }
                    let kept = EpisodeIdentity {
                        key: stored.identity_key.clone(),
                        source: stored.identity_source,
                        guid_key: identity.guid_key.clone(),
                        enclosure_key: identity.enclosure_key.clone(),
                        fingerprint_key: identity.fingerprint_key.clone(),
                        reason: format!("kept stored identity: {reason}"),
                    };
                    let row = episode_row(
                        stored.id,
                        podcast.id,
                        item,
                        n,
                        kept,
                        stored.first_seen_at,
                        now,
                    );
                    plan.updated.push(stored.id);
                    plan.upserts.push(row);
                    plan.counts.updated += 1;
                    continue;
                }
                // Keep the stored identity key; refresh the signal columns.
                let kept = EpisodeIdentity {
                    key: stored.identity_key.clone(),
                    source: stored.identity_source,
                    guid_key: identity.guid_key.clone(),
                    enclosure_key: identity.enclosure_key.clone(),
                    fingerprint_key: identity.fingerprint_key.clone(),
                    reason: format!("kept stored identity: {reason}"),
                };
                warnings.push(format!(
                    "item {}: matched stored episode {} by {reason} (incoming identity {incoming_key})",
                    item.index, stored.id
                ));
                plan.identity_notes.push(IdentityNote {
                    episode_id: stored.id,
                    incoming_key,
                    reason,
                });
                let row = episode_row(
                    stored.id,
                    podcast.id,
                    item,
                    n,
                    kept,
                    stored.first_seen_at,
                    now,
                );
                plan.updated.push(stored.id);
                plan.upserts.push(row);
                plan.counts.updated += 1;
            }
            Placement::Duplicate(i) => {
                let stored = &index[i];
                let reasons: Vec<String> = probable_same_episode(
                    &facts_from_item(&identity, n),
                    &facts_from_index(stored),
                )
                .iter()
                .map(|r| r.as_str().to_owned())
                .collect();
                let id = EpisodeId::new();
                let mut row = episode_row(id, podcast.id, item, n, identity, now, now);
                row.duplicate_of_episode_id = Some(stored.id);
                row.duplicate_reasons.clone_from(&reasons);
                row.archive_state = ArchiveState::Skipped;
                row.skip_reason = Some(format!("duplicate of {}", stored.id));
                warnings.push(format!(
                    "item {}: probably the same episode as {} ({}); stored as a skipped candidate",
                    item.index,
                    stored.id,
                    reasons.join(", ")
                ));
                plan.ambiguous.push(Ambiguity {
                    episode_id: id,
                    duplicate_of: stored.id,
                    reasons,
                });
                plan.added.push(id);
                plan.upserts.push(row);
                plan.counts.added += 1;
                plan.counts.ambiguous += 1;
            }
            Placement::New => {
                let id = EpisodeId::new();
                let row = episode_row(id, podcast.id, item, n, identity, now, now);
                plan.added.push(id);
                plan.upserts.push(row);
                plan.counts.added += 1;
            }
        }
    }

    // Malformed items: counted, and kept as placeholders when they carry a
    // guid (so the episode is not "new" forever once the host fixes it).
    for m in &parsed.malformed_items {
        plan.counts.malformed += 1;
        warnings.push(format!("item {}: malformed: {}", m.index, m.reason));
        let Some(guid) = m
            .partial_guid
            .as_deref()
            .map(normalize_guid)
            .filter(|g| !g.is_empty())
        else {
            continue;
        };
        let key = format!("guid:{guid}");
        if !claimed_keys.insert(key.clone()) {
            continue;
        }
        if let Some(&i) = lookup.by_key.get(key.as_str()) {
            // Known episode; leave its stored data alone, just keep it seen.
            claimed.insert(i);
            plan.unchanged.push(index[i].id);
            continue;
        }
        let id = EpisodeId::new();
        let title = m
            .partial_title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Malformed item {}", m.index));
        plan.upserts.push(Episode {
            id,
            podcast_id: podcast.id,
            guid: m.partial_guid.clone(),
            guid_is_permalink: None,
            identity: EpisodeIdentity {
                key,
                source: IdentitySource::Guid,
                guid_key: Some(guid),
                enclosure_key: None,
                fingerprint_key: None,
                reason: "guid of a malformed item".to_owned(),
            },
            sort_title: uguisu_core::model::sort_title(&title),
            title,
            subtitle: None,
            description_html: None,
            description_text: None,
            link: None,
            published_at: None,
            published_at_raw: None,
            published_at_quality: DateQuality::Invalid,
            updated_at_source: None,
            duration_secs: None,
            duration_raw: None,
            season: None,
            episode_number: None,
            episode_type: None,
            explicit: None,
            artwork_url: None,
            author: None,
            content_hash: String::new(),
            archive_state: ArchiveState::Skipped,
            skip_reason: Some("malformed item".to_owned()),
            malformed: true,
            malformed_reason: Some(m.reason.clone()),
            duplicate_of_episode_id: None,
            duplicate_reasons: Vec::new(),
            missing_streak: 0,
            first_seen_at: now,
            last_seen_in_feed_at: now,
            removed_from_feed_at: None,
            sort_at: now,
            source_metadata: None,
            enclosures: Vec::new(),
            extras: uguisu_core::model::EpisodeExtras::default(),
            created_at: now,
            updated_at: now,
        });
        plan.added.push(id);
    }

    // Removal detection (ADR 0017): only on a complete view of the feed.
    let complete = !parsed.truncated && parsed.malformed_items.is_empty();
    if complete {
        let present: Vec<usize> = index
            .iter()
            .enumerate()
            .filter(|(_, e)| e.removed_from_feed_at.is_none())
            .map(|(i, _)| i)
            .collect();
        let unseen: Vec<usize> = present
            .iter()
            .copied()
            .filter(|i| !claimed.contains(i))
            .collect();
        let guard = u64::from(rules.mass_removal_guard_percent);
        let exceeds_guard =
            !unseen.is_empty() && (unseen.len() as u64) * 100 > (present.len() as u64) * guard;
        if exceeds_guard {
            let reason = format!(
                "{} of {} present episodes are missing from this fetch (more than {}%); removal detection suppressed",
                unseen.len(),
                present.len(),
                rules.mass_removal_guard_percent
            );
            warnings.push(format!("removal_suppressed_mass_change: {reason}"));
            plan.removal_suppressed = Some(reason);
        } else {
            for i in unseen {
                let e = &index[i];
                plan.missing.push(e.id);
                let streak = e.missing_streak.saturating_add(1);
                if streak >= rules.removal_streak.max(1) {
                    plan.removed.push((e.id, streak));
                    plan.counts.removed_detected += 1;
                }
            }
        }
    } else {
        let why = if parsed.truncated {
            "feed was truncated"
        } else {
            "feed had malformed items"
        };
        plan.removal_suppressed = Some(format!("{why}; removal detection skipped"));
    }

    plan.warnings = warnings;
    plan.identity_keys = claimed_keys.into_iter().collect();
    plan
}

/// Fields compared when an existing episode changed, with their old and
/// new values (for `episode_changes` and `episode.updated`).
#[must_use]
pub fn diff_episode(old: &Episode, new: &Episode) -> Vec<(String, Option<String>, Option<String>)> {
    fn enc(e: &Episode) -> Option<String> {
        if e.enclosures.is_empty() {
            return None;
        }
        Some(
            e.enclosures
                .iter()
                .map(|x| {
                    format!(
                        "{} {} {}",
                        x.url,
                        x.mime_type.as_deref().unwrap_or("-"),
                        x.length_bytes
                            .map_or_else(|| "-".to_owned(), |l| l.to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
    fn ts(t: Option<OffsetDateTime>) -> Option<String> {
        t.map(|t| t.unix_timestamp().to_string())
    }
    fn num<T: ToString>(v: Option<T>) -> Option<String> {
        v.map(|v| v.to_string())
    }
    let pairs: Vec<(&str, Option<String>, Option<String>)> = vec![
        ("guid", old.guid.clone(), new.guid.clone()),
        ("title", Some(old.title.clone()), Some(new.title.clone())),
        ("subtitle", old.subtitle.clone(), new.subtitle.clone()),
        (
            "description_html",
            old.description_html.clone(),
            new.description_html.clone(),
        ),
        (
            "link",
            old.link.as_ref().map(ToString::to_string),
            new.link.as_ref().map(ToString::to_string),
        ),
        ("published_at", ts(old.published_at), ts(new.published_at)),
        (
            "duration_secs",
            num(old.duration_secs),
            num(new.duration_secs),
        ),
        ("season", num(old.season), num(new.season)),
        (
            "episode_number",
            num(old.episode_number),
            num(new.episode_number),
        ),
        (
            "episode_type",
            old.episode_type.clone(),
            new.episode_type.clone(),
        ),
        ("explicit", num(old.explicit), num(new.explicit)),
        (
            "artwork_url",
            old.artwork_url.as_ref().map(ToString::to_string),
            new.artwork_url.as_ref().map(ToString::to_string),
        ),
        ("author", old.author.clone(), new.author.clone()),
        ("enclosures", enc(old), enc(new)),
        (
            "extras",
            (!old.extras.is_empty())
                .then(|| serde_json::to_string(&old.extras).unwrap_or_default()),
            (!new.extras.is_empty())
                .then(|| serde_json::to_string(&new.extras).unwrap_or_default()),
        ),
        (
            "malformed",
            old.malformed.then(|| "true".to_owned()),
            new.malformed.then(|| "true".to_owned()),
        ),
    ];
    pairs
        .into_iter()
        .filter(|(_, a, b)| a != b)
        .map(|(f, a, b)| (f.to_owned(), a, b))
        .collect()
}
