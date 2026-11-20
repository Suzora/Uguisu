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
            "/blog.xml",
            get(|| async {
                (
                    [(CONTENT_TYPE, "application/rss+xml")],
                    "<rss version=\"2.0\"><channel><title>Blog</title>\
                     <item><title>A post</title><guid>post-1</guid></item></channel></rss>",
                )
            }),
        )
        .route(
            "/show",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/html")],
                    "<!doctype html><html><head><title>Show</title>\
                     <link rel=\"alternate\" type=\"application/rss+xml\" href=\"/blog.xml\">\
                     <link rel=\"alternate\" type=\"application/rss+xml\" href=\"/missing.xml\">\
                     <link rel=\"alternate\" type=\"application/rss+xml\" href=\"feed.xml\">\
                     </head><body><p>Welcome<br></body></html>",
                )
            }),
        )
        .route(
            "/audio/{file}",
            get(
                |axum::extract::Path(file): axum::extract::Path<String>| async move {
                    // `mirror-x.mp3` serves the same bytes as `x.mp3`: a moved but unchanged file.
                    let name = file.trim_start_matches("mirror-");
                    ([(CONTENT_TYPE, "audio/mpeg")], format!("fake audio {name}"))
                },
            ),
        )
        .route(
            "/files/{file}",
            get(
                |axum::extract::Path(file): axum::extract::Path<String>| async move {
                    if file.starts_with("missing") {
                        Err(axum::http::StatusCode::NOT_FOUND)
                    } else {
                        Ok(([(CONTENT_TYPE, "text/plain")], format!("content of {file}")))
                    }
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
    let published = uguisu.feed_episodes(id).await.unwrap();
    let xml = rss::build_feed(&podcast, &published, &[], "https://archive.example");
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
    let published = uguisu.feed_episodes(id).await.unwrap();
    let with_removed = rss::build_feed(&podcast, &published, &[], "https://archive.example");
    assert!(with_removed.contains("<guid>ep-1</guid>"));
    uguisu.set_options(id, false, false).await.unwrap();
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let without_removed = rss::build_feed(&podcast, &published, &[], "https://archive.example");
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

fn item_with_file(n: u32, file: &str) -> String {
    format!(
        r#"<item><title>Episode {n}</title><guid>ep-{n}</guid>
<pubDate>Mon, 0{n} Jan 2024 10:00:00 +0000</pubDate>
<enclosure url="BASE/audio/{file}" type="audio/mpeg" length="10"/></item>"#
    )
}

#[tokio::test]
async fn replaced_media_is_kept_as_a_new_version() {
    let source_feed: FeedBody = Arc::new(Mutex::new(feed(&format!(
        "{}{}",
        item_with_file(2, "2.mp3"),
        item_with_file(1, "1.mp3")
    ))));
    let base = start_source(source_feed.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 2);

    // The source re-uploads episode 2 under a new name and merely moves episode 1.
    let replaced = feed(&format!(
        "{}{}",
        item_with_file(2, "2-remastered.mp3"),
        item_with_file(1, "mirror-1.mp3")
    ));

    // By default a new address does not trigger a second download.
    *source_feed.lock().unwrap() = replaced.clone();
    uguisu.refresh(id).await.unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 0);

    // With versions enabled, the next change of address does.
    uguisu.set_options(id, true, true).await.unwrap();
    *source_feed.lock().unwrap() = feed(&format!(
        "{}{}",
        item_with_file(2, "2-final.mp3"),
        item_with_file(1, "mirror-mirror-1.mp3")
    ));
    uguisu.refresh(id).await.unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 2);
    assert_eq!(
        uguisu.download_pending().await.unwrap(),
        0,
        "nothing is left queued"
    );

    let episodes = uguisu.episodes(id).await.unwrap();
    let two = episodes.iter().find(|e| e.episode.guid == "ep-2").unwrap();
    assert_eq!(two.media_versions, 2);
    assert_eq!(
        two.media_file_name(),
        Some("media.v2.mp3"),
        "the newest copy is published"
    );
    let newest = uguisu.archive_path(two.media_path.as_deref().unwrap());
    assert_eq!(
        std::fs::read_to_string(&newest).unwrap(),
        "fake audio 2-final.mp3"
    );
    assert_eq!(
        std::fs::read_to_string(newest.with_file_name("media.mp3")).unwrap(),
        "fake audio 2.mp3",
        "the first copy is still there"
    );
    assert!(newest.with_file_name("media.v2.mp3.blake3").exists());

    // An unchanged file under a new address is not stored twice.
    let one = episodes.iter().find(|e| e.episode.guid == "ep-1").unwrap();
    assert_eq!(one.media_versions, 1);
    assert_eq!(one.media_file_name(), Some("media.mp3"));
    assert!(
        !uguisu
            .archive_path(one.media_path.as_deref().unwrap())
            .with_file_name("download.part")
            .exists()
    );

    assert_eq!(uguisu.verify().await.unwrap().ok, 3);
}

#[tokio::test]
async fn files_are_rechecked_when_their_last_check_is_old() {
    let source_feed: FeedBody = Arc::new(Mutex::new(feed(&format!(
        "{}{}",
        item_with_file(2, "2.mp3"),
        item_with_file(1, "1.mp3")
    ))));
    let base = start_source(source_feed).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.download_pending().await.unwrap();
    let day = std::time::Duration::from_secs(24 * 3600);

    // Freshly downloaded files are not due.
    assert_eq!(uguisu.verify_due(30 * day, 10).await.unwrap().checked, 0);
    assert_eq!(uguisu.stats().await.unwrap().damaged, 0);

    // One file was last checked long ago and has been damaged since.
    let episodes = uguisu.episodes(id).await.unwrap();
    let old = episodes[0].media_path.clone().unwrap();
    sqlx::query("UPDATE media_files SET verified_at = verified_at - 40 * 86400 WHERE path = ?")
        .bind(&old)
        .execute(uguisu.pool())
        .await
        .unwrap();
    std::fs::write(uguisu.archive_path(&old), "bit rot").unwrap();

    let report = uguisu.verify_due(30 * day, 10).await.unwrap();
    assert_eq!((report.checked, report.ok), (1, 0));
    assert_eq!(report.corrupt, vec![old]);
    let stats = uguisu.stats().await.unwrap();
    assert_eq!((stats.damaged, stats.archived), (1, 1));
    let episodes = uguisu.episodes(id).await.unwrap();
    assert_eq!(episodes[0].media_damaged, 1);
    assert!(episodes[0].media_path.is_none());
}

#[tokio::test]
async fn published_feed_keeps_the_original_items() {
    let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"
     xmlns:podcast="https://podcastindex.org/namespace/1.0" xmlns:custom="https://example.org/ns">
<channel>
  <title>Rich Cast</title>
  <description>A feed with extensions</description>
  <itunes:image href="BASE/cover.png"/>
  <item>
    <title>Chapters &amp; people</title>
    <guid isPermaLink="false">rich-1</guid>
    <pubDate>Mon, 01 Jan 2024 10:00:00 +0000</pubDate>
    <enclosure url="BASE/audio/1.mp3?session=abc" type="audio/mpeg" length="10"/>
    <podcast:chapters url="BASE/files/chapters.json" type="application/json+chapters"/>
    <podcast:transcript url="BASE/files/transcript.vtt?session=abc" type="text/vtt"/>
    <podcast:transcript url="BASE/files/missing.srt" type="application/srt"/>
    <itunes:image href="BASE/files/episode.png"/>
    <podcast:person role="host">Mika Tanabe</podcast:person>
    <custom:mood>calm</custom:mood>
    <description><![CDATA[<p>Show notes with <a href="https://example.org">a link</a></p>]]></description>
  </item>
  <item>
    <title>Older, with the cover repeated as its image</title>
    <guid>rich-0</guid>
    <pubDate>Fri, 01 Dec 2023 10:00:00 +0000</pubDate>
    <enclosure url="BASE/audio/0.mp3" type="audio/mpeg" length="10"/>
    <itunes:image href="BASE/cover.png?size=small"/>
  </item>
</channel>
</rss>"#;
    let base = start_source(Arc::new(Mutex::new(source.to_string()))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 2);

    // The files the episode links to are archived next to its media. The second episode's
    // image only repeats the podcast cover and is not stored again.
    let attachments = uguisu.attachments(id).await.unwrap();
    let mut names: Vec<&str> = attachments.iter().map(|a| a.file_name()).collect();
    names.sort();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names[0].starts_with("chapters-") && names[0].ends_with(".json"));
    assert!(names[1].starts_with("image-") && names[1].ends_with(".png"));
    assert!(names[2].starts_with("transcript-") && names[2].ends_with(".vtt"));
    let transcript = attachments
        .iter()
        .find(|a| a.file_name() == names[2])
        .unwrap();
    assert_eq!(transcript.mime, "text/vtt");
    assert_eq!(
        std::fs::read_to_string(uguisu.archive_path(&transcript.path)).unwrap(),
        "content of transcript.vtt"
    );
    let episodes = uguisu.episodes(id).await.unwrap();
    assert_eq!(episodes[0].attachment_count, 3);
    assert_eq!(episodes[1].attachment_count, 0);
    assert_eq!(uguisu.episodes_page(id, 1).await.unwrap().len(), 2);
    assert!(uguisu.episodes_page(id, 2).await.unwrap().is_empty());
    assert!(
        uguisu
            .archive_path(&transcript.path)
            .with_file_name("media.mp3")
            .exists()
    );

    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let published = uguisu.feed_episodes(id).await.unwrap();
    let xml = rss::build_feed(
        &podcast,
        &published,
        &attachments,
        "https://archive.example",
    );

    // What the publisher put into the item is still there, byte for byte.
    for original in [
        r#"<podcast:person role="host">Mika Tanabe</podcast:person>"#,
        "<custom:mood>calm</custom:mood>",
        r#"<![CDATA[<p>Show notes with <a href="https://example.org">a link</a></p>]]>"#,
        r#"<guid isPermaLink="false">rich-1</guid>"#,
    ] {
        assert!(xml.contains(original), "missing {original} in\n{xml}");
    }
    // Links to archived files point at the archive; the one that could not be fetched stays.
    let archive = format!("https://archive.example/media/{}/", podcast.feed_token);
    for (tag, name) in [
        ("<podcast:chapters url=\"", names[0]),
        ("<itunes:image href=\"", names[1]),
        ("<podcast:transcript url=\"", names[2]),
    ] {
        let prefix = format!("{tag}{archive}");
        let found = xml.match_indices(&prefix).any(|(start, _)| {
            let value = &xml[start + tag.len()..];
            value[..value.find('"').unwrap()].ends_with(name)
        });
        assert!(found, "no {tag} pointing at archived {name} in\n{xml}");
    }
    assert!(xml.contains(&format!(
        r#"<podcast:transcript url="{base}/files/missing.srt""#
    )));
    assert!(xml.contains(r#"type="application/json+chapters""#));
    assert!(!xml.contains("session=abc"));

    let republished = uguisu_core::feed::parse(xml.as_bytes()).expect("published feed is valid");
    assert_eq!(republished.items.len(), 2);
    let item = &republished.items[0];
    assert_eq!(item.guid, "rich-1");
    assert!(item.enclosure_url.as_deref().unwrap().starts_with(&archive));
    assert_eq!(item.enclosure_length, Some("fake audio 1.mp3".len() as i64));
    assert!(
        republished
            .namespaces
            .contains(&("custom".to_string(), "https://example.org/ns".to_string()))
    );

    // Attachments are part of the integrity check and are fetched again when damaged.
    let report = uguisu.verify().await.unwrap();
    assert_eq!((report.checked, report.ok), (5, 5));
    std::fs::write(uguisu.archive_path(&transcript.path), "bit rot").unwrap();
    let report = uguisu.verify().await.unwrap();
    assert_eq!(report.corrupt, vec![transcript.path.clone()]);
    assert_eq!(uguisu.stats().await.unwrap().damaged, 1);
    assert_eq!(uguisu.attachments(id).await.unwrap().len(), 2);
    uguisu.download_pending().await.unwrap();
    assert_eq!(uguisu.verify().await.unwrap().ok, 5);
    assert_eq!(uguisu.stats().await.unwrap().damaged, 0);
}

/// A feed split into pages (RFC 5005): every page is read and kept, and a broken page fails
/// the check instead of making its episodes look removed.
#[tokio::test]
async fn paged_feeds_are_read_completely() {
    #[derive(serde::Deserialize)]
    struct Page {
        page: Option<u32>,
    }
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/feed.xml",
        get({
            let base = base.clone();
            let broken = broken.clone();
            move |axum::extract::Query(query): axum::extract::Query<Page>| {
                let base = base.clone();
                let broken = broken.clone();
                async move {
                    let page = query.page.unwrap_or(1);
                    if page == 3 && broken.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    // Three pages; the last one links back to the first, which must not loop.
                    // Page 2 repeats an episode of page 1 and uses a relative link.
                    let (items, next) = match page {
                        1 => (
                            format!("{}{}", item(6, "Six"), item(5, "Five")),
                            format!("{base}/feed.xml?page=2"),
                        ),
                        2 => (
                            format!("{}{}{}", item(5, "Five"), item(4, "Four"), item(3, "Three")),
                            "feed.xml?page=3".to_string(),
                        ),
                        _ => (
                            format!("{}{}", item(2, "Two"), item(1, "One")),
                            format!("{base}/feed.xml"),
                        ),
                    };
                    let xml = format!(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
<channel><title>Paged Cast</title><link>https://example.org</link>
<atom:link rel="next" href="{}"/>
{items}</channel></rss>"#,
                        next.replace('&', "&amp;")
                    )
                    .replace("BASE", &base);
                    Ok(([(CONTENT_TYPE, "application/rss+xml")], xml))
                }
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    let titles: Vec<String> = uguisu
        .episodes(id)
        .await
        .unwrap()
        .into_iter()
        .map(|view| view.episode.title)
        .collect();
    assert_eq!(titles, ["Six", "Five", "Four", "Three", "Two", "One"]);
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let snapshots = uguisu.archive_path(&podcast.dir).join("feeds");
    assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 3);

    // A page that fails: the check reports it and no episode is marked as removed.
    broken.store(true, std::sync::atomic::Ordering::SeqCst);
    let error = uguisu.refresh(id).await.unwrap_err();
    assert!(format!("{error:#}").contains("500"), "{error:#}");
    let summary = uguisu.podcast_summary(id).await.unwrap().unwrap();
    assert_eq!(summary.removed_count, 0);
    assert!(summary.podcast.last_fetch_error.is_some());

    // Back to normal: nothing changed, so nothing new is stored.
    broken.store(false, std::sync::atomic::Ordering::SeqCst);
    uguisu.refresh(id).await.unwrap();
    assert_eq!(uguisu.episodes(id).await.unwrap().len(), 6);
    assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 3);
}

/// An Atom feed as source: archived like any other feed and published as RSS.
#[tokio::test]
async fn atom_feeds_are_archived_and_published_as_rss() {
    let atom = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Cast</title>
  <link href="https://example.org/"/>
  <entry>
    <id>urn:uuid:atom-1</id>
    <title>Only episode</title>
    <published>2024-01-02T10:00:00Z</published>
    <link rel="enclosure" href="BASE/audio/1.mp3" type="audio/mpeg" length="10"/>
    <content type="html">&lt;p&gt;Notes&lt;/p&gt;</content>
  </entry>
</feed>"#;
    let base = start_source(Arc::new(Mutex::new(atom.to_string()))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);
    let episodes = uguisu.episodes(id).await.unwrap();
    let item_xml = uguisu
        .archive_path(episodes[0].media_path.as_deref().unwrap())
        .with_file_name("item.xml");
    assert!(
        std::fs::read_to_string(item_xml)
            .unwrap()
            .starts_with("<entry>"),
        "the original entry is kept as it was"
    );

    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let published = uguisu.feed_episodes(id).await.unwrap();
    let xml = rss::build_feed(&podcast, &published, &[], "https://archive.example");
    assert!(xml.contains("<rss version=\"2.0\""));
    let republished = uguisu_core::feed::parse(xml.as_bytes()).unwrap();
    assert_eq!(republished.title, "Atom Cast");
    assert_eq!(republished.items.len(), 1);
    let item = &republished.items[0];
    assert_eq!(item.guid, "urn:uuid:atom-1");
    assert_eq!(item.title, "Only episode");
    assert_eq!(item.description.as_deref(), Some("<p>Notes</p>"));
    assert_eq!(item.published_at, Some(1_704_189_600));
    assert!(
        item.enclosure_url
            .as_deref()
            .unwrap()
            .starts_with("https://archive.example/media/")
    );
}

/// The address of the podcast's web page is enough: of the feeds it announces, the one with
/// episodes is added – not the blog feed listed first, nor the one that does not exist.
#[tokio::test]
async fn the_feed_is_found_from_the_web_page() {
    let base = start_source(Arc::new(Mutex::new(feed(&item(1, "First"))))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    let id = uguisu.add_podcast(&format!("{base}/show")).await.unwrap();
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    assert_eq!(podcast.title, "Test Cast");
    assert_eq!(podcast.feed_url, format!("{base}/feed.xml"));
    assert_eq!(
        uguisu
            .add_podcast(&format!("{base}/feed.xml"))
            .await
            .unwrap(),
        id,
        "page and feed address lead to the same podcast"
    );

    // Something that is neither a feed nor a page is refused with the parser's message.
    let error = uguisu
        .add_podcast(&format!("{base}/cover.png"))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("not valid XML"), "{error:#}");
}

/// A download that breaks off is continued where it stopped, not started again.
#[tokio::test]
async fn interrupted_downloads_are_continued() {
    use axum::body::{Body, Bytes};
    use axum::http::{HeaderMap, StatusCode, header};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const AUDIO: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    // What each request asked for, as the `Range` header or "-" without one.
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let failures = Arc::new(AtomicUsize::new(2));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let feed_xml = feed(&item(1, "First")).replace("BASE", &base);
    let app = Router::new()
        .route(
            "/feed.xml",
            get(move || {
                let xml = feed_xml.clone();
                async move { ([(CONTENT_TYPE, "application/rss+xml")], xml) }
            }),
        )
        .route(
            "/audio/{file}",
            get({
                let requests = requests.clone();
                let failures = failures.clone();
                move |headers: HeaderMap| {
                    let requests = requests.clone();
                    let failures = failures.clone();
                    async move {
                        let range = headers
                            .get(header::RANGE)
                            .map(|v| v.to_str().unwrap().to_string());
                        requests
                            .lock()
                            .unwrap()
                            .push(range.clone().unwrap_or_else(|| "-".into()));
                        let start: usize = match &range {
                            Some(range) => {
                                assert_eq!(headers[header::IF_RANGE], "\"v1\"");
                                range
                                    .trim_start_matches("bytes=")
                                    .trim_end_matches('-')
                                    .parse()
                                    .unwrap()
                            }
                            None => 0,
                        };
                        // The first two answers break off after ten bytes.
                        let fail = failures
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                            .is_ok();
                        let end = if fail { start + 10 } else { AUDIO.len() };
                        let mut chunks: Vec<Result<Bytes, std::io::Error>> =
                            vec![Ok(Bytes::from_static(&AUDIO[start..end]))];
                        if fail {
                            chunks.push(Err(std::io::Error::other("connection lost")));
                        }
                        let mut response = axum::response::Response::new(Body::from_stream(
                            // The pause lets the bytes reach the client before the break.
                            futures_util::StreamExt::then(
                                futures_util::stream::iter(chunks),
                                |chunk| async move {
                                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                    chunk
                                },
                            ),
                        ));
                        let headers = response.headers_mut();
                        headers.insert(CONTENT_TYPE, "audio/mpeg".parse().unwrap());
                        headers.insert(header::ETAG, "\"v1\"".parse().unwrap());
                        if range.is_some() {
                            *response.status_mut() = StatusCode::PARTIAL_CONTENT;
                            let value =
                                format!("bytes {start}-{}/{}", AUDIO.len() - 1, AUDIO.len());
                            response
                                .headers_mut()
                                .insert(header::CONTENT_RANGE, value.parse().unwrap());
                        }
                        response
                    }
                }
            }),
        )
        .route(
            "/cover.png",
            get(|| async { ([(CONTENT_TYPE, "image/png")], "fake png") }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);

    assert_eq!(
        *requests.lock().unwrap(),
        ["-", "bytes=10-", "bytes=20-"],
        "each request continues after the bytes already received"
    );
    let episodes = uguisu.episodes(id).await.unwrap();
    let path = uguisu.archive_path(episodes[0].media_path.as_deref().expect("archived"));
    assert_eq!(std::fs::read(&path).unwrap(), AUDIO);
    assert_eq!(
        episodes[0].media_blake3.as_deref(),
        Some(blake3::hash(AUDIO).to_hex().as_str())
    );
    assert!(!path.with_file_name("download.part").exists());
    assert!(!path.with_file_name("download.part.json").exists());
    assert!(uguisu.verify().await.unwrap().corrupt.is_empty());
}
