//! End-to-end test of the archive engine against a local feed server:
//! add → download → re-publish → source feed changes → fixity check.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::routing::get;
use uguisu_core::{Config, Uguisu, rss};

fn feed(items: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
<channel>
  <title>Test Cast</title>
  <link>https://example.org</link>
  <description>A feed for tests</description>
  <itunes:image href="BASE/cover.png"/>
  {items}
</channel>
</rss>"#
    )
}

fn item(n: u32, title: &str) -> String {
    format!(
        r#"<item><title>{title}</title><guid>ep-{n}</guid>
<pubDate>Mon, 0{n} Jan 2024 10:00:00 +0000</pubDate>
<enclosure url="BASE/audio/{n}.mp3?session=1" type="audio/mpeg" length="10"/></item>"#
    )
}

type FeedBody = Arc<Mutex<String>>;

async fn start_source(body: FeedBody) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let base_for_feed = base.clone();
    let app = Router::new()
        .route(
            "/feed.xml",
            get(move |State(body): State<FeedBody>| {
                let base = base_for_feed.clone();
                async move {
                    let xml = body.lock().unwrap().replace("BASE", &base);
                    ([(CONTENT_TYPE, "application/rss+xml")], xml)
                }
            }),
        )
        .route(
            "/audio/{file}",
            get(
                |axum::extract::Path(file): axum::extract::Path<String>| async move {
                    ([(CONTENT_TYPE, "audio/mpeg")], format!("fake audio {file}"))
                },
            ),
        )
        .route(
            "/cover.png",
            get(|| async { ([(CONTENT_TYPE, "image/png")], "fake png") }),
        )
        .with_state(body);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn archive_flow() {
    let source_feed: FeedBody = Arc::new(Mutex::new(feed(&format!(
        "{}{}",
        item(2, "Second"),
        item(1, "First")
    ))));
    let base = start_source(source_feed.clone()).await;

    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    // Add: both episodes are indexed, the raw feed is snapshotted, the cover is stored.
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    assert_eq!(
        uguisu
            .add_podcast(&format!("{base}/feed.xml"))
            .await
            .unwrap(),
        id,
        "adding the same feed twice must not create a duplicate"
    );
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    assert_eq!(podcast.title, "Test Cast");
    assert_eq!(podcast.cover_file.as_deref(), Some("cover.png"));
    let podcast_dir = uguisu.archive_path(&podcast.dir);
    assert!(podcast_dir.join("podcast.json").exists());
    assert_eq!(
        std::fs::read_dir(podcast_dir.join("feeds"))
            .unwrap()
            .count(),
        1
    );

    // Download: files land on disk next to their sidecars, hashes are recorded.
    assert_eq!(uguisu.download_pending().await.unwrap(), 2);
    let episodes = uguisu.episodes(id).await.unwrap();
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0].episode.title, "Second");
    for view in &episodes {
        let path = uguisu.archive_path(view.media_path.as_deref().expect("episode is archived"));
        assert!(path.exists());
        assert!(path.with_file_name("media.mp3.blake3").exists());
        assert!(path.with_file_name("episode.json").exists());
        assert!(path.with_file_name("item.xml").exists());
        assert_eq!(view.media_mime.as_deref(), Some("audio/mpeg"));
    }
    assert_eq!(
        std::fs::read_to_string(uguisu.archive_path(episodes[1].media_path.as_deref().unwrap()))
            .unwrap(),
        "fake audio 1.mp3"
    );

    // Re-publish: original GUIDs, enclosures pointing at the archive.
    let xml = rss::build_feed(&podcast, &episodes, "https://archive.example");
    let republished = uguisu_core::feed::parse(xml.as_bytes()).unwrap();
    assert_eq!(republished.items.len(), 2);
    assert_eq!(republished.items[0].guid, "ep-2");
    let enclosure = republished.items[0].enclosure_url.as_deref().unwrap();
    assert!(
        enclosure.starts_with(&format!(
            "https://archive.example/media/{}/",
            podcast.feed_token
        )) && enclosure.ends_with("/media.mp3"),
        "unexpected enclosure {enclosure}"
    );

    // The source drops episode 1, renames episode 2 and publishes episode 3.
    *source_feed.lock().unwrap() = feed(&format!(
        "{}{}",
        item(3, "Third"),
        item(2, "Second (remastered)")
    ));
    uguisu.refresh(id).await.unwrap();
    assert_eq!(
        std::fs::read_dir(podcast_dir.join("feeds"))
            .unwrap()
            .count(),
        2
    );
    let episodes = uguisu.episodes(id).await.unwrap();
    assert_eq!(episodes.len(), 3, "a removed episode stays in the archive");
    let first = episodes.iter().find(|e| e.episode.guid == "ep-1").unwrap();
    assert!(first.episode.removed_at.is_some());
    assert!(first.media_path.is_some());
    let second = episodes.iter().find(|e| e.episode.guid == "ep-2").unwrap();
    assert_eq!(second.episode.title, "Second (remastered)");
    assert!(second.episode.removed_at.is_none());
    let revisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM episode_revisions WHERE episode_id = ?")
            .bind(second.episode.id)
            .fetch_one(uguisu.pool())
            .await
            .unwrap();
    assert_eq!(revisions, 2, "the old title is kept as a revision");

    // Removed episodes are published or hidden depending on the podcast setting.
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let with_removed = rss::build_feed(&podcast, &episodes, "https://archive.example");
    assert!(with_removed.contains("<guid isPermaLink=\"false\">ep-1</guid>"));
    uguisu.set_include_removed(id, false).await.unwrap();
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let without_removed = rss::build_feed(&podcast, &episodes, "https://archive.example");
    assert!(!without_removed.contains("ep-1</guid>"));

    // Fixity: intact files pass, a damaged file is detected and queued again.
    uguisu.download_pending().await.unwrap();
    let report = uguisu.verify().await.unwrap();
    assert_eq!((report.checked, report.ok), (3, 3));
    let damaged = uguisu.archive_path(first.media_path.as_deref().unwrap());
    std::fs::write(&damaged, "bit rot").unwrap();
    let report = uguisu.verify().await.unwrap();
    assert_eq!(report.ok, 2);
    assert_eq!(report.corrupt.len(), 1);
    assert_eq!(uguisu.stats().await.unwrap().archived, 2);
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);
    assert_eq!(
        std::fs::read_to_string(&damaged).unwrap(),
        "fake audio 1.mp3"
    );
    assert_eq!(uguisu.verify().await.unwrap().ok, 3);

    // Removing keeps the files by default; deleting them is an explicit choice.
    uguisu.remove_podcast(id, false).await.unwrap();
    assert!(uguisu.podcast(id).await.unwrap().is_none());
    assert_eq!(uguisu.stats().await.unwrap().episodes, 0);
    assert!(podcast_dir.join("podcast.json").exists());
    assert!(damaged.exists());
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.remove_podcast(id, true).await.unwrap();
    assert!(!podcast_dir.exists());
}

#[tokio::test]
async fn private_addresses_are_refused_by_default() {
    let data = tempfile::tempdir().unwrap();
    let uguisu = Uguisu::open(Config::new(data.path())).await.unwrap();
    let error = uguisu
        .add_podcast("http://127.0.0.1:9/feed.xml")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("private or local"), "{error:#}");
}
