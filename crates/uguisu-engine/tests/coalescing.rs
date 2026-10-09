//! Refresh coalescing and the bounded `refresh_all` (ADR 0016).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Harness, fixture};
use uguisu_core::UguisuError;
use uguisu_core::feed::RefreshOutcome;
use uguisu_core::ids::PodcastId;
use uguisu_engine::RefreshOptions;
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn concurrent_refreshes_share_one_fetch() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fixture("episodes_v2.xml"))
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&h.server)
        .await;
    let mut handles = Vec::new();
    for i in 0..10 {
        let engine = h.engine.clone();
        handles.push(tokio::spawn(async move {
            engine
                .refresh_podcast(
                    id,
                    RefreshOptions {
                        force: i % 2 == 0,
                        cancel: None,
                    },
                )
                .await
        }));
    }
    let mut fetch_ids = Vec::new();
    for hd in handles {
        let r = hd.await.unwrap().unwrap();
        assert_eq!(r.outcome, RefreshOutcome::Fetched);
        fetch_ids.push(r.fetch_id);
    }
    fetch_ids.dedup();
    assert_eq!(fetch_ids.len(), 1, "every caller got the same run");
    assert_eq!(
        h.server.received_requests().await.unwrap().len(),
        1,
        "one fetch for ten callers"
    );
    assert_eq!(h.engine.fetch_log(id, 10).await.unwrap().len(), 2);
    assert_eq!(h.engine.coordinator().inflight_count(), 0);
    // A later refresh is a fresh run.
    let r = h.refresh(id, true).await;
    assert_ne!(r.fetch_id, fetch_ids[0]);
}

#[tokio::test]
async fn different_podcasts_refresh_in_parallel() {
    const DELAY: Duration = Duration::from_secs(1);
    let h = Harness::new().await;
    h.serve("/a.xml", &fixture("episodes_v1.xml"), None, None)
        .await;
    h.serve("/b.xml", &fixture("missing_guid.xml"), None, None)
        .await;
    let a = h
        .engine
        .add_podcast(&h.url("/a.xml"), CancellationToken::new())
        .await
        .unwrap();
    let b = h
        .engine
        .add_podcast(&h.url("/b.xml"), CancellationToken::new())
        .await
        .unwrap();
    h.reset().await;
    for p in ["/a.xml", "/b.xml"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(fixture(if p == "/a.xml" {
                        "episodes_v1.xml"
                    } else {
                        "missing_guid.xml"
                    }))
                    .set_delay(DELAY),
            )
            .mount(&h.server)
            .await;
    }
    let started = std::time::Instant::now();
    let (ra, rb) = tokio::join!(
        h.engine.refresh_podcast(
            a.podcast.id,
            RefreshOptions {
                force: true,
                cancel: None
            }
        ),
        h.engine.refresh_podcast(
            b.podcast.id,
            RefreshOptions {
                force: true,
                cancel: None
            }
        ),
    );
    assert_eq!(ra.unwrap().outcome, RefreshOutcome::Fetched);
    assert_eq!(rb.unwrap().outcome, RefreshOutcome::Fetched);
    // One after the other they would take at least both delays.
    assert!(
        started.elapsed() < 2 * DELAY,
        "ran in parallel: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_dropped_awaiter_keeps_the_run() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    let id = added.podcast.id;
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fixture("episodes_v2.xml"))
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&h.server)
        .await;
    let engine = h.engine.clone();
    let task = tokio::spawn(async move {
        engine
            .refresh_podcast(
                id,
                RefreshOptions {
                    force: true,
                    cancel: None,
                },
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    task.abort();
    assert!(task.await.is_err());
    assert_eq!(h.engine.coordinator().inflight_count(), 1, "still running");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(h.engine.coordinator().inflight_count(), 0);
    let log = h.engine.fetch_log(id, 1).await.unwrap();
    assert_eq!(log[0].outcome, RefreshOutcome::Fetched);
    assert_eq!(log[0].episodes.added, 1, "the run completed and persisted");
}

#[tokio::test]
async fn errors_reach_every_awaiter() {
    let h = Harness::new().await;
    let missing = PodcastId::new();
    let (a, b) = tokio::join!(
        h.engine.refresh_podcast(missing, RefreshOptions::default()),
        h.engine.refresh_podcast(missing, RefreshOptions::default()),
    );
    assert!(matches!(a.unwrap_err(), UguisuError::NotFound { .. }));
    assert!(matches!(b.unwrap_err(), UguisuError::NotFound { .. }));

    let mut ids = Vec::new();
    for i in 0..5 {
        let p = format!("/p{i}.xml");
        h.serve(&p, &fixture("episodes_v1.xml"), None, None).await;
        let added = h
            .engine
            .add_podcast(&h.url(&p), CancellationToken::new())
            .await
            .unwrap();
        ids.push(added.podcast.id);
    }
    h.reset().await;
    for i in 0..5 {
        let template = if i == 2 {
            ResponseTemplate::new(500)
        } else {
            ResponseTemplate::new(200)
                .set_body_bytes(fixture("episodes_v2.xml"))
                .set_delay(Duration::from_millis(200))
        };
        Mock::given(method("GET"))
            .and(path(format!("/p{i}.xml")))
            .respond_with(template)
            .mount(&h.server)
            .await;
    }
    let entries = h
        .engine
        .refresh_all(true, 2, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(entries.len(), 5);
    let mut failed = 0;
    for e in &entries {
        assert!(ids.contains(&e.podcast_id));
        assert_eq!(e.title, "Versioned Show");
        match &e.result().unwrap().outcome {
            RefreshOutcome::Fetched => {}
            RefreshOutcome::Failed { .. } => failed += 1,
            other @ RefreshOutcome::NotModified { .. } => panic!("{other:?}"),
        }
    }
    assert_eq!(failed, 1);
    assert_eq!(h.engine.coordinator().inflight_count(), 0);

    // A cancelled run reports cancellation instead of fetching.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let entries = h.engine.refresh_all(true, 8, cancel).await.unwrap();
    assert!(
        entries
            .iter()
            .all(|e| matches!(e.error, Some(UguisuError::Cancelled(_))))
    );
}
