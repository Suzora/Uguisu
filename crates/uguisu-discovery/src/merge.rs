//! Deterministic merge of a deduplicated group into one canonical candidate.
//!
//! Members are ordered by provider trust (highest first), then provider id
//! and provider reference, so the result never depends on the order in
//! which providers answered. Each field records its source in
//! `provenance`.

use std::collections::HashMap;

use uguisu_core::provider::ProviderId;

use crate::candidate::{FeedHealthHints, PodcastCandidate};

/// Per-field provider preference that overrides trust order.
fn preference(field: &str) -> &'static [ProviderId] {
    match field {
        // Apple's 600px artwork is the most consistent.
        "artwork" => &[
            ProviderId::APPLE,
            ProviderId::PODCAST_INDEX,
            ProviderId::GPODDER_NET,
        ],
        // Podcast Index tracks feed moves; Apple next.
        "feed_url" => &[
            ProviderId::PODCAST_INDEX,
            ProviderId::APPLE,
            ProviderId::GPODDER_NET,
        ],
        _ => &[],
    }
}

/// Merges `members` (at least one) into a canonical candidate.
#[allow(clippy::too_many_lines, clippy::implicit_hasher)] // one field table; callers use the std hasher
pub fn merge(
    mut members: Vec<PodcastCandidate>,
    trust: &HashMap<ProviderId, f32>,
) -> PodcastCandidate {
    let trust_of = |c: &PodcastCandidate| -> f32 {
        c.primary_provider()
            .and_then(|p| trust.get(&p).copied())
            .unwrap_or(0.5)
    };
    members.sort_by(|a, b| {
        trust_of(b)
            .partial_cmp(&trust_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.primary_provider().cmp(&b.primary_provider()))
            .then_with(|| {
                a.identities
                    .first()
                    .map(|i| i.provider_ref.clone())
                    .cmp(&b.identities.first().map(|i| i.provider_ref.clone()))
            })
    });
    if members.len() == 1 {
        return members.remove(0);
    }

    let by_preference = |field: &str| -> Vec<usize> {
        let prefs = preference(field);
        let mut order: Vec<usize> = (0..members.len()).collect();
        if !prefs.is_empty() {
            order.sort_by_key(|&i| {
                let p = members[i].primary_provider();
                prefs
                    .iter()
                    .position(|x| Some(*x) == p)
                    .unwrap_or(prefs.len())
            });
        }
        order
    };

    let mut out = members[0].clone();
    out.provenance.clear();
    let mut note = |field: &str, idx: usize| {
        if let Some(p) = members[idx].primary_provider() {
            out.provenance.insert(field.to_owned(), p);
        }
    };

    // title: highest trust (already first)
    note("title", 0);

    macro_rules! first_some {
        ($field:ident, $name:literal) => {{
            let order = by_preference($name);
            let mut chosen = None;
            for i in order {
                if members[i].$field.is_some() {
                    chosen = Some(i);
                    break;
                }
            }
            match chosen {
                Some(i) => {
                    out.$field.clone_from(&members[i].$field);
                    note($name, i);
                }
                None => out.$field = None,
            }
        }};
    }
    first_some!(author, "author");
    first_some!(publisher, "publisher");
    first_some!(artwork, "artwork");
    first_some!(language, "language");
    first_some!(website, "website");
    first_some!(feed_url, "feed_url");
    first_some!(episode_count, "episode_count");
    first_some!(last_published, "last_published");
    first_some!(explicit, "explicit");
    first_some!(itunes_id, "itunes_id");
    first_some!(podcast_guid, "podcast_guid");

    // description: longest
    let longest = members
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.description.as_ref().map(|d| (i, d.chars().count())))
        .max_by_key(|(i, len)| (*len, std::cmp::Reverse(*i)));
    match longest {
        Some((i, _)) => {
            out.description.clone_from(&members[i].description);
            note("description", i);
        }
        None => out.description = None,
    }

    // categories: union in trust order
    let mut cats: Vec<String> = Vec::new();
    for (i, m) in members.iter().enumerate() {
        for c in &m.categories {
            if !cats.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                cats.push(c.clone());
                if cats.len() == 1 {
                    note("categories", i);
                }
            }
        }
    }
    out.categories = cats;

    // identities and popularity: all, de-duplicated
    let mut identities = Vec::new();
    let mut popularity = Vec::new();
    for m in &members {
        for id in &m.identities {
            if !identities
                .iter()
                .any(|x: &crate::candidate::ProviderIdentity| {
                    x.provider == id.provider && x.provider_ref == id.provider_ref
                })
            {
                identities.push(id.clone());
            }
        }
        for p in &m.popularity {
            if !popularity
                .iter()
                .any(|x: &crate::candidate::Popularity| x.provider == p.provider)
            {
                popularity.push(p.clone());
            }
        }
    }
    out.identities = identities;
    out.popularity = popularity;

    // health: first Some per field in trust order
    let mut health = FeedHealthHints::default();
    for m in &members {
        health.dead = health.dead.or(m.health.dead);
        health.locked = health.locked.or(m.health.locked);
        health.last_update = health.last_update.or(m.health.last_update);
        health.http_status = health.http_status.or(m.health.http_status);
    }
    out.health = health;
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use time::OffsetDateTime;
    use url::Url;

    use super::*;
    use crate::candidate::{Popularity, ProviderIdentity};

    fn trust() -> HashMap<ProviderId, f32> {
        HashMap::from([
            (ProviderId::PODCAST_INDEX, 0.9),
            (ProviderId::APPLE, 0.8),
            (ProviderId::GPODDER_NET, 0.6),
        ])
    }

    fn cand(provider: ProviderId, title: &str) -> PodcastCandidate {
        let identity = ProviderIdentity {
            provider,
            provider_ref: format!("{provider}-ref"),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        };
        let mut c = PodcastCandidate::new(title, identity);
        c.attribute_all_to(provider);
        c
    }

    fn members() -> Vec<PodcastCandidate> {
        let mut apple = cand(ProviderId::APPLE, "Darknet Diaries");
        apple.artwork = Some(Url::parse("https://apple.test/600.jpg").unwrap());
        apple.feed_url = Some(Url::parse("https://old.test/feed").unwrap());
        apple.categories = vec!["Technology".into(), "True Crime".into()];
        apple.description = Some("short".into());
        apple.itunes_id = Some(1);
        let mut pi = cand(ProviderId::PODCAST_INDEX, "Darknet Diaries");
        pi.artwork = Some(Url::parse("https://pi.test/art.jpg").unwrap());
        pi.feed_url = Some(Url::parse("https://feeds.megaphone.fm/darknetdiaries").unwrap());
        pi.website = Some(Url::parse("https://darknetdiaries.com").unwrap());
        pi.description = Some("a much longer description here".into());
        pi.categories = vec!["technology".into(), "Society".into()];
        pi.podcast_guid = Some("guid".into());
        pi.health.dead = Some(false);
        let mut gp = cand(ProviderId::GPODDER_NET, "Darknet Diaries");
        gp.author = Some("Jack Rhysider".into());
        gp.popularity.push(Popularity {
            provider: ProviderId::GPODDER_NET,
            raw: 500.0,
            normalized: 0.5,
            label: "subscribers".into(),
        });
        vec![apple, pi, gp]
    }

    #[test]
    fn field_rules_and_provenance() {
        let m = merge(members(), &trust());
        assert_eq!(
            m.primary_provider(),
            Some(ProviderId::PODCAST_INDEX),
            "highest trust first"
        );
        assert_eq!(
            m.feed_url.as_ref().map(Url::as_str),
            Some("https://feeds.megaphone.fm/darknetdiaries")
        );
        assert_eq!(
            m.provenance.get("feed_url"),
            Some(&ProviderId::PODCAST_INDEX)
        );
        assert_eq!(
            m.artwork.as_ref().map(Url::as_str),
            Some("https://apple.test/600.jpg"),
            "artwork prefers Apple"
        );
        assert_eq!(m.provenance.get("artwork"), Some(&ProviderId::APPLE));
        assert_eq!(m.author.as_deref(), Some("Jack Rhysider"));
        assert_eq!(m.provenance.get("author"), Some(&ProviderId::GPODDER_NET));
        assert_eq!(
            m.description.as_deref(),
            Some("a much longer description here")
        );
        assert_eq!(m.categories, vec!["technology", "Society", "True Crime"]);
        assert_eq!(m.identities.len(), 3);
        assert_eq!(m.popularity.len(), 1);
        assert_eq!(m.itunes_id, Some(1));
        assert_eq!(m.podcast_guid.as_deref(), Some("guid"));
        assert_eq!(m.health.dead, Some(false));
        assert_eq!(
            m.providers(),
            vec![
                ProviderId::PODCAST_INDEX,
                ProviderId::APPLE,
                ProviderId::GPODDER_NET
            ]
        );
    }

    #[test]
    fn merge_is_order_independent() {
        let a = merge(members(), &trust());
        let mut rev = members();
        rev.reverse();
        let b = merge(rev, &trust());
        let mut rot = members();
        rot.rotate_left(1);
        let c = merge(rot, &trust());
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn singleton_passes_through() {
        let single = cand(ProviderId::APPLE, "Solo");
        assert_eq!(merge(vec![single.clone()], &trust()), single);
    }
}
