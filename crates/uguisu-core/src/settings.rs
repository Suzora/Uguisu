//! What a configuration key *is*, as opposed to what it currently holds
//! (ADR 0028).
//!
//! [`SETTINGS`] is the one list of keys Uguisu knows. It is metadata, not a
//! parser: the parsing and the cross-field validation stay in
//! [`crate::config::Config::from_layers`], because a second parser is a
//! second set of rules to keep in agreement. What this table adds is the
//! things an API, a CLI and a UI need to know before they offer a key to
//! somebody — what shape a value has, whether it may be stored in the
//! database at all, and whether changing it does anything before a restart.
//!
//! The key *is* the environment variable's name. One vocabulary means a
//! user who has read `docs/CONFIGURATION.md` can already use the API, and a
//! stored row and an exported environment mean the same thing.

use std::collections::BTreeMap;

use crate::config::env_keys as k;

/// The shape of a value, so a caller can render an input and reject an
/// obvious mistake before the parser sees it.
///
/// This is deliberately coarse. The authority on whether a value is
/// acceptable is the parser, which also knows the cross-field rules
/// (`per_host <= global`, and so on) that no per-key description can
/// express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// `true` or `false`.
    Bool,
    /// A whole number within an inclusive range.
    Integer {
        /// Smallest accepted value.
        min: i64,
        /// Largest accepted value.
        max: i64,
    },
    /// A duration in milliseconds.
    Millis,
    /// A duration in seconds.
    Secs,
    /// A size in bytes.
    Bytes,
    /// Free text.
    Text,
    /// A filesystem path.
    Path,
    /// One of a fixed set of words.
    Choice(&'static [&'static str]),
}

impl SettingKind {
    /// Stable name, as the API spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Integer { .. } => "integer",
            Self::Millis => "millis",
            Self::Secs => "secs",
            Self::Bytes => "bytes",
            Self::Text => "text",
            Self::Path => "path",
            Self::Choice(_) => "choice",
        }
    }
}

/// Which half of the configuration a key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Providers and the shared HTTP client.
    Discovery,
    /// Everything the engine owns.
    Engine,
}

/// One configuration key.
#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    /// The `UGUISU_*` name, which is also the stored key.
    pub key: &'static str,
    /// The shape of the value.
    pub kind: SettingKind,
    /// Which sub-configuration parses it.
    pub scope: Scope,
    /// Whether a value for it may live in the database.
    ///
    /// `false` for three kinds of key, each for its own reason: a secret
    /// (`docs/CONFIGURATION.md` — secrets stay in the environment or a
    /// file), the directories that decide where the database and the media
    /// live (a row inside the database cannot say where the database is),
    /// and the SSRF allowlist (`docs/SECURITY.md` §3.1 — one compromised
    /// session must not be able to widen the network policy).
    pub persistable: bool,
    /// Whether a change takes effect without restarting the process.
    ///
    /// `false` where the value is captured at construction — the HTTP
    /// clients, the discovery stack, the download queue's `Deps` and the
    /// destination resolver all read their configuration once. Those keys
    /// are still stored; the API says `restart_required` rather than
    /// pretending the change landed.
    pub live: bool,
    /// Whether the value is a credential.
    ///
    /// A secret is never echoed: not as the value in force, not as the row
    /// somebody stored, and not in a report of a row Uguisu is ignoring.
    /// Every secret is also unstorable, which is why [`secret_of`] fixes
    /// both — a credential in the database would need encryption at rest,
    /// and there is none (`docs/SECURITY.md` §3.7).
    pub secret: bool,
    /// One line for a person choosing a value.
    pub summary: &'static str,
}

const fn spec_of(
    key: &'static str,
    kind: SettingKind,
    scope: Scope,
    persistable: bool,
    live: bool,
    summary: &'static str,
) -> SettingSpec {
    SettingSpec {
        key,
        kind,
        scope,
        persistable,
        live,
        secret: false,
        summary,
    }
}

/// A key whose value is a credential, and therefore never stored and never
/// echoed.
const fn secret_of(
    key: &'static str,
    kind: SettingKind,
    scope: Scope,
    summary: &'static str,
) -> SettingSpec {
    SettingSpec {
        key,
        kind,
        scope,
        persistable: false,
        live: false,
        secret: true,
        summary,
    }
}

const COUNT: SettingKind = SettingKind::Integer { min: 1, max: 1024 };
const ANY_COUNT: SettingKind = SettingKind::Integer {
    min: 0,
    max: 1_000_000,
};
const POSITIVE: SettingKind = SettingKind::Integer {
    min: 1,
    max: 1_000_000,
};
const PERCENT: SettingKind = SettingKind::Integer { min: 1, max: 100 };
const RESULTS: SettingKind = SettingKind::Integer { min: 1, max: 100 };
const INTERVAL: SettingKind = SettingKind::Integer {
    min: crate::config::MIN_REFRESH_INTERVAL_SECS.cast_signed(),
    max: crate::config::MAX_INTERVAL_SECS.cast_signed(),
};
const PROFILES: SettingKind = SettingKind::Choice(&["windows", "posix", "portable"]);
const DEPTHS: SettingKind = SettingKind::Choice(&["existence", "light", "full"]);
const PRIORITIES: SettingKind = SettingKind::Choice(&["low", "normal", "high"]);
const TAG_MODES: SettingKind = SettingKind::Choice(&["fill_missing", "sync"]);
const LANGS: SettingKind = SettingKind::Choice(&["en_us", "ja_jp"]);

use Scope::{Discovery as D, Engine as E};
use SettingKind::{Bool, Bytes, Millis, Path, Secs, Text};

/// Every key Uguisu reads, in the order `config validate` prints them.
///
/// Adding a key here and an arm in `Config::from_layers` is the whole change:
/// every key list in the crate is derived from this table.
///
/// Left as written rather than reformatted: a row per key, aligned, is
/// the point. Broken across six lines each it stops being a table and
/// becomes 600 lines nobody reads.
#[rustfmt::skip]
pub const SETTINGS: &[SettingSpec] = &[
    // --- discovery providers
    spec_of(k::APPLE_ENABLED, Bool, D, true, false,
        "Query the Apple directory. No key is needed."),
    spec_of(k::APPLE_COUNTRY, Text, D, true, false,
        "Storefront country as an ISO 3166-1 alpha-2 code."),
    spec_of(k::APPLE_LANG, LANGS, D, true, false,
        "Apple accepts only these two language tags."),
    spec_of(k::APPLE_BASE_URL, Text, D, true, false,
        "Base URL override, for a mirror or a test server."),
    spec_of(k::PODCASTINDEX_ENABLED, Bool, D, true, false,
        "Query Podcast Index. Defaults to whether a key is set."),
    secret_of(k::PODCASTINDEX_KEY, Text, D,
        "Podcast Index API key. Never stored in the database."),
    secret_of(k::PODCASTINDEX_SECRET, Text, D,
        "Podcast Index API secret. Never stored in the database."),
    spec_of(k::PODCASTINDEX_BASE_URL, Text, D, false, false,
        "Base URL override. Environment only: the key and secret go wherever it points."),
    spec_of(k::GPODDERNET_ENABLED, Bool, D, true, false,
        "Query gpodder.net."),
    spec_of(k::GPODDERNET_BASE_URL, Text, D, true, false,
        "Base URL override."),
    spec_of(k::SOFT_DEADLINE_MS, Millis, D, true, false,
        "When a search returns what it has so far."),
    spec_of(k::HARD_DEADLINE_MS, Millis, D, true, false,
        "When a search gives up on the rest."),
    spec_of(k::PROVIDER_TIMEOUT_MS, Millis, D, true, false,
        "Per-provider request timeout."),
    spec_of(k::LIMIT, RESULTS, D, true, false,
        "Results a search asks each provider for."),
    spec_of(k::CACHE_SEARCH_TTL_SECS, Secs, D, true, false,
        "How long a search result stays usable."),
    spec_of(k::CACHE_LOOKUP_TTL_SECS, Secs, D, true, false,
        "How long a lookup result stays usable."),
    spec_of(k::CACHE_MAX_ENTRIES, ANY_COUNT, D, true, false,
        "Entries the in-memory provider cache holds."),
    // --- shared HTTP
    spec_of(k::HTTP_ALLOW_PRIVATE_HOSTS, Text, D, false, false,
        "Hosts allowed to resolve to private addresses. Environment only."),
    spec_of(k::HTTP_USER_AGENT, Text, D, true, false,
        "User agent sent with every request."),
    spec_of(k::HTTP_CONNECT_TIMEOUT_MS, Millis, D, true, false,
        "Connect timeout."),
    spec_of(k::HTTP_REQUEST_TIMEOUT_MS, Millis, D, true, false,
        "Whole-request timeout."),
    // --- data
    spec_of(k::DATA_DIR, Path, E, false, false,
        "Where the database and the lock live. Environment only."),
    // --- feeds
    spec_of(k::FEED_MAX_BYTES, Bytes, E, true, true,
        "Largest feed body accepted."),
    spec_of(k::FEED_MAX_ITEMS, POSITIVE, E, true, true,
        "Items parsed from one feed."),
    spec_of(k::FEED_REFRESH_CONCURRENCY, COUNT, E, true, true,
        "Feeds refreshed at once."),
    spec_of(k::FEED_REFRESH_TIMEOUT_MS, Millis, E, true, true,
        "How long one refresh may take."),
    spec_of(k::FEED_RETAIN_FETCHES, POSITIVE, E, true, true,
        "Fetch-log rows kept per podcast."),
    spec_of(k::FEED_REMOVAL_STREAK, COUNT, E, true, true,
        "Refreshes an episode must be absent before removal is recorded."),
    spec_of(k::FEED_MASS_REMOVAL_GUARD_PERCENT, PERCENT, E, true, true,
        "Percent of episodes that may vanish in one refresh before removal detection pauses."),
    spec_of(k::FEED_SCHEDULER, Bool, E, true, true,
        "Let `uguisu serve` refresh feeds on its own."),
    spec_of(k::FEED_REFRESH_INTERVAL_SECS, INTERVAL, E, true, true,
        "Default seconds between refreshes of one podcast."),
    // --- media and downloads
    spec_of(k::MEDIA_DIR, Path, E, false, false,
        "Where media is archived. Environment only."),
    spec_of(k::DOWNLOAD_GLOBAL_CONCURRENCY, COUNT, E, true, false,
        "Downloads running at once."),
    spec_of(k::DOWNLOAD_PER_HOST_CONCURRENCY, COUNT, E, true, false,
        "Downloads running at once against one host."),
    spec_of(k::DOWNLOAD_MAX_ATTEMPTS, COUNT, E, true, false,
        "Attempts before a job fails for good."),
    spec_of(k::DOWNLOAD_BACKOFF_BASE_MS, Millis, E, true, false,
        "First retry delay."),
    spec_of(k::DOWNLOAD_BACKOFF_MAX_MS, Millis, E, true, false,
        "Longest retry delay."),
    spec_of(k::DOWNLOAD_IDLE_TIMEOUT_MS, Millis, E, true, false,
        "How long a transfer may deliver nothing."),
    spec_of(k::DOWNLOAD_PROGRESS_INTERVAL_MS, Millis, E, true, false,
        "How often progress is published."),
    spec_of(k::DOWNLOAD_MAX_BYTES, Bytes, E, true, false,
        "Largest media file accepted; at least 1 MiB."),
    spec_of(k::DOWNLOAD_MIN_FREE_BYTES, Bytes, E, true, false,
        "Free space below which the queue pauses itself."),
    spec_of(k::DOWNLOAD_SHUTDOWN_GRACE_MS, Millis, E, true, false,
        "How long a shutdown waits for running work."),
    // --- archive
    spec_of(k::ARCHIVE_TEMPLATE, Text, E, true, false,
        "Path template for archived media (ADR 0009)."),
    spec_of(k::ARCHIVE_PATH_PROFILE, PROFILES, E, true, false,
        "Which characters a path segment may contain."),
    spec_of(k::ARCHIVE_VERIFY_ON_COMPLETION, Bool, E, true, true,
        "Verify a file as soon as it is downloaded."),
    spec_of(k::ARCHIVE_VERIFY_DEPTH, DEPTHS, E, true, true,
        "How thoroughly that check looks."),
    spec_of(k::ARCHIVE_AUTO_DOWNLOAD, Bool, E, true, true,
        "Queue discovered episodes unasked. Off by design (ADR 0023)."),
    spec_of(k::ARCHIVE_MAX_BACKLOG, ANY_COUNT, E, true, true,
        "Episodes of one podcast waiting at once. 0 means no limit."),
    spec_of(k::ARCHIVE_MAX_AGE_DAYS, ANY_COUNT, E, true, true,
        "Ignore episodes older than this. 0 means no limit."),
    spec_of(k::ARCHIVE_PRIORITY, PRIORITIES, E, true, true,
        "Queue priority for jobs the policy creates."),
    spec_of(k::ARCHIVE_SIDECARS, Bool, E, true, true,
        "Write a portable sidecar beside each archived file."),
    spec_of(k::ARCHIVE_MANIFESTS, Bool, E, true, true,
        "Maintain a sha256sum manifest per podcast."),
    spec_of(k::ARCHIVE_ARTWORK_FETCH, Bool, E, true, true,
        "Fetch podcast artwork on refresh. Off by design."),
    spec_of(k::ARCHIVE_ARTWORK_MAX_BYTES, Bytes, E, true, true,
        "Largest artwork image accepted."),
    spec_of(k::ARCHIVE_TAG_MODE, TAG_MODES, E, true, true,
        "Default mode for `archive tags write`."),
    spec_of(k::ARCHIVE_IMPORT_MATCH_THRESHOLD, PERCENT, E, true, true,
        "Score a candidate must reach to be imported."),
    // --- maintenance
    spec_of(k::MAINTENANCE_INTERVAL_SECS, INTERVAL, E, true, true,
        "Seconds between housekeeping passes."),
    spec_of(k::EVENTS_RETAIN_DAYS, ANY_COUNT, E, true, true,
        "Days of event history kept. 0 means no age limit."),
    spec_of(k::EVENTS_RETAIN_MAX_ROWS, ANY_COUNT, E, true, true,
        "Event rows kept whatever their age. 0 means no row limit."),
];

/// The specification of one key, if Uguisu knows it.
#[must_use]
pub fn spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Whether a value for `key` may be taken from the database.
///
/// An unknown key is not persistable: a value nothing reads is better
/// reported than quietly honoured.
#[must_use]
pub fn is_persistable(key: &str) -> bool {
    spec(key).is_some_and(|s| s.persistable)
}

/// Every key, in declaration order.
pub fn all() -> impl Iterator<Item = &'static str> + Clone {
    SETTINGS.iter().map(|s| s.key)
}

/// Every key of one scope, in declaration order.
pub fn in_scope(scope: Scope) -> impl Iterator<Item = &'static str> + Clone {
    SETTINGS
        .iter()
        .filter(move |s| s.scope == scope)
        .map(|s| s.key)
}

/// A stored setting, as the database holds it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Setting {
    /// The `UGUISU_*` key.
    pub key: String,
    /// The value, in the same syntax the environment variable uses.
    pub value: String,
    /// When it was last written.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// Who wrote it, when the caller said.
    pub updated_by: Option<String>,
}

/// The stored layer as a map, for handing to `Config::from_layers`.
#[must_use]
pub fn to_map(settings: &[Setting]) -> BTreeMap<String, String> {
    settings
        .iter()
        .map(|s| (s.key.clone(), s.value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn keys_are_unique_and_env_named() {
        let mut seen = std::collections::BTreeSet::new();
        for s in SETTINGS {
            assert!(s.key.starts_with("UGUISU_"), "{}", s.key);
            assert!(seen.insert(s.key), "{} is listed twice", s.key);
            assert!(!s.summary.is_empty(), "{} has no summary", s.key);
        }
        assert_eq!(seen.len(), SETTINGS.len());
        assert_eq!(all().count(), SETTINGS.len());
        assert_eq!(
            in_scope(Scope::Discovery).count() + in_scope(Scope::Engine).count(),
            SETTINGS.len()
        );
    }

    #[test]
    fn the_unstorable_keys_are_exactly_six() {
        let refused: Vec<&str> = SETTINGS
            .iter()
            .filter(|s| !s.persistable)
            .map(|s| s.key)
            .collect();
        assert_eq!(
            refused,
            vec![
                "UGUISU_PODCASTINDEX_KEY",
                "UGUISU_PODCASTINDEX_SECRET",
                "UGUISU_DISCOVERY_PODCASTINDEX_BASE_URL",
                "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS",
                "UGUISU_DATA_DIR",
                "UGUISU_MEDIA_DIR",
            ],
            "the two secrets, where they are sent, the SSRF allowlist and the two directories — \
             adding to this list is a decision, not an oversight"
        );
        assert!(!is_persistable("UGUISU_PODCASTINDEX_KEY"));
        assert!(!is_persistable("UGUISU_SOMETHING_NOBODY_DEFINED"));
        assert!(is_persistable("UGUISU_FEED_REFRESH_CONCURRENCY"));
    }

    #[test]
    fn a_secret_key_is_also_unstorable() {
        let secrets: Vec<&str> = SETTINGS
            .iter()
            .filter(|s| s.secret)
            .map(|s| s.key)
            .collect();
        assert_eq!(
            secrets,
            vec!["UGUISU_PODCASTINDEX_KEY", "UGUISU_PODCASTINDEX_SECRET"],
        );
        assert!(
            SETTINGS.iter().filter(|s| s.secret).all(|s| !s.persistable),
            "a credential in the database would need encryption at rest, \
             and there is none"
        );
    }

    #[test]
    fn a_choice_lists_its_accepted_words() {
        for s in SETTINGS {
            if let SettingKind::Choice(words) = s.kind {
                assert!(!words.is_empty(), "{}", s.key);
                for w in words {
                    assert!(!w.is_empty());
                }
            }
        }
        assert_eq!(
            spec("UGUISU_ARCHIVE_TAG_MODE").map(|s| s.kind),
            Some(TAG_MODES)
        );
    }

    #[test]
    fn declared_bounds_match_the_parser() {
        let parses = |key: &str, value: i64| {
            let value = value.to_string();
            crate::config::Config::from_lookup(|k| (k == key).then(|| value.clone())).is_ok()
        };
        for key in [
            "UGUISU_DISCOVERY_LIMIT",
            "UGUISU_FEED_MAX_ITEMS",
            "UGUISU_FEED_RETAIN_FETCHES",
            "UGUISU_FEED_REFRESH_INTERVAL_SECS",
            "UGUISU_MAINTENANCE_INTERVAL_SECS",
        ] {
            let SettingKind::Integer { min, max } = spec(key).unwrap().kind else {
                panic!("{key} is not an integer");
            };
            assert!(parses(key, min) && parses(key, max), "{key} {min}..={max}");
            assert!(!parses(key, min - 1), "{key} accepts {}", min - 1);
            // The parser sets no ceiling on the two feed counts.
            if key != "UGUISU_FEED_MAX_ITEMS" && key != "UGUISU_FEED_RETAIN_FETCHES" {
                assert!(!parses(key, max + 1), "{key} accepts {}", max + 1);
            }
        }
    }

    #[test]
    fn captured_configuration_is_not_live() {
        // The HTTP clients, the discovery stack and the download queue read
        // their configuration once, when the engine opens. Saying otherwise
        // would make the API report a change that did not happen.
        for key in [
            "UGUISU_HTTP_CONNECT_TIMEOUT_MS",
            "UGUISU_DISCOVERY_LIMIT",
            "UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY",
            "UGUISU_ARCHIVE_TEMPLATE",
        ] {
            assert!(!spec(key).unwrap().live, "{key} cannot be live");
        }
        for key in [
            "UGUISU_FEED_REFRESH_CONCURRENCY",
            "UGUISU_ARCHIVE_AUTO_DOWNLOAD",
        ] {
            assert!(spec(key).unwrap().live, "{key} is read per operation");
        }
    }
}
