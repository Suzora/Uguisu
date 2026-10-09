//! Episode identity (ADR 0006, ADR 0014).
//!
//! Cascade per feed item: a GUID that is non-empty and unique within the
//! feed → `guid:<normalized>`; else a normalized primary enclosure URL that
//! is unique within the feed → `url:<normalized>`; else a fingerprint of
//! (normalized title, publish day, enclosure length) → `fp:<sha256>`. The
//! secondary keys are always computed so that a later refresh can recognise
//! an item whose GUID changed (see [`probable_same_episode`]).

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use time::OffsetDateTime;
use uguisu_core::model::{EpisodeIdentity, IdentitySource};
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::normalize::{NormalizedItem, parse_url};
use crate::parser::ParsedItem;

/// Query parameters stripped before an enclosure URL is used as a key.
const TRACKING_PARAMS: [&str; 12] = [
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "fbclid",
    "gclid",
    "mc_cid",
    "mc_eid",
    "ref",
    "src",
];

/// Known tracking / redirect prefixes that wrap the real media URL, with
/// the number of opaque path segments (tracking ids) that follow them.
const TRACKING_PREFIXES: [(&str, usize); 12] = [
    ("chtbl.com/track/", 1),
    ("www.podtrac.com/pts/redirect.mp3/", 0),
    ("dts.podtrac.com/redirect.mp3/", 0),
    ("dts.podtrac.com/redirect.m4a/", 0),
    ("pdst.fm/e/", 0),
    ("mgln.ai/e/", 0),
    ("mgln.ai/track/", 1),
    ("op3.dev/e/", 0),
    ("op3.dev/e,", 0),
    ("pdcn.co/e/", 0),
    ("pscrb.fm/rss/p/", 0),
    ("arttrk.com/p/", 1),
];

/// Normalizes an enclosure URL for identity purposes: lower-cased scheme and
/// host, default ports dropped, tracking parameters and fragments removed,
/// known tracking prefixes unwrapped, `http`/`https` folded.
#[must_use]
pub fn normalize_enclosure_url(url: &Url) -> String {
    let mut current = url.clone();
    // Unwrap tracking prefixes (they can be nested, e.g. chtbl → podtrac).
    for _ in 0..4 {
        let host = current.host_str().unwrap_or("").to_ascii_lowercase();
        let path = current.path().trim_start_matches('/');
        let combined = match current.query() {
            Some(q) => format!("{host}/{path}?{q}"),
            None => format!("{host}/{path}"),
        };
        let mut unwrapped = None;
        let lower = combined.to_ascii_lowercase();
        for (prefix, skip) in TRACKING_PREFIXES {
            let Some(mut rest) = lower
                .strip_prefix(prefix)
                .map(|_| &combined[prefix.len()..])
            else {
                continue;
            };
            for _ in 0..skip {
                match rest.split_once('/') {
                    Some((_, after)) => rest = after,
                    None => rest = "",
                }
            }
            if rest.is_empty() {
                continue;
            }
            let candidate = if rest.starts_with("http://") || rest.starts_with("https://") {
                rest.to_owned()
            } else {
                format!("https://{rest}")
            };
            if let Ok(u) = Url::parse(&candidate)
                && u.has_host()
            {
                unwrapped = Some(u);
                break;
            }
        }
        match unwrapped {
            Some(u) => current = u,
            None => break,
        }
    }
    let host = current.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let port = match (current.scheme(), current.port()) {
        (_, None) | ("http", Some(80)) | ("https", Some(443)) => String::new(),
        (_, Some(p)) => format!(":{p}"),
    };
    let path = current.path().trim_end_matches('/');
    let mut query: Vec<(String, String)> = current
        .query_pairs()
        .filter(|(k, _)| !TRACKING_PARAMS.contains(&k.to_ascii_lowercase().as_str()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    query.sort();
    let query = if query.is_empty() {
        String::new()
    } else {
        let joined: Vec<String> = query.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("?{}", joined.join("&"))
    };
    format!("{host}{port}{path}{query}")
}

/// Normalizes a title for identity: NFKC, case-folded, punctuation removed,
/// whitespace collapsed.
#[must_use]
pub fn normalize_title(title: &str) -> String {
    title
        .nfkc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalizes a GUID: trimmed, whitespace collapsed; case is kept because
/// GUIDs are opaque strings.
#[must_use]
pub fn normalize_guid(guid: &str) -> String {
    guid.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sha256_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0u8]);
        }
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}

/// Fingerprint of (normalized title, publish day, enclosure length).
#[must_use]
pub fn fingerprint(
    title: &str,
    published: Option<OffsetDateTime>,
    enclosure_length: Option<u64>,
) -> String {
    let day = published.map_or_else(String::new, |d| {
        let d = d.to_offset(time::UtcOffset::UTC).date();
        format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
    });
    let len = enclosure_length.map_or_else(String::new, |l| l.to_string());
    sha256_hex(&[&normalize_title(title), &day, &len])
}

/// Signals extracted from one item before the cascade is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySignals {
    /// Normalized GUID.
    pub guid: Option<String>,
    /// Normalized primary enclosure URL.
    pub enclosure: Option<String>,
    /// Fingerprint.
    pub fingerprint: Option<String>,
}

/// Collects the identity signals of a normalized item.
#[must_use]
pub fn signals(parsed: &ParsedItem, normalized: &NormalizedItem) -> IdentitySignals {
    let guid = parsed
        .guid
        .as_deref()
        .map(normalize_guid)
        .filter(|g| !g.is_empty());
    let primary = normalized.enclosures.iter().find(|e| e.is_primary);
    let enclosure = primary
        .map(|e| normalize_enclosure_url(&e.url))
        .or_else(|| {
            parsed
                .enclosures
                .first()
                .and_then(|e| parse_url(&e.url))
                .map(|u| normalize_enclosure_url(&u))
        })
        .filter(|s| !s.is_empty());
    let length = primary.and_then(|e| e.length_bytes);
    let fingerprint = match &normalized.identity_title {
        Some(title) => Some(fingerprint(title, normalized.published.value, length)),
        // An invented title says nothing about the item; with no enclosure
        // either, the fingerprint would be one constant for every such item.
        None => primary.map(|_| fingerprint(&normalized.title, normalized.published.value, length)),
    };
    IdentitySignals {
        guid,
        enclosure,
        fingerprint,
    }
}

/// Applies the cascade to every item of one feed document, taking
/// per-feed uniqueness into account. The result is index-aligned with
/// `items`.
#[must_use]
pub fn resolve_identities(items: &[IdentitySignals]) -> Vec<EpisodeIdentity> {
    let mut guid_counts: HashMap<&str, usize> = HashMap::new();
    let mut url_counts: HashMap<&str, usize> = HashMap::new();
    for s in items {
        if let Some(g) = &s.guid {
            *guid_counts.entry(g.as_str()).or_default() += 1;
        }
        if let Some(u) = &s.enclosure {
            *url_counts.entry(u.as_str()).or_default() += 1;
        }
    }
    let mut seen_fp: HashMap<String, usize> = HashMap::new();
    items
        .iter()
        .map(|s| {
            let guid_unique = s
                .guid
                .as_deref()
                .is_some_and(|g| guid_counts.get(g) == Some(&1));
            let url_unique = s
                .enclosure
                .as_deref()
                .is_some_and(|u| url_counts.get(u) == Some(&1));
            let (key, source, reason) = if let (true, Some(g)) = (guid_unique, &s.guid) {
                (
                    format!("guid:{g}"),
                    IdentitySource::Guid,
                    "unique guid".to_owned(),
                )
            } else if let (true, Some(u)) = (url_unique, &s.enclosure) {
                let why = if s.guid.is_some() {
                    "guid duplicated in feed; unique enclosure url"
                } else {
                    "no guid; unique enclosure url"
                };
                (
                    format!("url:{u}"),
                    IdentitySource::EnclosureUrl,
                    why.to_owned(),
                )
            } else {
                let fp = s
                    .fingerprint
                    .clone()
                    .unwrap_or_else(|| sha256_hex(&["", "", ""]));
                // Repeated fingerprints (identical duplicate items) collapse
                // onto the same key deliberately: they are the same episode.
                let n = seen_fp.entry(fp.clone()).or_insert(0);
                *n += 1;
                let why = match (&s.guid, &s.enclosure) {
                    (Some(_), Some(_)) => "guid and enclosure url duplicated in feed; fingerprint",
                    (Some(_), None) => "guid duplicated in feed, no enclosure; fingerprint",
                    (None, Some(_)) => "no guid, enclosure url duplicated in feed; fingerprint",
                    (None, None) => "no guid, no enclosure; fingerprint",
                };
                (
                    format!("fp:{fp}"),
                    IdentitySource::Fingerprint,
                    why.to_owned(),
                )
            };
            EpisodeIdentity {
                key,
                source,
                guid_key: s.guid.clone(),
                enclosure_key: s.enclosure.clone(),
                fingerprint_key: s.fingerprint.clone(),
                reason,
            }
        })
        .collect()
}

/// Hash of the fields whose change counts as an episode update
/// (brief §20). Dates are compared at second precision in UTC.
#[must_use]
pub fn comparable_hash(n: &NormalizedItem) -> String {
    let published = n
        .published
        .value
        .map_or_else(String::new, |d| d.unix_timestamp().to_string());
    let enclosures: Vec<String> = n
        .enclosures
        .iter()
        .map(|e| {
            format!(
                "{}|{}|{}|{}",
                e.url,
                e.mime_type.as_deref().unwrap_or(""),
                e.length_bytes.map_or_else(String::new, |l| l.to_string()),
                u8::from(e.is_primary)
            )
        })
        .collect();
    let extras = serde_json::to_string(&n.extras).unwrap_or_default();
    let parts: Vec<String> = vec![
        n.title.clone(),
        n.subtitle.clone().unwrap_or_default(),
        n.description_html.clone().unwrap_or_default(),
        n.link.as_ref().map_or("", Url::as_str).to_owned(),
        published,
        n.duration_secs.map_or_else(String::new, |d| d.to_string()),
        n.season.map_or_else(String::new, |d| d.to_string()),
        n.episode_number.map_or_else(String::new, |d| d.to_string()),
        n.episode_type.clone().unwrap_or_default(),
        n.explicit.map_or_else(String::new, |e| e.to_string()),
        n.artwork_url.as_ref().map_or("", Url::as_str).to_owned(),
        n.author.clone().unwrap_or_default(),
        enclosures.join("\n"),
        extras,
    ];
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    sha256_hex(&refs)
}

/// Reasons two items are probably the same episode (ADR 0014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchReason {
    /// Same normalized enclosure URL.
    SameEnclosureUrl,
    /// Same publication minute.
    SamePublishedMinute,
    /// Same normalized title.
    SameTitle,
    /// Same enclosure length.
    SameMediaLength,
    /// Same fingerprint.
    SameFingerprint,
}

impl MatchReason {
    /// Stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SameEnclosureUrl => "same_enclosure_url",
            Self::SamePublishedMinute => "same_published_minute",
            Self::SameTitle => "same_title",
            Self::SameMediaLength => "same_media_length",
            Self::SameFingerprint => "same_fingerprint",
        }
    }
}

/// The comparable facts of a stored or incoming episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchFacts {
    /// Normalized enclosure URL.
    pub enclosure_key: Option<String>,
    /// Fingerprint key.
    pub fingerprint_key: Option<String>,
    /// Normalized title.
    pub title: String,
    /// Publication time.
    pub published_at: Option<OffsetDateTime>,
    /// Primary enclosure length.
    pub enclosure_length: Option<u64>,
}

/// Which secondary signals two items share.
#[must_use]
pub fn probable_same_episode(a: &MatchFacts, b: &MatchFacts) -> Vec<MatchReason> {
    let mut reasons = Vec::new();
    if a.enclosure_key.is_some() && a.enclosure_key == b.enclosure_key {
        reasons.push(MatchReason::SameEnclosureUrl);
    }
    if a.fingerprint_key.is_some() && a.fingerprint_key == b.fingerprint_key {
        reasons.push(MatchReason::SameFingerprint);
    }
    if let (Some(x), Some(y)) = (a.published_at, b.published_at)
        && (x.unix_timestamp() / 60) == (y.unix_timestamp() / 60)
    {
        reasons.push(MatchReason::SamePublishedMinute);
    }
    if !a.title.is_empty() && normalize_title(&a.title) == normalize_title(&b.title) {
        reasons.push(MatchReason::SameTitle);
    }
    if a.enclosure_length.is_some() && a.enclosure_length == b.enclosure_length {
        reasons.push(MatchReason::SameMediaLength);
    }
    reasons
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use time::macros::datetime;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn enclosure_urls_are_normalized_and_unwrapped() {
        assert_eq!(
            normalize_enclosure_url(&u(
                "HTTPS://WWW.CDN.Example:443/a/B.mp3?utm_source=x&b=2&a=1#frag"
            )),
            "cdn.example/a/B.mp3?a=1&b=2"
        );
        assert_eq!(
            normalize_enclosure_url(&u("http://cdn.example/a.mp3")),
            normalize_enclosure_url(&u("https://cdn.example/a.mp3/"))
        );
        assert_eq!(
            normalize_enclosure_url(&u(
                "https://dts.podtrac.com/redirect.mp3/www.buzzsprout.com/1/episodes/2562823-x.mp3"
            )),
            "buzzsprout.com/1/episodes/2562823-x.mp3"
        );
        assert_eq!(
            normalize_enclosure_url(&u(
                "https://chtbl.com/track/ABC123/dts.podtrac.com/redirect.mp3/traffic.megaphone.fm/X.mp3?updated=1"
            )),
            "traffic.megaphone.fm/X.mp3?updated=1"
        );
        assert_eq!(
            normalize_enclosure_url(&u("https://pdst.fm/e/https://cdn.example/x.mp3")),
            "cdn.example/x.mp3"
        );
        assert_eq!(
            normalize_enclosure_url(&u("http://cdn.example:8080/x.mp3")),
            "cdn.example:8080/x.mp3"
        );
    }

    #[test]
    fn titles_and_guids_normalize() {
        assert_eq!(
            normalize_title("  Épisode #1 – „Zitat“  "),
            "épisode 1 zitat"
        );
        assert_eq!(normalize_title("Ｆｕｌｌｗｉｄｔｈ"), "fullwidth");
        assert_eq!(normalize_guid(" a  b "), "a b");
        assert_eq!(normalize_guid("ABC"), "ABC");
    }

    #[test]
    fn fingerprint_uses_day_precision() {
        let a = fingerprint("Ep", Some(datetime!(2025-09-01 23:59:00 UTC)), Some(10));
        let b = fingerprint("EP!", Some(datetime!(2025-09-01 00:01:00 UTC)), Some(10));
        let c = fingerprint("Ep", Some(datetime!(2025-09-02 00:01:00 UTC)), Some(10));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(
            a,
            fingerprint("Ep", Some(datetime!(2025-09-01 23:59:00 UTC)), Some(11))
        );
        assert_eq!(a.len(), 64);
    }

    fn sig(guid: Option<&str>, url: Option<&str>, fp: &str) -> IdentitySignals {
        IdentitySignals {
            guid: guid.map(str::to_owned),
            enclosure: url.map(str::to_owned),
            fingerprint: Some(fp.to_owned()),
        }
    }

    #[test]
    fn the_cascade_prefers_guid_then_url() {
        let items = vec![
            sig(Some("g1"), Some("u1"), "f1"),
            sig(Some("dup"), Some("u2"), "f2"),
            sig(Some("dup"), Some("u3"), "f3"),
            sig(None, Some("shared"), "f4"),
            sig(None, Some("shared"), "f5"),
            sig(None, None, "f6"),
            sig(None, None, "f6"),
        ];
        let ids = resolve_identities(&items);
        assert_eq!(ids[0].key, "guid:g1");
        assert_eq!(ids[0].source, IdentitySource::Guid);
        assert_eq!(ids[1].key, "url:u2");
        assert_eq!(ids[1].source, IdentitySource::EnclosureUrl);
        assert!(ids[1].reason.contains("duplicated"));
        assert_eq!(ids[2].key, "url:u3");
        assert_eq!(ids[3].key, "fp:f4");
        assert_eq!(ids[3].source, IdentitySource::Fingerprint);
        assert_eq!(ids[4].key, "fp:f5");
        assert_eq!(ids[5].key, ids[6].key, "identical items share one key");
        assert_eq!(ids[1].guid_key.as_deref(), Some("dup"));
        assert_eq!(ids[3].enclosure_key.as_deref(), Some("shared"));
    }

    #[test]
    fn match_reasons() {
        let a = MatchFacts {
            enclosure_key: Some("cdn/x.mp3".into()),
            fingerprint_key: Some("f".into()),
            title: "Episode One".into(),
            published_at: Some(datetime!(2025-09-01 10:00:20 UTC)),
            enclosure_length: Some(1000),
        };
        let b = MatchFacts {
            enclosure_key: Some("cdn/x.mp3".into()),
            fingerprint_key: Some("g".into()),
            title: "episode one!".into(),
            published_at: Some(datetime!(2025-09-01 10:00:50 UTC)),
            enclosure_length: Some(1000),
        };
        let r = probable_same_episode(&a, &b);
        assert!(r.contains(&MatchReason::SameEnclosureUrl));
        assert!(r.contains(&MatchReason::SameTitle));
        assert!(r.contains(&MatchReason::SamePublishedMinute));
        assert!(r.contains(&MatchReason::SameMediaLength));
        assert!(!r.contains(&MatchReason::SameFingerprint));
        let none = MatchFacts {
            enclosure_key: None,
            fingerprint_key: None,
            title: "Other".into(),
            published_at: None,
            enclosure_length: None,
        };
        assert!(probable_same_episode(&a, &none).is_empty());
    }
}
