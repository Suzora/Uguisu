//! Property tests: normalization, similarity bounds, dedup safety and
//! order-independence of ranking and merging.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use proptest::prelude::*;
use time::OffsetDateTime;
use uguisu_discovery::candidate::{PodcastCandidate, ProviderIdentity};
use uguisu_discovery::fuzzy::{fold_for_match, title_similarity, token_set_ratio};
use uguisu_discovery::query::{NormalizedQuery, ascii_fold, fold_text};
use uguisu_discovery::rank::{RankContext, rank};
use uguisu_discovery::{DedupThresholds, ProviderId, dedup, merge};
use url::Url;

fn candidate(provider: ProviderId, title: &str, feed: Option<&str>) -> PodcastCandidate {
    let identity = ProviderIdentity {
        provider,
        provider_ref: format!("{provider}:{title}"),
        confidence: 1.0,
        url: None,
        fetched_at: OffsetDateTime::UNIX_EPOCH,
    };
    let mut c = PodcastCandidate::new(title, identity);
    c.feed_url = feed.and_then(|f| Url::parse(f).ok());
    c.attribute_all_to(provider);
    c
}

fn title_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "darknet",
            "diaries",
            "the",
            "daily",
            "hack",
            "gemischtes",
            "dark",
            "net",
            "show",
            "cast",
            "radio",
            "hour",
        ]),
        1..4,
    )
    .prop_map(|w| w.join(" "))
}

proptest! {
    #[test]
    fn folding_is_idempotent(s in "\\PC{0,40}") {
        let once = fold_text(&s);
        prop_assert_eq!(fold_text(&once), once.clone());
        let a = ascii_fold(&once);
        prop_assert_eq!(ascii_fold(&a), a.clone());
        prop_assert!(!a.contains("  "));
    }

    #[test]
    fn similarities_are_bounded_and_symmetric(a in "[a-z ]{0,30}", b in "[a-z ]{0,30}") {
        let fa = fold_for_match(&a);
        let fb = fold_for_match(&b);
        for f in [token_set_ratio, title_similarity] {
            let x = f(&fa, &fb);
            let y = f(&fb, &fa);
            prop_assert!((0.0..=1.0).contains(&x), "{x}");
            prop_assert!((x - y).abs() < 1e-5, "{x} vs {y}");
        }
        prop_assert!((title_similarity(&fa, &fa) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ranking_order_is_independent_of_input_order(titles in prop::collection::vec(title_strategy(), 1..8), seed in any::<u64>()) {
        let query = NormalizedQuery::parse("darknet diaries");
        let ctx = RankContext { trust: HashMap::from([(ProviderId::APPLE, 0.8)]), now: Some(OffsetDateTime::UNIX_EPOCH), ..RankContext::default() };
        let cands: Vec<_> = titles.iter().enumerate().map(|(i, t)| (candidate(ProviderId::APPLE, &format!("{t} {i}"), Some(&format!("https://x.test/{i}"))), vec![])).collect();
        let mut shuffled = cands.clone();
        // deterministic shuffle from the seed
        let mut s = seed;
        for i in (1..shuffled.len()).rev() {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            let j = usize::try_from(s % (i as u64 + 1)).unwrap_or(0);
            shuffled.swap(i, j);
        }
        let a: Vec<String> = rank(&query, cands, &ctx).into_iter().map(|r| r.candidate.title).collect();
        let b: Vec<String> = rank(&query, shuffled, &ctx).into_iter().map(|r| r.candidate.title).collect();
        prop_assert_eq!(a, b);
    }

    #[test]
    fn dedup_needs_real_evidence(titles in prop::collection::vec(title_strategy(), 2..6)) {
        // Distinct feed URLs, no authors, no websites, no ids: only exact folded titles could merge, and they don't (title alone is never enough).
        let cands: Vec<_> = titles.iter().enumerate().map(|(i, t)| candidate(ProviderId::APPLE, t, Some(&format!("https://x.test/{i}")))).collect();
        let groups = dedup::group(cands, &DedupThresholds::default());
        prop_assert_eq!(groups.len(), titles.len());
        for g in &groups {
            prop_assert!(g.reasons.is_empty());
        }
    }

    #[test]
    fn merge_is_order_independent(n in 1usize..4) {
        let trust = HashMap::from([(ProviderId::PODCAST_INDEX, 0.9), (ProviderId::APPLE, 0.8), (ProviderId::GPODDER_NET, 0.6)]);
        let providers = [ProviderId::APPLE, ProviderId::PODCAST_INDEX, ProviderId::GPODDER_NET];
        let members: Vec<_> = providers.iter().take(n).map(|p| {
            let mut c = candidate(*p, "Same Show", Some("https://same.test/feed"));
            c.description = Some(format!("description from {p}"));
            c
        }).collect();
        let a = merge::merge(members.clone(), &trust);
        let mut rev = members;
        rev.reverse();
        let b = merge::merge(rev, &trust);
        prop_assert_eq!(a, b);
    }
}
