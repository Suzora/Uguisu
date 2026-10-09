//! The service properties that are easiest to lose and hardest to spot:
//! that a pass fills its capacity, that a restart does not stampede, and
//! that rebuilding the archive index leaves the search index alone.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use time::{Duration, OffsetDateTime};
use uguisu_core::ids::PodcastId;
use uguisu_core::schedule;
use uguisu_engine::rebuild::RebuildOptions;
use uguisu_http::CancellationToken;
use uguisu_storage::{podcasts, search};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Adds `count` podcasts, all due now.
async fn library(h: &Harness, count: usize) -> Vec<PodcastId> {
    let mut ids = Vec::new();
    for n in 0..count {
        h.serve_fixture(&format!("/feed{n}.xml"), "minimal_rss.xml")
            .await;
        let added = h
            .engine
            .add_podcast(&h.url(&format!("/feed{n}.xml")), CancellationToken::new())
            .await
            .unwrap();
        ids.push(added.podcast.id);
    }
    let mut tx = h.engine.storage().begin().await.unwrap();
    for id in &ids {
        podcasts::set_next_refresh_at(&mut tx, *id, None, OffsetDateTime::now_utc())
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    ids
}

#[tokio::test]
async fn capacity_is_filled_not_spent() {
    // The exclusion happens in SQL. If it did not — if the query returned
    // `LIMIT capacity` rows and the loop threw away the ones it was
    // already refreshing — this pass would start one podcast instead of
    // three, and the rest would wait for the next tick.
    let h = Harness::new().await;
    let ids = library(&h, 4).await;

    // One podcast is already being refreshed by somebody else, and stays
    // that way: its host answers slowly, so the refresh is still in
    // flight when the pass looks. A `while` on the coalescer's counter
    // would be a race — a local mock can finish before the first check.
    h.reset().await;
    for n in 1..4 {
        h.serve_fixture(&format!("/feed{n}.xml"), "minimal_rss.xml")
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/feed0.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(common::fixture("minimal_rss.xml"))
                .insert_header("content-type", "application/rss+xml")
                .set_delay(std::time::Duration::from_secs(3)),
        )
        .mount(&h.server)
        .await;

    let engine = h.engine.clone();
    let busy = ids[0];
    let inflight = tokio::spawn(async move {
        engine
            .refresh_podcast(busy, uguisu_engine::RefreshOptions::default())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while h.engine.coordinator().inflight_count() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the refresh never reached the coalescer");

    let pass = h.engine.run_scheduler_pass(3).await.unwrap();
    assert_eq!(
        (pass.due, pass.started),
        (3, 3),
        "three free slots must start three other podcasts, not two"
    );
    let _ = inflight.await;
}

#[tokio::test]
async fn a_restart_does_not_stampede() {
    // Every write of `next_refresh_at` goes through `plan_next`, so a
    // library that came back from a week of downtime drains at the
    // scheduler's capacity and then spreads itself out. Without the
    // catch-up spread, all of them would land in the same six-minute
    // window for ever.
    let h = Harness::new().await;
    let ids = library(&h, 6).await;
    // Pretend they were all last due a week ago: maximally overdue.
    let long_ago = OffsetDateTime::now_utc() - Duration::days(7);
    let mut tx = h.engine.storage().begin().await.unwrap();
    for id in &ids {
        podcasts::set_next_refresh_at(&mut tx, *id, Some(long_ago), OffsetDateTime::now_utc())
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    // Two at a time, which is what makes it a queue rather than a herd.
    let pass = h.engine.run_scheduler_pass(2).await.unwrap();
    assert_eq!(pass.started, 2);
    // The scheduler's own set, not the coalescer's: a slot is claimed
    // synchronously inside the pass, while the coalescer only learns
    // about the refresh once the spawned task is first polled. Waiting on
    // the latter can observe zero before anything has started.
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while h.engine.scheduler_status().await.unwrap().inflight > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the refreshes never finished");

    // The two that ran are now spread across the interval instead of
    // sharing a due time.
    let mut reader = h.engine.storage().reader().await.unwrap();
    let mut planned = Vec::new();
    for id in &ids {
        if let Some(at) = podcasts::get(&mut reader, *id)
            .await
            .unwrap()
            .unwrap()
            .next_refresh_at
            && at > OffsetDateTime::now_utc()
        {
            planned.push(at);
        }
    }
    assert_eq!(planned.len(), 2, "only what ran was replanned");
    let interval = h.engine.config().feed.refresh_interval;
    for at in &planned {
        assert!(
            *at <= OffsetDateTime::now_utc()
                + Duration::seconds(2 * interval.as_secs().cast_signed())
        );
    }
    // And the arithmetic that produced them is the one the tests pin.
    let slots: Vec<u64> = ids.iter().map(|id| schedule::slot_permille(*id)).collect();
    assert!(
        slots
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1,
        "six podcasts share one slot: {slots:?}"
    );
}

#[tokio::test]
async fn an_archive_rebuild_spares_search() {
    // ADR 0029's claim, tested rather than asserted: `reconcile --rebuild`
    // writes zero FTS rows *because it writes no searchable text*, not
    // because a trigger looks the other way. It reads podcasts and
    // episodes and writes `archive_files`; none of those is a column the
    // `UPDATE OF` clauses name.
    let h = Harness::new().await;
    library(&h, 2).await;
    h.engine.reindex_search().await.unwrap();

    let snapshot = |h: &Harness| {
        let engine = h.engine.clone();
        async move {
            let mut reader = engine.storage().reader().await.unwrap();
            let counts = search::counts(&mut reader).await.unwrap();
            let rows: Vec<(String, String)> =
                sqlx::query_as("SELECT episode_id, title FROM episodes_fts ORDER BY episode_id")
                    .fetch_all(&mut *reader)
                    .await
                    .unwrap();
            (counts, rows)
        }
    };
    let before = snapshot(&h).await;
    assert!(before.0.1 > 0, "nothing was indexed to begin with");

    let report = h
        .engine
        .rebuild_archive(&RebuildOptions {
            apply: true,
            podcast: None,
        })
        .await
        .unwrap();
    assert!(report.applied);

    let after = snapshot(&h).await;
    assert_eq!(
        after, before,
        "a rebuild changed the search index; ADR 0029 says it cannot"
    );
    assert_eq!(
        h.engine.search_index_status().await.unwrap().state,
        uguisu_core::search::IndexState::Ready,
        "and it did not invalidate it either"
    );
}
