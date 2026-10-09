//! Local search end to end (ADR 0029): what the triggers index, what a
//! rebuild does, and what a search says when it cannot answer properly.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use uguisu_core::search::IndexState;
use uguisu_engine::search::{SearchOutcome, SearchRequest};

#[tokio::test]
async fn a_refresh_makes_its_episodes_findable() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;

    let results = h
        .engine
        .search_library(&SearchRequest::new(&added.podcast.title))
        .await
        .unwrap();
    assert_eq!(results.outcome, SearchOutcome::Ok);
    assert_eq!(results.podcasts[0].hit.podcast_id, added.podcast.id);
    assert!(results.podcasts[0].score > 0.0);

    // Episodes came in through the refresh, so the triggers indexed them
    // without anybody rebuilding anything.
    let page = h.engine.episodes(added.podcast.id, None, 50).await.unwrap();
    let title = page.episodes[0].title.clone();
    let results = h
        .engine
        .search_library(&SearchRequest::new(&title))
        .await
        .unwrap();
    assert!(
        results.episodes.iter().any(|e| e.hit.title == title),
        "{title} is not findable: {:?}",
        results.episodes
    );
    let first = &results.episodes[0];
    assert!(
        (first.score - first.signals.iter().map(|s| s.contribution).sum::<f64>()).abs() < 1e-9,
        "the explanation must add up to the score"
    );
}

#[tokio::test]
async fn a_search_says_why_nothing_matched() {
    let h = Harness::new().await;
    h.add_fixture("episodes_v1.xml").await;

    // A library that has never been indexed says so rather than pretending
    // its empty answer is the whole truth — even though the triggers have
    // in fact indexed everything written since the migration.
    let results = h
        .engine
        .search_library(&SearchRequest::new("supercalifragilistic"))
        .await
        .unwrap();
    assert_eq!(results.outcome, SearchOutcome::IndexStale);
    h.engine.reindex_search().await.unwrap();

    // Nothing to search for: no SQL runs at all.
    let results = h
        .engine
        .search_library(&SearchRequest::new("  *** "))
        .await
        .unwrap();
    assert_eq!(results.outcome, SearchOutcome::EmptyQuery);
    assert!(results.terms.is_empty());

    // A well-formed query with no match.
    let results = h
        .engine
        .search_library(&SearchRequest::new("supercalifragilistic"))
        .await
        .unwrap();
    assert_eq!(results.outcome, SearchOutcome::NoResults);

    // Hostile input is a search, not an error.
    for hostile in ["\"unterminated", "a OR b", "NEAR(x y)", "col:value", "-x"] {
        let results = h
            .engine
            .search_library(&SearchRequest::new(hostile))
            .await
            .unwrap();
        assert!(
            matches!(
                results.outcome,
                SearchOutcome::Ok | SearchOutcome::NoResults
            ),
            "{hostile} produced {:?}",
            results.outcome
        );
    }
}

#[tokio::test]
async fn a_rebuilt_index_says_its_worth() {
    let mut h = Harness::new().await;
    h.add_fixture("episodes_v1.xml").await;
    // Migration 0005 creates the tables but does not fill them, so a
    // library that predates the index starts `stale` — and everything
    // written since was indexed by the triggers regardless.
    assert_eq!(
        h.engine.search_index_status().await.unwrap().state,
        IndexState::Stale
    );

    let report = h.engine.reindex_search().await.unwrap();
    assert!(report.episodes > 0);
    let status = h.engine.search_index_status().await.unwrap();
    assert_eq!(status.state, IndexState::Ready);
    assert_eq!(status.episodes, report.episodes);
    assert!(status.built_at.is_some());

    // Rebuilding twice is not doubling.
    let again = h.engine.reindex_search().await.unwrap();
    assert_eq!(again.episodes, report.episodes);
    assert_eq!(again.podcasts, report.podcasts);

    // The background build is a no-op once the index is ready.
    h.engine.start_search_index();
    h.restart(None).await;
    assert_eq!(
        h.engine.search_index_status().await.unwrap().state,
        IndexState::Ready,
        "a restart does not invalidate a built index"
    );
}

#[tokio::test]
async fn the_background_build_makes_it_ready() {
    let h = Harness::new().await;
    h.add_fixture("episodes_v1.xml").await;
    assert_eq!(
        h.engine.search_index_status().await.unwrap().state,
        IndexState::Stale
    );
    h.engine.start_search_index();
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if h.engine.search_index_status().await.unwrap().state == IndexState::Ready {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the background build never finished");
}
