//! Discovery that outlives the process (ADR 0030).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use uguisu_core::provider::ResolutionOutcome;
use uguisu_http::CancellationToken;

#[tokio::test]
async fn adding_a_podcast_records_its_resolution() {
    let mut h = Harness::new().await;
    let added = h.add_fixture("minimal_rss.xml").await;

    let records = h.engine.discovery_records(10).await.unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.status, ResolutionOutcome::Resolved);
    assert_eq!(record.input, h.url("/feed.xml"));
    assert_eq!(
        record.feed_url.as_deref(),
        Some(h.url("/feed.xml").as_str())
    );
    assert_eq!(record.podcast_id, Some(added.podcast.id));
    assert!(!record.steps.is_empty(), "the provenance is the point");

    // It survives a restart, and it is provenance rather than a source:
    // nothing re-reads it as a feed.
    h.restart(None).await;
    let again = h.engine.discovery_record(record.id).await.unwrap();
    assert_eq!(&again, record);

    // Deleting the podcast keeps the record, with the link cleared —
    // `ON DELETE SET NULL`, because provenance outlives what it produced.
    // (The library has no delete command yet; this is the row going away
    // by whatever route later provides one.)
    let mut tx = h.engine.storage().begin().await.unwrap();
    sqlx::query("DELETE FROM podcasts WHERE id = ?1")
        .bind(added.podcast.id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let after = h.engine.discovery_record(record.id).await.unwrap();
    assert_eq!(after.podcast_id, None);
    assert_eq!(after.feed_url, record.feed_url);
}

#[tokio::test]
async fn a_resolution_that_failed_is_recorded_too() {
    let h = Harness::new().await;
    let err = h
        .engine
        .resolve_input(&h.url("/nothing.xml"), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());

    let records = h.engine.discovery_records(10).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, ResolutionOutcome::Unresolved);
    assert!(records[0].feed_url.is_none());
    assert!(records[0].detail.is_some(), "why it failed is the point");
    assert!(records[0].podcast_id.is_none());
}

#[tokio::test]
async fn the_provider_cache_survives_a_restart() {
    // The cache's second tier is the database, so what a provider said
    // before a restart is still known after one. Nothing here reaches a
    // provider: the test writes and reads the store directly, which is
    // the part that is new.
    let mut h = Harness::new().await;
    let store = h.engine.cache_store();
    let value = uguisu_discovery::cache::CachedValue::Search(vec![]);
    let now = time::OffsetDateTime::now_utc();
    store
        .put(
            uguisu_core::provider::ProviderId::APPLE,
            uguisu_discovery::cache::CacheKind::Search,
            "rust|25|",
            &value,
            now,
            now + time::Duration::hours(1),
        )
        .await;

    h.restart(None).await;
    let store = h.engine.cache_store();
    let back = store
        .get(
            uguisu_core::provider::ProviderId::APPLE,
            uguisu_discovery::cache::CacheKind::Search,
            "rust|25|",
        )
        .await;
    assert!(back.is_some(), "the cache did not survive the restart");

    // An expired row is a miss, and the maintenance pass removes it.
    let store = h.engine.cache_store();
    store
        .put(
            uguisu_core::provider::ProviderId::APPLE,
            uguisu_discovery::cache::CacheKind::Lookup,
            "itunes:1",
            &value,
            now - time::Duration::hours(2),
            now - time::Duration::hours(1),
        )
        .await;
    assert!(
        store
            .get(
                uguisu_core::provider::ProviderId::APPLE,
                uguisu_discovery::cache::CacheKind::Lookup,
                "itunes:1",
            )
            .await
            .is_none(),
        "an expired row must never be served"
    );
    let report = h.engine.run_maintenance().await.unwrap();
    assert_eq!(report.cache_expired, 1);
}
