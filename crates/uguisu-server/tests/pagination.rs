//! The cursor contract, on every route that has one (ADR 0040).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::App;

const FEED: &str = include_str!("../../../tests/fixtures/feeds/parser/episodes_v1.xml");

/// Adds a podcast and returns its id and its episode ids, newest first.
async fn library(t: &App) -> (String, Vec<String>) {
    t.serve("/feed.xml", FEED).await;
    let added = t
        .send(
            "POST",
            "/api/v1/podcasts",
            Some(serde_json::json!({ "input": format!("{}/feed.xml", t.server.uri()) })),
        )
        .await;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.json);
    let podcast = added.json["podcast"]["id"].as_str().unwrap().to_owned();
    let listed = t
        .send("GET", &format!("/api/v1/podcasts/{podcast}/episodes"), None)
        .await;
    let episodes = listed.json["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    (podcast, episodes)
}

/// Every route that takes a cursor.
fn paged(podcast: &str) -> Vec<String> {
    vec![
        format!("/api/v1/podcasts/{podcast}/episodes"),
        "/api/v1/episodes/duplicates".to_owned(),
        "/api/v1/downloads".to_owned(),
        "/api/v1/podcasts".to_owned(),
        "/api/v1/archive".to_owned(),
        "/api/v1/archive/missing".to_owned(),
        "/api/v1/archive/invalid".to_owned(),
    ]
}

#[tokio::test]
async fn a_limit_outside_the_range_is_refused_everywhere() {
    let t = App::open().await;
    let (podcast, _) = library(&t).await;
    for uri in paged(&podcast) {
        for bad in ["0", "501", "99999"] {
            let r = t.send("GET", &format!("{uri}?limit={bad}"), None).await;
            assert_eq!(
                r.status,
                StatusCode::BAD_REQUEST,
                "{uri}?limit={bad} answered {}: {}",
                r.status,
                r.json
            );
            assert_eq!(r.json["error"]["kind"], "invalid", "{uri}?limit={bad}");
        }
        // Not a number at all is refused by the extractor, in the same shape.
        let r = t.send("GET", &format!("{uri}?limit=abc"), None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{uri}?limit=abc");
        assert_eq!(r.json["error"]["kind"], "invalid", "{uri}?limit=abc");
        // The edges of the range are fine.
        for good in ["1", "500"] {
            let r = t.send("GET", &format!("{uri}?limit={good}"), None).await;
            assert_eq!(r.status, StatusCode::OK, "{uri}?limit={good}: {}", r.json);
        }
    }
    t.close().await;
}

#[tokio::test]
async fn an_unknown_cursor_is_a_bad_request_everywhere() {
    let t = App::open().await;
    let (podcast, _) = library(&t).await;
    for uri in paged(&podcast) {
        // A well-formed id that names nothing.
        let ulid = uguisu_core::ids::EpisodeId::new();
        let r = t.send("GET", &format!("{uri}?after={ulid}"), None).await;
        assert_eq!(
            r.status,
            StatusCode::BAD_REQUEST,
            "{uri}?after={ulid} answered {}: {}",
            r.status,
            r.json
        );
        assert_eq!(r.json["error"]["kind"], "invalid", "{uri}");
        // And one that is not an id at all.
        let r = t.send("GET", &format!("{uri}?after=nonsense"), None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{uri}?after=nonsense");
    }
    t.close().await;
}

/// A cursor from another podcast's list must not silently offset this one.
#[tokio::test]
async fn a_cursor_from_elsewhere_is_refused() {
    let t = App::open().await;
    let (podcast, episodes) = library(&t).await;
    let elsewhere = uguisu_core::ids::PodcastId::new();
    let r = t
        .send(
            "GET",
            &format!(
                "/api/v1/podcasts/{elsewhere}/episodes?after={}",
                episodes[0]
            ),
            None,
        )
        .await;
    assert!(
        r.status == StatusCode::NOT_FOUND || r.status == StatusCode::BAD_REQUEST,
        "{}: {}",
        r.status,
        r.json
    );
    // The cursor is valid for the podcast it came from.
    let ok = t
        .send(
            "GET",
            &format!("/api/v1/podcasts/{podcast}/episodes?after={}", episodes[0]),
            None,
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.json);
    t.close().await;
}

/// The old bug: a page that happened to be exactly full reported a cursor, and
/// the client had to make one more request to learn there was nothing after it.
#[tokio::test]
async fn an_exactly_full_last_page_has_no_cursor() {
    let t = App::open().await;
    let (podcast, episodes) = library(&t).await;
    let total = episodes.len();
    assert!(total >= 3, "the fixture needs a few episodes: {total}");

    let uri = format!("/api/v1/podcasts/{podcast}/episodes?limit={total}");
    let r = t.send("GET", &uri, None).await;
    assert_eq!(r.json["episodes"].as_array().unwrap().len(), total);
    assert!(
        r.json["next_after"].is_null(),
        "a page holding everything must not offer a cursor: {}",
        r.json
    );

    // And a walk in smaller steps still ends exactly once.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..total + 2 {
        let step = match &cursor {
            None => format!("/api/v1/podcasts/{podcast}/episodes?limit=1"),
            Some(after) => format!("/api/v1/podcasts/{podcast}/episodes?limit=1&after={after}"),
        };
        let r = t.send("GET", &step, None).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.json);
        for e in r.json["episodes"].as_array().unwrap() {
            seen.push(e["id"].as_str().unwrap().to_owned());
        }
        match r.json["next_after"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(
        seen, episodes,
        "a walk in steps of one saw a different list"
    );
    t.close().await;
}

#[tokio::test]
async fn the_podcast_list_filters_and_sorts() {
    let t = App::open().await;
    let (podcast, _) = library(&t).await;

    let by_status = t.send("GET", "/api/v1/podcasts?status=active", None).await;
    assert_eq!(by_status.status, StatusCode::OK, "{}", by_status.json);
    assert_eq!(by_status.json["podcasts"].as_array().unwrap().len(), 1);
    let none = t
        .send("GET", "/api/v1/podcasts?status=archived", None)
        .await;
    assert!(none.json["podcasts"].as_array().unwrap().is_empty());

    for sort in ["title", "added", "refreshed", "episodes"] {
        let r = t
            .send("GET", &format!("/api/v1/podcasts?sort={sort}"), None)
            .await;
        assert_eq!(r.status, StatusCode::OK, "{sort}: {}", r.json);
        assert_eq!(r.json["podcasts"][0]["podcast"]["id"], podcast);
    }
    for bad in [
        "/api/v1/podcasts?status=nonsense",
        "/api/v1/podcasts?sort=sideways",
    ] {
        let r = t.send("GET", bad, None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.json);
        assert_eq!(r.json["error"]["kind"], "invalid", "{bad}");
    }
    t.close().await;
}

/// Seeds titled podcasts with ties in every sort key: `(title, hours after
/// the epoch of the last refresh, episodes)`.
async fn seeded(t: &App, rows: &[(&str, Option<i64>, usize)]) {
    let mut tx = t.engine.storage().begin().await.unwrap();
    for (title, refreshed, episodes) in rows {
        let mut p = uguisu_storage::podcasts::sample(title);
        p.last_refresh_at =
            refreshed.map(|h| time::OffsetDateTime::UNIX_EPOCH + time::Duration::hours(h));
        uguisu_storage::podcasts::insert(&mut tx, &p).await.unwrap();
        let eps: Vec<_> = (0..*episodes)
            .map(|n| {
                uguisu_storage::episodes::sample(
                    p.id,
                    &format!("{title}{n}"),
                    "e",
                    time::OffsetDateTime::UNIX_EPOCH,
                )
            })
            .collect();
        uguisu_storage::episodes::upsert_all(&mut tx, &eps)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

/// Every podcast of a list, `limit` at a time, as the JSON rows.
async fn walk(t: &App, query: &str, limit: u32) -> Vec<serde_json::Value> {
    let mut seen = Vec::new();
    let mut after = String::new();
    for _ in 0..50 {
        let r = t
            .send(
                "GET",
                &format!("/api/v1/podcasts?{query}&limit={limit}{after}"),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{query}: {}", r.json);
        seen.extend(r.json["podcasts"].as_array().unwrap().iter().cloned());
        match r.json["next_after"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => return seen,
        }
    }
    panic!("{query} never ended");
}

#[tokio::test]
async fn every_podcast_sort_pages_stably() {
    let t = App::open().await;
    seeded(
        &t,
        &[
            ("Alpha", Some(2), 3),
            ("Beta", Some(1), 1),
            ("The Beta", None, 3),
            ("Delta", Some(2), 0),
            ("Gamma", None, 1),
            ("Zeta", Some(3), 0),
            ("Eta", Some(3), 2),
        ],
    )
    .await;
    let ids = |rows: &[serde_json::Value]| -> Vec<String> {
        rows.iter()
            .map(|p| p["podcast"]["id"].as_str().unwrap().to_owned())
            .collect()
    };
    for (query, count) in [
        ("sort=title", 7),
        ("sort=added", 7),
        ("sort=refreshed", 7),
        ("sort=episodes", 7),
        ("sort=refreshed&q=eta", 4),
        ("sort=episodes&q=ETA", 4),
        ("q=%20beta%20", 2),
        ("q=", 7),
    ] {
        let whole = walk(&t, query, 500).await;
        assert_eq!(whole.len(), count, "{query}: {whole:?}");
        let mut unique = ids(&whole);
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), count, "{query} repeated a podcast");
        for limit in [1, 2, 3] {
            assert_eq!(
                ids(&walk(&t, query, limit).await),
                ids(&whole),
                "{query}, {limit} at a time, walked a different list"
            );
        }
        if query.contains("refreshed") {
            let keys: Vec<&str> = whole
                .iter()
                .map(|p| p["podcast"]["last_refresh_at"].as_str().unwrap_or(""))
                .collect();
            assert!(
                keys.windows(2).all(|w| w[0] >= w[1]),
                "{query}: newest first, never refreshed last: {keys:?}"
            );
        }
        if query.contains("episodes") {
            let keys: Vec<u64> = whole
                .iter()
                .map(|p| p["episodes_total"].as_u64().unwrap())
                .collect();
            assert!(
                keys.windows(2).all(|w| w[0] >= w[1]),
                "{query}: most episodes first: {keys:?}"
            );
        }
    }

    let title_first = walk(&t, "sort=title", 500).await;
    let alpha = title_first[0]["podcast"]["id"].as_str().unwrap();
    for bad in [
        format!("/api/v1/podcasts?q=eta&after={alpha}"),
        format!("/api/v1/podcasts?q={}", "a".repeat(201)),
    ] {
        let r = t.send("GET", &bad, None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.json);
        assert_eq!(r.json["error"]["kind"], "invalid", "{bad}");
    }
    let longest = format!("/api/v1/podcasts?q={}", "a".repeat(200));
    assert_eq!(t.send("GET", &longest, None).await.status, StatusCode::OK);
    t.close().await;
}

/// The counters and the last fetch used to cost three queries per row. They
/// still have to be right for every row on the page.
#[tokio::test]
async fn a_page_still_carries_the_counters_and_the_last_fetch() {
    let t = App::open().await;
    let (podcast, episodes) = library(&t).await;
    let r = t.send("GET", "/api/v1/podcasts?limit=1", None).await;
    let first = &r.json["podcasts"][0];
    assert_eq!(first["podcast"]["id"], podcast);
    assert_eq!(first["episodes_total"], episodes.len());
    assert_eq!(first["episodes_present"], episodes.len());
    assert!(first["source"]["feed_url"].is_string(), "{}", r.json);
    assert_eq!(
        first["last_fetch"]["outcome"], "fetched",
        "the newest fetch of the current source: {}",
        r.json
    );
    assert_eq!(
        first["last_fetch"]["source_id"], first["source"]["id"],
        "the fetch has to belong to the source it is reported under"
    );
    t.close().await;
}
