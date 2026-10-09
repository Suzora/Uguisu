//! Deciding, without being asked, whether an episode should be downloaded
//! (ADR 0023).
//!
//! The decision is a pure function of the policy, the episode and the
//! current time: no database, no filesystem, no queue. The engine asks it
//! and then does the queuing, so [`uguisu-download`]'s service never
//! learns what a policy is, and the rule can be tested as a table.
//!
//! Automatic archiving is **off** unless it is turned on, globally or per
//! podcast, and this function is only ever consulted for episodes the
//! engine discovered by itself. An explicit `download` command does not go
//! through here at all: a user who asks for an episode gets it, whatever
//! the policy says.
//!
//! [`uguisu-download`]: https://docs.rs/uguisu-download

use time::{Duration, OffsetDateTime};
use uguisu_core::archive::{ArchivePolicy, PolicyDecision, PolicyMode, policy_reason};
use uguisu_core::config::ArchiveConfig;
use uguisu_core::download::Priority;
use uguisu_core::model::{ArchiveState, Episode};

/// The policy actually in force for one podcast: the global defaults with
/// the podcast's overrides applied.
///
/// Resolving the two into one value up front keeps the precedence rule in
/// a single place, instead of spreading `unwrap_or` over the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectivePolicy {
    /// Whether the policy queues anything at all.
    pub mode: PolicyMode,
    /// Most episodes of this podcast that may be waiting to be archived at
    /// once; 0 means no limit.
    pub max_backlog: u32,
    /// Episodes published longer ago than this are left alone; 0 means no
    /// limit.
    pub max_age_days: u32,
    /// Priority for jobs this policy creates.
    pub priority: Priority,
}

impl EffectivePolicy {
    /// The installation-wide policy from the configuration.
    #[must_use]
    pub fn global(cfg: &ArchiveConfig) -> Self {
        Self {
            mode: if cfg.auto_download {
                PolicyMode::Auto
            } else {
                PolicyMode::Manual
            },
            max_backlog: cfg.max_backlog,
            max_age_days: cfg.max_age_days,
            priority: cfg.priority,
        }
    }

    /// The global policy with one podcast's overrides applied. A field the
    /// podcast leaves unset keeps the global value; `mode` is always the
    /// podcast's, which is what makes per-podcast opt-in and opt-out work
    /// on an installation whose default is the other way round.
    #[must_use]
    pub fn resolve(cfg: &ArchiveConfig, podcast: Option<&ArchivePolicy>) -> Self {
        let mut out = Self::global(cfg);
        if let Some(p) = podcast {
            out.mode = p.mode;
            if let Some(v) = p.max_backlog {
                out.max_backlog = v;
            }
            if let Some(v) = p.max_age_days {
                out.max_age_days = v;
            }
            if let Some(v) = p.priority {
                out.priority = v;
            }
        }
        out
    }
}

/// What the engine already knows about the episode's place in the queue,
/// so the decision does not have to look anything up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    /// A download job already exists for this episode.
    pub has_job: bool,
    /// An archive record already exists for this episode.
    pub has_archive_file: bool,
    /// How many of this podcast's episodes are already waiting to be
    /// archived, including the ones the current evaluation just queued.
    /// Compared against the backlog limit, newest episode first.
    pub queued_so_far: u32,
}

/// Decides what to do with one discovered episode.
///
/// The order of the checks is the order of their cost and certainty: what
/// the policy forbids outright, then what the episode itself rules out,
/// then the limits. Every `Skip` names a reason from
/// [`policy_reason`](uguisu_core::archive::policy_reason), so the event log
/// says why an episode was passed over rather than leaving the user to
/// guess.
#[must_use]
pub fn decide(
    policy: &EffectivePolicy,
    episode: &Episode,
    position: Position,
    now: OffsetDateTime,
) -> PolicyDecision {
    let skip = |reason: &str| PolicyDecision::Skip {
        reason: reason.to_owned(),
    };

    if policy.mode == PolicyMode::Manual {
        return skip(policy_reason::DISABLED);
    }
    // Anything already in flight or already stored is left alone: the
    // database's unique constraint on `download_jobs.episode_id` is the
    // real boundary, this only avoids the pointless attempt.
    if position.has_archive_file || episode.archive_state == ArchiveState::Archived {
        return skip(policy_reason::ALREADY_ARCHIVED);
    }
    if position.has_job
        || matches!(
            episode.archive_state,
            ArchiveState::Queued | ArchiveState::Downloading
        )
    {
        return skip(policy_reason::ALREADY_QUEUED);
    }
    if episode.archive_state == ArchiveState::Skipped || episode.skip_reason.is_some() {
        return skip(policy_reason::SKIPPED);
    }
    if episode.duplicate_of_episode_id.is_some() {
        // A duplicate candidate would store the same bytes twice under two
        // names; a user can still ask for it explicitly.
        return skip(policy_reason::DUPLICATE);
    }
    if episode.removed_from_feed_at.is_some() {
        return skip(policy_reason::REMOVED_FROM_FEED);
    }
    if episode.primary_enclosure().is_none() {
        return skip(policy_reason::NO_ENCLOSURE);
    }
    if is_too_old(policy.max_age_days, episode, now) {
        return skip(policy_reason::TOO_OLD);
    }
    if policy.max_backlog > 0 && position.queued_so_far >= policy.max_backlog {
        return skip(policy_reason::BACKLOG_EXCEEDED);
    }
    PolicyDecision::Queue {
        priority: policy.priority,
    }
}

/// Whether an episode falls outside the age limit.
///
/// An episode with no publication date is **not** too old: the date is
/// missing, not ancient, and refusing it would silently drop every episode
/// of a feed that omits dates. `sort_at` is not used as a substitute,
/// because for such a feed it is the time Uguisu first saw the item, which
/// would make the answer depend on when the podcast was added.
fn is_too_old(max_age_days: u32, episode: &Episode, now: OffsetDateTime) -> bool {
    if max_age_days == 0 {
        return false;
    }
    let Some(published) = episode.published_at else {
        return false;
    };
    let cutoff = now - Duration::days(i64::from(max_age_days));
    published < cutoff
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use uguisu_core::ids::{EnclosureId, EpisodeId, PodcastId};
    use uguisu_core::model::{
        DateQuality, Enclosure, EnclosureKind, Episode, EpisodeExtras, EpisodeIdentity,
        IdentitySource,
    };
    use url::Url;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_758_000_000).unwrap()
    }

    fn auto() -> EffectivePolicy {
        EffectivePolicy {
            mode: PolicyMode::Auto,
            max_backlog: 3,
            max_age_days: 0,
            priority: Priority::Normal,
        }
    }

    fn episode() -> Episode {
        let podcast_id = PodcastId::new();
        let id = EpisodeId::new();
        Episode {
            id,
            podcast_id,
            guid: Some("g".into()),
            guid_is_permalink: None,
            identity: EpisodeIdentity {
                key: "guid:g".into(),
                source: IdentitySource::Guid,
                guid_key: Some("g".into()),
                enclosure_key: None,
                fingerprint_key: None,
                reason: "test".into(),
            },
            title: "Pilot".into(),
            subtitle: None,
            sort_title: "pilot".into(),
            description_html: None,
            description_text: None,
            link: None,
            published_at: Some(now() - Duration::days(2)),
            published_at_raw: None,
            published_at_quality: DateQuality::Exact,
            updated_at_source: None,
            duration_secs: None,
            duration_raw: None,
            season: None,
            episode_number: None,
            episode_type: None,
            explicit: None,
            artwork_url: None,
            author: None,
            content_hash: "c".into(),
            archive_state: ArchiveState::Expected,
            skip_reason: None,
            malformed: false,
            malformed_reason: None,
            duplicate_of_episode_id: None,
            duplicate_reasons: Vec::new(),
            missing_streak: 0,
            first_seen_at: now(),
            last_seen_in_feed_at: now(),
            removed_from_feed_at: None,
            sort_at: now(),
            source_metadata: None,
            enclosures: vec![Enclosure {
                id: EnclosureId::new(),
                episode_id: id,
                url: Url::parse("https://cdn.test/a.mp3").unwrap(),
                length_bytes: Some(1024),
                mime_type: Some("audio/mpeg".into()),
                is_primary: true,
                kind: EnclosureKind::Audio,
                position: 0,
                bitrate: None,
                height: None,
                codecs: None,
                lang: None,
                title: None,
                integrity_type: None,
                integrity_value: None,
                sources: Vec::new(),
            }],
            extras: EpisodeExtras::default(),
            created_at: now(),
            updated_at: now(),
        }
    }

    fn reason_of(d: &PolicyDecision) -> &str {
        match d {
            PolicyDecision::Skip { reason } => reason,
            PolicyDecision::Queue { .. } => "queue",
        }
    }

    #[test]
    fn a_fresh_episode_takes_its_priority() {
        let mut p = auto();
        p.priority = Priority::High;
        let d = decide(&p, &episode(), Position::default(), now());
        assert_eq!(
            d,
            PolicyDecision::Queue {
                priority: Priority::High
            }
        );
    }

    #[test]
    fn manual_mode_queues_nothing_at_all() {
        let p = EffectivePolicy {
            mode: PolicyMode::Manual,
            ..auto()
        };
        assert_eq!(
            reason_of(&decide(&p, &episode(), Position::default(), now())),
            policy_reason::DISABLED
        );
    }

    #[test]
    fn every_skip_reason_is_named() {
        /// One way to make an episode ineligible.
        type Case = (&'static str, Box<dyn Fn(&mut Episode, &mut Position)>);

        let cases: Vec<Case> = vec![
            (
                policy_reason::ALREADY_ARCHIVED,
                Box::new(|_, pos| pos.has_archive_file = true),
            ),
            (
                policy_reason::ALREADY_ARCHIVED,
                Box::new(|e, _| e.archive_state = ArchiveState::Archived),
            ),
            (
                policy_reason::ALREADY_QUEUED,
                Box::new(|_, pos| pos.has_job = true),
            ),
            (
                policy_reason::ALREADY_QUEUED,
                Box::new(|e, _| e.archive_state = ArchiveState::Downloading),
            ),
            (
                policy_reason::SKIPPED,
                Box::new(|e, _| e.skip_reason = Some("user".into())),
            ),
            (
                policy_reason::DUPLICATE,
                Box::new(|e, _| e.duplicate_of_episode_id = Some(EpisodeId::new())),
            ),
            (
                policy_reason::REMOVED_FROM_FEED,
                Box::new(|e, _| e.removed_from_feed_at = Some(now())),
            ),
            (
                policy_reason::NO_ENCLOSURE,
                Box::new(|e, _| e.enclosures.clear()),
            ),
        ];
        for (expected, mutate) in cases {
            let mut e = episode();
            let mut pos = Position::default();
            mutate(&mut e, &mut pos);
            let d = decide(&auto(), &e, pos, now());
            assert_eq!(reason_of(&d), expected, "{e:?}");
        }
    }

    #[test]
    fn the_age_limit_tolerates_no_date() {
        let p = EffectivePolicy {
            max_age_days: 30,
            ..auto()
        };
        let mut old = episode();
        old.published_at = Some(now() - Duration::days(31));
        assert_eq!(
            reason_of(&decide(&p, &old, Position::default(), now())),
            policy_reason::TOO_OLD
        );

        let mut just_inside = episode();
        just_inside.published_at = Some(now() - Duration::days(29));
        assert!(matches!(
            decide(&p, &just_inside, Position::default(), now()),
            PolicyDecision::Queue { .. }
        ));

        // No date is "unknown", not "ancient": a feed without dates must
        // not silently archive nothing.
        let mut undated = episode();
        undated.published_at = None;
        undated.published_at_quality = DateQuality::Invalid;
        assert!(matches!(
            decide(&p, &undated, Position::default(), now()),
            PolicyDecision::Queue { .. }
        ));

        // 0 turns the limit off, however old the episode is.
        let mut ancient = episode();
        ancient.published_at = Some(now() - Duration::days(10_000));
        assert!(matches!(
            decide(&auto(), &ancient, Position::default(), now()),
            PolicyDecision::Queue { .. }
        ));
    }

    #[test]
    fn the_backlog_limit_counts_outstanding_work() {
        let p = auto();
        for queued in 0..p.max_backlog {
            let pos = Position {
                queued_so_far: queued,
                ..Position::default()
            };
            assert!(
                matches!(
                    decide(&p, &episode(), pos, now()),
                    PolicyDecision::Queue { .. }
                ),
                "{queued} of {}",
                p.max_backlog
            );
        }
        let pos = Position {
            queued_so_far: p.max_backlog,
            ..Position::default()
        };
        assert_eq!(
            reason_of(&decide(&p, &episode(), pos, now())),
            policy_reason::BACKLOG_EXCEEDED
        );

        let unlimited = EffectivePolicy {
            max_backlog: 0,
            ..auto()
        };
        let pos = Position {
            queued_so_far: 10_000,
            ..Position::default()
        };
        assert!(matches!(
            decide(&unlimited, &episode(), pos, now()),
            PolicyDecision::Queue { .. }
        ));
    }

    #[test]
    fn a_podcast_overrides_field_by_field() {
        let mut cfg = ArchiveConfig {
            auto_download: false,
            max_backlog: 3,
            max_age_days: 90,
            priority: Priority::Normal,
            ..ArchiveConfig::default()
        };

        assert_eq!(
            EffectivePolicy::resolve(&cfg, None).mode,
            PolicyMode::Manual
        );

        // Opting one podcast in, on an installation that is opted out.
        let podcast_id = PodcastId::new();
        let opted_in = ArchivePolicy {
            podcast_id,
            mode: PolicyMode::Auto,
            max_backlog: Some(10),
            max_age_days: None,
            priority: Some(Priority::High),
            updated_at: now(),
        };
        let resolved = EffectivePolicy::resolve(&cfg, Some(&opted_in));
        assert_eq!(resolved.mode, PolicyMode::Auto);
        assert_eq!(resolved.max_backlog, 10);
        assert_eq!(resolved.max_age_days, 90, "unset fields keep the default");
        assert_eq!(resolved.priority, Priority::High);

        // And opting one podcast out, on an installation that is opted in.
        cfg.auto_download = true;
        assert_eq!(EffectivePolicy::global(&cfg).mode, PolicyMode::Auto);
        let opted_out = ArchivePolicy {
            podcast_id,
            mode: PolicyMode::Manual,
            max_backlog: None,
            max_age_days: None,
            priority: None,
            updated_at: now(),
        };
        let resolved = EffectivePolicy::resolve(&cfg, Some(&opted_out));
        assert_eq!(resolved.mode, PolicyMode::Manual);
        assert_eq!(resolved.max_backlog, 3);
    }

    #[test]
    fn the_shipped_defaults_archive_nothing() {
        let cfg = ArchiveConfig::default();
        let d = decide(
            &EffectivePolicy::global(&cfg),
            &episode(),
            Position::default(),
            now(),
        );
        assert_eq!(reason_of(&d), policy_reason::DISABLED);
    }
}
