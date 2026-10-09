//! Removal semantics (ADR 0017): streaks, the mass-change guard, truncated
//! and partial fetches, and the `max_items` cap — on the pure planner and
//! through the engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use common::{Harness, fixture, synthetic_feed, synthetic_feed_range};
use time::OffsetDateTime;
use uguisu_core::config::{FeedConfig, FeedLimits};
use uguisu_core::feed::RefreshOutcome;
use uguisu_core::ids::PodcastId;
use uguisu_core::model::{FeedKind, Podcast, PodcastStatus};
use uguisu_engine::sync::{SyncRules, plan};
use uguisu_storage::episodes::EpisodeIndex;

fn podcast() -> Podcast {
    let now = OffsetDateTime::UNIX_EPOCH;
    Podcast {
        id: PodcastId::new(),
        title: "Synthetic Show".into(),
        sort_title: "synthetic show".into(),
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
        podcast_guid: None,
        feed_kind: FeedKind::Rss2,
        status: PodcastStatus::Active,
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

/// The stored index a first import of `synthetic_feed(count)` would leave.
fn stored_index(count: usize, streak: u32) -> Vec<EpisodeIndex> {
    let parsed = uguisu_feed::parse(&synthetic_feed(count), &FeedLimits::default()).unwrap();
    let p = plan(
        &podcast(),
        &parsed,
        &[],
        SyncRules {
            removal_streak: 2,
            mass_removal_guard_percent: 50,
        },
        OffsetDateTime::now_utc(),
    );
    p.upserts
        .iter()
        .map(|e| EpisodeIndex {
            id: e.id,
            identity_key: e.identity.key.clone(),
            identity_source: e.identity.source,
            guid_key: e.identity.guid_key.clone(),
            enclosure_key: e.identity.enclosure_key.clone(),
            fingerprint_key: e.identity.fingerprint_key.clone(),
            content_hash: e.content_hash.clone(),
            title: e.title.clone(),
            missing_streak: streak,
            removed_from_feed_at: None,
            duplicate_of_episode_id: None,
            malformed: false,
            first_seen_at: e.first_seen_at,
            published_at: e.published_at,
        })
        .collect()
}

const RULES: SyncRules = SyncRules {
    removal_streak: 2,
    mass_removal_guard_percent: 50,
};

#[test]
fn the_planner_removes_at_the_streak() {
    let index = stored_index(10, 0);
    let now = OffsetDateTime::now_utc();
    // Two of ten missing: below the guard, streak becomes 1.
    let parsed = uguisu_feed::parse(&synthetic_feed_range(0..8), &FeedLimits::default()).unwrap();
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert_eq!(p.counts.unchanged, 8);
    assert_eq!(p.missing.len(), 2);
    assert!(p.removed.is_empty());
    assert!(p.removal_suppressed.is_none());
    // Same two missing again with streak 1 stored: removal at streak 2.
    let index = stored_index(10, 1);
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert_eq!(p.missing.len(), 2);
    assert_eq!(p.removed.len(), 2);
    assert!(p.removed.iter().all(|(_, s)| *s == 2));
    assert_eq!(p.counts.removed_detected, 2);
    // A streak threshold of 1 removes on the first miss.
    let p = plan(
        &podcast(),
        &parsed,
        &stored_index(10, 0),
        SyncRules {
            removal_streak: 1,
            mass_removal_guard_percent: 50,
        },
        now,
    );
    assert_eq!(p.removed.len(), 2);
}

#[test]
fn planner_suppresses_mass_removal_and_incomplete_fetches() {
    let index = stored_index(10, 1);
    let now = OffsetDateTime::now_utc();
    // 6 of 10 missing (> 50 %): nothing is touched, warning recorded.
    let parsed = uguisu_feed::parse(&synthetic_feed_range(0..4), &FeedLimits::default()).unwrap();
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert!(p.missing.is_empty());
    assert!(p.removed.is_empty());
    assert!(p.removal_suppressed.as_deref().unwrap().contains("6 of 10"));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.starts_with("removal_suppressed_mass_change"))
    );
    // Exactly 50 % is allowed.
    let parsed = uguisu_feed::parse(&synthetic_feed_range(0..5), &FeedLimits::default()).unwrap();
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert_eq!(p.missing.len(), 5);
    assert_eq!(p.removed.len(), 5);
    // Guard at 100 % disables the guard.
    let parsed = uguisu_feed::parse(&synthetic_feed_range(0..1), &FeedLimits::default()).unwrap();
    let p = plan(
        &podcast(),
        &parsed,
        &index,
        SyncRules {
            removal_streak: 2,
            mass_removal_guard_percent: 100,
        },
        now,
    );
    assert_eq!(p.removed.len(), 9);
    // A truncated fetch never touches streaks.
    let limits = FeedLimits {
        max_items: 3,
        ..FeedLimits::default()
    };
    let parsed = uguisu_feed::parse(&synthetic_feed(10), &limits).unwrap();
    assert!(parsed.truncated);
    assert_eq!(parsed.items.len(), 3);
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert_eq!(p.counts.unchanged, 3);
    assert!(p.missing.is_empty());
    assert!(
        p.removal_suppressed
            .as_deref()
            .unwrap()
            .contains("truncated")
    );
    // A fetch with a malformed item neither.
    let parsed =
        uguisu_feed::parse(&fixture("malformed_item.xml"), &FeedLimits::default()).unwrap();
    assert!(parsed.is_partial());
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert!(p.missing.is_empty());
    assert!(
        p.removal_suppressed
            .as_deref()
            .unwrap()
            .contains("malformed")
    );
    // Episodes already detected as removed are left alone (no double counting).
    let mut index = stored_index(10, 2);
    for e in &mut index[8..] {
        e.removed_from_feed_at = Some(now);
    }
    let parsed = uguisu_feed::parse(&synthetic_feed_range(0..7), &FeedLimits::default()).unwrap();
    let p = plan(&podcast(), &parsed, &index, RULES, now);
    assert_eq!(p.missing.len(), 1, "only the newly missing one");
    assert_eq!(p.removed.len(), 1);
}

#[tokio::test]
async fn the_engine_applies_streaks_and_guards() {
    let h = Harness::with_feed_config(FeedConfig {
        removal_streak: 3,
        mass_removal_guard_percent: 50,
        ..FeedConfig::default()
    })
    .await;
    h.serve("/feed.xml", &synthetic_feed(20), None, None).await;
    let added = h
        .engine
        .add_podcast(&h.url("/feed.xml"), uguisu_http::CancellationToken::new())
        .await
        .unwrap();
    let id = added.podcast.id;
    assert_eq!(added.report.unwrap().episodes.added, 20);

    // Five episodes disappear: streak climbs to 3 over three complete fetches.
    h.reset().await;
    h.serve("/feed.xml", &synthetic_feed_range(0..15), None, None)
        .await;
    for expected_streak in 1..=3u32 {
        let r = h.refresh(id, true).await;
        assert_eq!(r.outcome, RefreshOutcome::Fetched);
        let eps = h.engine.episodes(id, None, 100).await.unwrap().episodes;
        let missing: Vec<_> = eps
            .iter()
            .filter(|e| {
                e.guid
                    .as_deref()
                    .is_some_and(|g| g.trim_start_matches("ep-").parse::<usize>().unwrap() >= 15)
            })
            .collect();
        assert_eq!(missing.len(), 5);
        assert!(missing.iter().all(|e| e.missing_streak == expected_streak));
        if expected_streak < 3 {
            assert_eq!(r.episodes.removed_detected, 0);
            assert!(missing.iter().all(|e| e.removed_from_feed_at.is_none()));
        } else {
            assert_eq!(r.episodes.removed_detected, 5);
            assert!(missing.iter().all(|e| e.removed_from_feed_at.is_some()));
        }
    }
    let detail = h.engine.podcast(id).await.unwrap();
    assert_eq!((detail.episodes_total, detail.episodes_present), (20, 15));

    // A fetch that drops most of the feed is suppressed and leaves streaks alone.
    h.reset().await;
    h.serve("/feed.xml", &synthetic_feed_range(0..3), None, None)
        .await;
    let r = h.refresh(id, true).await;
    assert!(r.removal_suppressed.is_some(), "{r:?}");
    assert_eq!(r.episodes.removed_detected, 0);
    let eps = h.engine.episodes(id, None, 100).await.unwrap().episodes;
    assert!(
        eps.iter()
            .filter(|e| e.removed_from_feed_at.is_none())
            .all(|e| e.missing_streak == 0)
    );

    // The parser cap makes the fetch truncated: reported, streaks untouched.
    let capped = Harness::with_feed_config(FeedConfig {
        limits: FeedLimits {
            max_items: 5,
            ..FeedLimits::default()
        },
        ..FeedConfig::default()
    })
    .await;
    capped
        .serve("/feed.xml", &synthetic_feed(20), None, None)
        .await;
    let added = capped
        .engine
        .add_podcast(
            &capped.url("/feed.xml"),
            uguisu_http::CancellationToken::new(),
        )
        .await
        .unwrap();
    let r = added.report.unwrap();
    assert!(r.truncated);
    assert!(r.is_partial());
    assert_eq!(r.episodes.added, 5);
    assert!(r.warnings.iter().any(|w| w.contains("truncated")));
    let r = capped.refresh(added.podcast.id, true).await;
    assert_eq!(r.episodes.unchanged, 5);
    assert!(r.removal_suppressed.is_some());
}

#[tokio::test]
async fn a_raised_guard_records_the_shrink() {
    let h = Harness::new().await;
    h.serve("/feed.xml", &synthetic_feed(10), None, None).await;
    let id = h
        .engine
        .add_podcast(&h.url("/feed.xml"), uguisu_http::CancellationToken::new())
        .await
        .unwrap()
        .podcast
        .id;

    // The feed keeps two of its ten items, which the default guard of 50 %
    // suppresses on this refresh and on every later one.
    h.reset().await;
    h.serve("/feed.xml", &synthetic_feed_range(0..2), None, None)
        .await;
    for _ in 0..3 {
        let r = h.refresh(id, true).await;
        assert!(r.removal_suppressed.is_some(), "{r:?}");
        assert_eq!(r.episodes.removed_detected, 0);
    }

    h.engine
        .set_setting("UGUISU_FEED_MASS_REMOVAL_GUARD_PERCENT", "90", Some("test"))
        .await
        .unwrap();
    // The default streak of two complete fetches still applies.
    let first = h.refresh(id, true).await;
    assert!(first.removal_suppressed.is_none(), "{first:?}");
    assert_eq!(first.episodes.removed_detected, 0);
    let second = h.refresh(id, true).await;
    assert_eq!(second.episodes.removed_detected, 8, "{second:?}");
}

#[tokio::test]
async fn ten_thousand_items_import_and_refresh_identically() {
    let h = Harness::new().await;
    let body = synthetic_feed(10_000);
    h.serve("/big.xml", &body, Some("\"big-1\""), None).await;
    let started = std::time::Instant::now();
    let added = h
        .engine
        .add_podcast(&h.url("/big.xml"), uguisu_http::CancellationToken::new())
        .await
        .unwrap();
    let import = started.elapsed();
    let r = added.report.unwrap();
    assert_eq!(
        r.outcome,
        RefreshOutcome::Fetched,
        "{:?}",
        &r.warnings[..r.warnings.len().min(3)]
    );
    assert_eq!(r.episodes.added, 10_000);
    assert_eq!(r.episodes.seen, 10_000);
    assert!(!r.truncated);
    let id = added.podcast.id;

    // Identical body: fingerprint hit without parsing.
    h.reset().await;
    h.serve("/big.xml", &body, None, None).await;
    let r = h.refresh(id, false).await;
    assert!(matches!(r.outcome, RefreshOutcome::NotModified { .. }));

    // Forced: full parse and sync, nothing changes.
    let started = std::time::Instant::now();
    let r = h.refresh(id, true).await;
    let identical = started.elapsed();
    assert_eq!(r.outcome, RefreshOutcome::Fetched);
    assert_eq!(r.episodes.unchanged, 10_000);
    assert_eq!(r.episodes.added + r.episodes.updated, 0);

    // One new episode and one change.
    let mut v2 = synthetic_feed_range(0..10_001);
    let marker = b"Episode 5000: the one";
    let pos = v2.windows(marker.len()).position(|w| w == marker).unwrap();
    v2.splice(
        pos..pos + marker.len(),
        b"Episode 5000: the two".iter().copied(),
    );
    h.reset().await;
    h.serve("/big.xml", &v2, None, None).await;
    let r = h.refresh(id, false).await;
    assert_eq!(r.episodes.added, 1);
    assert_eq!(r.episodes.updated, 1);
    assert_eq!(r.episodes.unchanged, 9_999);

    let mut page = h.engine.episodes(id, None, 500).await.unwrap();
    let mut total = page.episodes.len();
    while let Some(after) = page.next_after {
        page = h.engine.episodes(id, Some(after), 500).await.unwrap();
        total += page.episodes.len();
    }
    assert_eq!(total, 10_001);
    eprintln!("10k import {import:?}, identical forced refresh {identical:?}");
    assert!(import < std::time::Duration::from_secs(60), "{import:?}");
}
