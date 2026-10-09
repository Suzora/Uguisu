//! Cross-provider deduplication.
//!
//! Candidates are grouped with union-find on strong keys (normalized feed
//! URL, iTunes id, `podcast:guid`) and medium keys (same website host plus
//! very similar title; very similar title plus very similar author).
//! Similar titles below those thresholds are never merged — they become
//! [`AmbiguityNote`]s the UI can show as "possibly the same as …".

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::candidate::PodcastCandidate;
use crate::fuzzy::{fold_for_match, title_similarity};

/// Similarity thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DedupThresholds {
    /// Title similarity required together with a matching author.
    pub title_with_author: f32,
    /// Author similarity required together with a matching title.
    pub author: f32,
    /// Title similarity required together with the same website host.
    pub title_with_website: f32,
    /// Title similarity above which unmerged pairs are flagged as ambiguous.
    pub ambiguity: f32,
}

impl Default for DedupThresholds {
    fn default() -> Self {
        Self {
            title_with_author: 0.95,
            author: 0.9,
            title_with_website: 0.9,
            ambiguity: 0.85,
        }
    }
}

/// Why two candidates were merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeReason {
    /// Same normalized feed URL.
    FeedUrl,
    /// Same iTunes id.
    ItunesId,
    /// Same `podcast:guid`.
    PodcastGuid,
    /// Same website host and near-identical title.
    WebsiteAndTitle,
    /// Near-identical title and author.
    TitleAndAuthor,
}

/// A pair that looked similar but was not merged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AmbiguityNote {
    /// Title of the other candidate.
    pub other_title: String,
    /// Title similarity.
    pub similarity: f32,
    /// Short explanation.
    pub reason: String,
}

/// A merged group.
#[derive(Debug, Clone)]
pub struct DedupGroup {
    /// Member candidates in input order.
    pub members: Vec<PodcastCandidate>,
    /// Reasons that joined members (empty for singletons).
    pub reasons: Vec<MergeReason>,
    /// Similar-but-separate candidates (by title).
    pub ambiguities: Vec<AmbiguityNote>,
}

/// Normalizes a feed URL into a comparison key: scheme dropped, host
/// lowercased, default port removed, tracking parameters removed,
/// trailing slash removed, fragment dropped.
pub fn feed_key(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = url.path().trim_end_matches('/').to_owned();
    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !(k.starts_with("utm_") || k == "ref" || k == "source" || k == "fbclid" || k == "gclid")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    params.sort();
    let query = if params.is_empty() {
        String::new()
    } else {
        format!(
            "?{}",
            params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        )
    };
    format!("{host}{port}{path}{query}")
}

fn website_host(url: &Url) -> String {
    let h = url.host_str().unwrap_or_default().to_ascii_lowercase();
    h.strip_prefix("www.").unwrap_or(&h).to_owned()
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, i: usize) -> usize {
        let mut r = i;
        while self.parent[r] != r {
            r = self.parent[r];
        }
        let mut c = i;
        while self.parent[c] != r {
            let next = self.parent[c];
            self.parent[c] = r;
            c = next;
        }
        r
    }

    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        // Keep the smaller root so output order stays deterministic.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[hi] = lo;
        true
    }
}

/// Groups candidates. Output groups are ordered by the first appearance of
/// any member, so the result is deterministic for a given input order and
/// membership does not depend on it.
#[allow(clippy::too_many_lines)] // key collection, pairwise pass and group assembly belong together
pub fn group(candidates: Vec<PodcastCandidate>, thresholds: &DedupThresholds) -> Vec<DedupGroup> {
    let n = candidates.len();
    let mut uf = UnionFind::new(n);
    let mut reasons: Vec<Vec<MergeReason>> = vec![Vec::new(); n];

    // Strong keys.
    let mut by_feed: HashMap<String, usize> = HashMap::new();
    let mut by_itunes: HashMap<u64, usize> = HashMap::new();
    let mut by_guid: HashMap<String, usize> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        if let Some(f) = &c.feed_url {
            let key = feed_key(f);
            if let Some(&j) = by_feed.get(&key) {
                if uf.union(i, j) {
                    reasons[i].push(MergeReason::FeedUrl);
                }
            } else {
                by_feed.insert(key, i);
            }
        }
        if let Some(id) = c.itunes_id {
            if let Some(&j) = by_itunes.get(&id) {
                if uf.union(i, j) {
                    reasons[i].push(MergeReason::ItunesId);
                }
            } else {
                by_itunes.insert(id, i);
            }
        }
        if let Some(g) = &c.podcast_guid {
            let key = g.trim().to_ascii_lowercase();
            if let Some(&j) = by_guid.get(&key) {
                if uf.union(i, j) {
                    reasons[i].push(MergeReason::PodcastGuid);
                }
            } else {
                by_guid.insert(key, i);
            }
        }
    }

    // Medium keys and ambiguity notes (pairwise on folded titles). A note
    // names the other show as it is titled, read here because the groups
    // below take their members out one group at a time.
    let display: Vec<String> = candidates.iter().map(|c| c.title.clone()).collect();
    let titles: Vec<String> = candidates
        .iter()
        .map(|c| fold_for_match(&c.title))
        .collect();
    let authors: Vec<Option<String>> = candidates
        .iter()
        .map(|c| c.author.as_deref().map(fold_for_match))
        .collect();
    let hosts: Vec<Option<String>> = candidates
        .iter()
        .map(|c| c.website.as_ref().map(website_host))
        .collect();
    let mut ambiguous: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = title_similarity(&titles[i], &titles[j]);
            if sim < thresholds.ambiguity {
                continue;
            }
            let same_host = matches!((&hosts[i], &hosts[j]), (Some(a), Some(b)) if a == b);
            let author_sim = match (&authors[i], &authors[j]) {
                (Some(a), Some(b)) => title_similarity(a, b),
                _ => 0.0,
            };
            let reason = if same_host && sim >= thresholds.title_with_website {
                Some(MergeReason::WebsiteAndTitle)
            } else if sim >= thresholds.title_with_author && author_sim >= thresholds.author {
                Some(MergeReason::TitleAndAuthor)
            } else {
                None
            };
            if let Some(r) = reason {
                if uf.union(i, j) {
                    reasons[j].push(r);
                }
            } else {
                ambiguous[i].push((j, sim));
                ambiguous[j].push((i, sim));
            }
        }
    }

    // Build groups in first-appearance order.
    let mut root_to_group: HashMap<usize, usize> = HashMap::new();
    let mut groups: Vec<(Vec<usize>, Vec<MergeReason>)> = Vec::new();
    for (i, member_reasons) in reasons.iter().enumerate() {
        let root = uf.find(i);
        let gi = *root_to_group.entry(root).or_insert_with(|| {
            groups.push((Vec::new(), Vec::new()));
            groups.len() - 1
        });
        groups[gi].0.push(i);
        groups[gi].1.extend(member_reasons.iter().copied());
    }

    let mut members: Vec<Option<PodcastCandidate>> = candidates.into_iter().map(Some).collect();
    groups
        .into_iter()
        .map(|(indices, mut reasons)| {
            reasons.sort_by_key(|r| *r as u8);
            reasons.dedup();
            let mut notes = Vec::new();
            for &i in &indices {
                for &(j, sim) in &ambiguous[i] {
                    if uf.find(j) != uf.find(i) {
                        let other = &titles[j];
                        if !notes
                            .iter()
                            .any(|n: &AmbiguityNote| fold_for_match(&n.other_title) == *other)
                        {
                            notes.push(AmbiguityNote {
                                other_title: display[j].clone(),
                                similarity: sim,
                                reason: "similar title, but no shared feed, id, website or author"
                                    .to_owned(),
                            });
                        }
                    }
                }
            }
            notes.sort_by(|a, b| {
                b.similarity
                    .partial_cmp(&a.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.other_title.cmp(&b.other_title))
            });
            DedupGroup {
                members: indices.iter().filter_map(|&i| members[i].take()).collect(),
                reasons,
                ambiguities: notes,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::many_single_char_names)]

    use time::OffsetDateTime;
    use uguisu_core::provider::ProviderId;

    use super::*;
    use crate::candidate::ProviderIdentity;

    fn cand(
        provider: ProviderId,
        title: &str,
        feed: Option<&str>,
        author: Option<&str>,
        website: Option<&str>,
    ) -> PodcastCandidate {
        let identity = ProviderIdentity {
            provider,
            provider_ref: title.to_owned(),
            confidence: 1.0,
            url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
        };
        let mut c = PodcastCandidate::new(title, identity);
        c.feed_url = feed.map(|f| Url::parse(f).unwrap());
        c.author = author.map(str::to_owned);
        c.website = website.map(|w| Url::parse(w).unwrap());
        c
    }

    #[test]
    fn feed_keys_normalize_url_noise() {
        let a =
            feed_key(&Url::parse("https://www.Example.com/feed/?utm_source=x&b=2&a=1").unwrap());
        let b = feed_key(&Url::parse("http://example.com:80/feed?a=1&b=2#frag").unwrap());
        assert_eq!(a, b);
        assert_eq!(a, "example.com/feed?a=1&b=2");
        assert_ne!(
            feed_key(&Url::parse("https://example.com/feed").unwrap()),
            feed_key(&Url::parse("https://example.com/feed2").unwrap())
        );
    }

    #[test]
    fn strong_keys_merge_titles_do_not() {
        let cands = vec![
            cand(
                ProviderId::APPLE,
                "Darknet Diaries",
                Some("https://feeds.megaphone.fm/darknetdiaries"),
                Some("Jack Rhysider"),
                None,
            ),
            cand(
                ProviderId::PODCAST_INDEX,
                "Darknet Diaries",
                Some("http://feeds.megaphone.fm/darknetdiaries/"),
                Some("Jack Rhysider"),
                Some("https://darknetdiaries.com"),
            ),
            cand(
                ProviderId::GPODDER_NET,
                "Darknet Diaries Fan Recap",
                Some("https://recap.test/feed"),
                Some("Recap Crew"),
                None,
            ),
            cand(
                ProviderId::GPODDER_NET,
                "Darknet Diary",
                Some("https://other.test/feed"),
                None,
                None,
            ),
        ];
        let groups = group(cands, &DedupThresholds::default());
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].reasons, vec![MergeReason::FeedUrl]);
        assert!(
            groups[0]
                .ambiguities
                .iter()
                .any(|n| n.other_title == "Darknet Diary"),
            "{:?}",
            groups[0].ambiguities
        );
        assert_eq!(groups[2].members[0].title, "Darknet Diary");
        assert!(
            groups[2]
                .ambiguities
                .iter()
                .any(|n| n.other_title == "Darknet Diaries"),
            "a note on a later group names an earlier one as titled: {:?}",
            groups[2].ambiguities
        );
    }

    #[test]
    fn itunes_id_guid_website_and_author_rules() {
        let mut a = cand(ProviderId::APPLE, "The Show", None, None, None);
        a.itunes_id = Some(42);
        let mut b = cand(
            ProviderId::PODCAST_INDEX,
            "The Show (Official)",
            Some("https://x.test/f"),
            None,
            None,
        );
        b.itunes_id = Some(42);
        let mut c = cand(
            ProviderId::PODCAST_INDEX,
            "Totally Different",
            Some("https://y.test/f"),
            None,
            None,
        );
        c.podcast_guid = Some("GUID-1".into());
        let mut d = cand(
            ProviderId::GPODDER_NET,
            "Another Name",
            Some("https://z.test/f"),
            None,
            None,
        );
        d.podcast_guid = Some("guid-1".into());
        let e = cand(
            ProviderId::APPLE,
            "History Hour",
            None,
            None,
            Some("https://www.history.test/"),
        );
        let f = cand(
            ProviderId::GPODDER_NET,
            "History Hour",
            Some("https://history.test/rss"),
            None,
            Some("http://history.test/podcast"),
        );
        let g = cand(
            ProviderId::APPLE,
            "Morning Brew",
            None,
            Some("Brew Team"),
            None,
        );
        let h = cand(
            ProviderId::GPODDER_NET,
            "Morning Brew",
            Some("https://brew.test/rss"),
            Some("brew team"),
            None,
        );
        let groups = group(vec![a, b, c, d, e, f, g, h], &DedupThresholds::default());
        let titles: Vec<Vec<String>> = groups
            .iter()
            .map(|g| g.members.iter().map(|m| m.title.clone()).collect())
            .collect();
        assert_eq!(groups.len(), 4, "{titles:?}");
        assert_eq!(groups[0].reasons, vec![MergeReason::ItunesId]);
        assert_eq!(groups[1].reasons, vec![MergeReason::PodcastGuid]);
        assert_eq!(groups[2].reasons, vec![MergeReason::WebsiteAndTitle]);
        assert_eq!(groups[3].reasons, vec![MergeReason::TitleAndAuthor]);
    }

    #[test]
    fn membership_is_order_independent() {
        let base = vec![
            cand(
                ProviderId::APPLE,
                "Darknet Diaries",
                Some("https://feeds.megaphone.fm/darknetdiaries"),
                Some("Jack Rhysider"),
                None,
            ),
            cand(
                ProviderId::GPODDER_NET,
                "Darknet Diaries",
                Some("http://feeds.megaphone.fm/darknetdiaries"),
                Some("Jack Rhysider"),
                None,
            ),
            cand(
                ProviderId::PODCAST_INDEX,
                "Darknet Diary",
                Some("https://other.test/feed"),
                None,
                None,
            ),
        ];
        let a = group(base.clone(), &DedupThresholds::default());
        let mut rev = base;
        rev.reverse();
        let b = group(rev, &DedupThresholds::default());
        let sizes = |g: &[DedupGroup]| {
            let mut s: Vec<usize> = g.iter().map(|x| x.members.len()).collect();
            s.sort_unstable();
            s
        };
        assert_eq!(sizes(&a), sizes(&b));
    }
}
