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

/// Checks a published feed against what podcast apps rely on: RSS 2.0 with the required
/// channel elements, and for every item a unique identifier, a usable enclosure pointing at
/// the archive and, if present, a date in the standard format.
fn assert_valid_feed(xml: &str) {
    let doc = roxmltree::Document::parse(xml).expect("the feed is well-formed XML");
    let rss = doc.root_element();
    assert_eq!(rss.tag_name().name(), "rss");
    assert_eq!(rss.attribute("version"), Some("2.0"));
    let own = |node: roxmltree::Node, name: &str| -> Vec<String> {
        node.children()
            .filter(|c| c.tag_name().name() == name && c.tag_name().namespace().is_none())
            .map(|c| c.text().unwrap_or_default().trim().to_string())
            .collect()
    };
    let channels: Vec<_> = rss.children().filter(|c| c.is_element()).collect();
    assert_eq!(channels.len(), 1, "exactly one channel");
    let channel = channels[0];
    assert_eq!(channel.tag_name().name(), "channel");
    for required in ["title", "link", "description"] {
        assert_eq!(
            own(channel, required).len(),
            1,
            "one <{required}> in the channel"
        );
    }
    assert!(!own(channel, "title")[0].is_empty());
    assert!(own(channel, "link")[0].starts_with("http"));

    let mut guids = std::collections::HashSet::new();
    for item in channel.children().filter(|c| c.tag_name().name() == "item") {
        let guid = own(item, "guid");
        assert_eq!(guid.len(), 1, "one <guid> per item");
        assert!(!guid[0].is_empty());
        assert!(guids.insert(guid[0].clone()), "duplicate guid {}", guid[0]);
        assert!(
            !own(item, "title").is_empty() || !own(item, "description").is_empty(),
            "an item needs a title or a description"
        );
        let enclosures: Vec<_> = item
            .children()
            .filter(|c| c.tag_name().name() == "enclosure" && c.tag_name().namespace().is_none())
            .collect();
        assert_eq!(enclosures.len(), 1, "one <enclosure> per item");
        let enclosure = enclosures[0];
        let url = enclosure.attribute("url").expect("enclosure url");
        assert!(url.starts_with("https://archive.example/media/"), "{url}");
        assert!(!url.contains(' '));
        let length: u64 = enclosure
            .attribute("length")
            .expect("enclosure length")
            .parse()
            .expect("numeric length");
        assert!(length > 0);
        assert!(
            enclosure
                .attribute("type")
                .expect("enclosure type")
                .contains('/')
        );
        for date in own(item, "pubDate") {
            assert!(
                uguisu_core::feed::parse_date(&date).is_some(),
                "unreadable date {date}"
            );
            assert!(date.ends_with("+0000") || date.ends_with("GMT"), "{date}");
        }
    }
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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
    assert_valid_feed(&xml);
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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
    assert_valid_feed(&xml);

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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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
    assert_valid_feed(&xml);
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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

    const AUDIO: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    // What each request asked for, as the `Range` header or "-" without one.
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let failures = Arc::new(Mutex::new(2u32));

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
                        let fail = {
                            let mut left = failures.lock().unwrap();
                            let fail = *left > 0;
                            *left = left.saturating_sub(1);
                            fail
                        };
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
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
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

/// A host that asks for a break gets it: the download is postponed by the time it named and
/// does not count as a failed attempt. Requests to one host keep their distance.
#[tokio::test]
async fn sources_are_treated_politely() {
    use axum::http::{StatusCode, header};

    let audio_requests = Arc::new(Mutex::new(0u32));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let feed_xml = feed(&format!("{}{}", item(2, "Second"), item(1, "First")))
        .replace("BASE", &base)
        .replace("<itunes:image href=\"", "<itunes:image href=\"x");
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
                let audio_requests = audio_requests.clone();
                move || {
                    *audio_requests.lock().unwrap() += 1;
                    async {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            [(header::RETRY_AFTER, "120")],
                        )
                    }
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::from_millis(300);
    cfg.download_concurrency = 1;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    let started = std::time::Instant::now();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.refresh(id).await.unwrap();
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(300),
        "the second request to the host waits for its slot"
    );

    uguisu.download_pending().await.unwrap();
    assert_eq!(
        *audio_requests.lock().unwrap(),
        1,
        "after the host asked for a break, the second episode is not even requested"
    );
    let now = uguisu_core::util::now();
    for view in uguisu.episodes(id).await.unwrap() {
        let episode = view.episode;
        assert_eq!(episode.download_attempts, 0, "a break is not a failure");
        let wait = episode.next_attempt_at - now;
        assert!((100..=121).contains(&wait), "postponed by {wait} seconds");
        assert!(
            episode
                .download_error
                .as_deref()
                .unwrap()
                .contains("asked to wait"),
        );
    }
}

/// Images shown in the show notes are archived, and the published notes show the copies.
#[tokio::test]
async fn images_in_show_notes_are_archived() {
    let notes = r#"<item><title>Pictures</title><guid>ep-1</guid>
<pubDate>Mon, 01 Jan 2024 10:00:00 +0000</pubDate>
<description><![CDATA[<p><img src="BASE/files/photo.png?w=600&amp;fit=1" alt="A photo">
<img src="BASE/files/missing.png"> <img src="BASE/files/pixel"></p>]]></description>
<enclosure url="BASE/audio/1.mp3" type="audio/mpeg" length="10"/></item>"#;
    let base = start_source(Arc::new(Mutex::new(feed(notes)))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.download_pending().await.unwrap();

    // The photo is archived; the missing file and the thing that is no image are not.
    let attachments = uguisu.attachments(id).await.unwrap();
    assert_eq!(attachments.len(), 1);
    let photo = &attachments[0];
    assert!(photo.file_name().starts_with("notes-image-"));
    assert!(photo.file_name().ends_with(".png"));
    assert_eq!(
        std::fs::read_to_string(uguisu.archive_path(&photo.path)).unwrap(),
        "content of photo.png"
    );

    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let published = uguisu.feed_episodes(id).await.unwrap();
    let xml = rss::build_feed(
        &podcast,
        &published,
        &attachments,
        "https://archive.example",
    );
    assert_valid_feed(&xml);
    let archived = rss::attachment_url("https://archive.example", &podcast.feed_token, photo);
    assert!(
        xml.contains(&format!("<img src=\"{archived}\" alt=\"A photo\">")),
        "{xml}"
    );
    assert!(!xml.contains("photo.png?w=600"));
    assert!(
        xml.contains(&format!("{base}/files/missing.png")),
        "what could not be archived keeps its source address"
    );
    assert!(uguisu_core::feed::parse(xml.as_bytes()).is_ok());
}

/// A feed behind a password: the credentials from the feed address also open the media files
/// on the same server, and appear nowhere in the archive, the published feed or messages.
#[tokio::test]
async fn feeds_with_credentials_are_archived_without_leaking_them() {
    use axum::http::{Request, StatusCode, header};

    // "me:secret" in Base64.
    const AUTHORIZED: &str = "Basic bWU6c2VjcmV0";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://{address}");
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
            get(|| async { ([(CONTENT_TYPE, "audio/mpeg")], "private audio") }),
        )
        .route(
            "/cover.png",
            get(|| async { ([(CONTENT_TYPE, "image/png")], "fake png") }),
        )
        .layer(axum::middleware::from_fn(
            |request: Request<axum::body::Body>, next: axum::middleware::Next| async move {
                let authorized = request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .is_some_and(|value| value == AUTHORIZED);
                if authorized {
                    Ok(next.run(request).await)
                } else {
                    Err(StatusCode::UNAUTHORIZED)
                }
            },
        ));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    // Without or with wrong credentials the feed is refused, and the message keeps quiet.
    assert!(
        uguisu
            .add_podcast(&format!("{base}/feed.xml"))
            .await
            .is_err()
    );
    let error = uguisu
        .add_podcast(&format!("http://me:wrong-pass@{address}/feed.xml"))
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("401"), "{message}");
    assert!(!message.contains("wrong-pass"), "{message}");

    let id = uguisu
        .add_podcast(&format!("http://me:secret@{address}/feed.xml"))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);
    let episodes = uguisu.episodes(id).await.unwrap();
    let media = uguisu.archive_path(episodes[0].media_path.as_deref().expect("archived"));
    assert_eq!(std::fs::read_to_string(media).unwrap(), "private audio");
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    assert_eq!(podcast.cover_file.as_deref(), Some("cover.png"));

    // Nothing on disk besides the database knows the password.
    fn files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, found);
            } else {
                found.push(path);
            }
        }
    }
    let mut archived = Vec::new();
    files(&uguisu.archive_dir(), &mut archived);
    assert!(archived.len() >= 5);
    for path in archived {
        let content = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        assert!(!content.contains("secret"), "{}", path.display());
    }
    let published = uguisu.feed_episodes(id).await.unwrap();
    let xml = rss::build_feed(&podcast, &published, &[], "https://archive.example");
    assert_valid_feed(&xml);
    assert!(!xml.contains("secret"));
}

/// The archive carries its format version, and a newer archive is not touched.
#[tokio::test]
async fn the_archive_states_its_format() {
    let data = tempfile::tempdir().unwrap();
    let marker = data.path().join("archive/archive.json");
    drop(Uguisu::open(Config::new(data.path())).await.unwrap());
    let content: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
    assert_eq!(content["format"], 4);
    // Opening again is fine and leaves the marker alone.
    drop(Uguisu::open(Config::new(data.path())).await.unwrap());

    std::fs::write(&marker, r#"{ "format": 99, "software": "Uguisu" }"#).unwrap();
    let error = Uguisu::open(Config::new(data.path()))
        .await
        .err()
        .expect("a newer archive must be refused");
    assert!(
        format!("{error:#}").contains("format version 99"),
        "{error:#}"
    );
}

/// `episode.json` records the episode's files with their checksums and whether the episode is
/// still in the feed; an archive from before that is brought up to date.
#[tokio::test]
async fn episode_files_describe_the_archive() {
    let two = format!(
        "{}{}",
        item(2, "Second").replace(
            "<guid>",
            "<podcast:transcript xmlns:podcast=\"https://podcastindex.org/namespace/1.0\" \
             url=\"BASE/files/two.vtt\" type=\"text/vtt\"/><guid>"
        ),
        item(1, "First")
    );
    let source: FeedBody = Arc::new(Mutex::new(feed(&two)));
    let base = start_source(source.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.download_pending().await.unwrap();

    let read = |uguisu: &Uguisu, view: &uguisu_core::model::EpisodeView| -> serde_json::Value {
        let path = uguisu
            .archive_path(view.media_path.as_deref().unwrap())
            .with_file_name("episode.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    let episodes = uguisu.episodes(id).await.unwrap();
    let second = read(&uguisu, &episodes[0]);
    assert_eq!(second["guid"], "ep-2");
    assert_eq!(second["hash"], episodes[0].episode.meta_hash.as_str());
    assert!(second["removed"].is_null());
    assert!(second["first_seen"].is_string());
    let files = second["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["kind"], "media");
    assert_eq!(files[0]["name"], "media.mp3");
    assert_eq!(files[0]["type"], "audio/mpeg");
    assert_eq!(
        files[0]["blake3"],
        blake3::hash(b"fake audio 2.mp3").to_hex().as_str()
    );
    assert_eq!(files[0]["size"], 16);
    assert_eq!(files[1]["kind"], "transcript");
    assert_eq!(
        files[1]["blake3"],
        blake3::hash(b"content of two.vtt").to_hex().as_str()
    );
    assert_eq!(files[1]["source"], format!("{base}/files/two.vtt"));
    let revisions = second["revisions"].as_array().unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0]["file"], "item.xml");
    assert_eq!(revisions[0]["hash"], second["hash"]);

    // What this installation decided about the podcast is on disk as well.
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let settings_path = uguisu.archive_path(&podcast.dir).join("uguisu.json");
    let settings = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    assert_eq!(
        settings(&settings_path)["feed_token"],
        podcast.feed_token.as_str()
    );
    assert_eq!(settings(&settings_path)["keep_versions"], false);
    uguisu.set_options(id, true, true).await.unwrap();
    assert_eq!(settings(&settings_path)["keep_versions"], true);
    uguisu.regenerate_token(id).await.unwrap();
    assert_ne!(
        settings(&settings_path)["feed_token"],
        podcast.feed_token.as_str()
    );

    // An episode that leaves the feed is marked in its file, and unmarked when it returns.
    *source.lock().unwrap() = feed(&item(1, "First"));
    uguisu.refresh(id).await.unwrap();
    assert!(read(&uguisu, &episodes[0])["removed"].is_string());
    *source.lock().unwrap() = feed(&two);
    uguisu.refresh(id).await.unwrap();
    assert!(read(&uguisu, &episodes[0])["removed"].is_null());

    // A changed feed entry: the earlier state stays next to the current one.
    *source.lock().unwrap() = feed(&two.replace("Second", "Second, corrected"));
    uguisu.refresh(id).await.unwrap();
    let second = read(&uguisu, &episodes[0]);
    let revisions = second["revisions"].as_array().unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[1]["file"], "item.xml");
    let episode_dir = uguisu
        .archive_path(episodes[0].media_path.as_deref().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let earlier =
        std::fs::read_to_string(episode_dir.join(revisions[0]["file"].as_str().unwrap())).unwrap();
    assert!(earlier.contains("<title>Second</title>"));
    let current = std::fs::read_to_string(episode_dir.join("item.xml")).unwrap();
    assert!(current.contains("<title>Second, corrected</title>"));

    // An archive in the first format: its files are brought up to date once.
    std::fs::remove_file(&settings_path).unwrap();
    let marker = data.path().join("archive/archive.json");
    std::fs::write(&marker, r#"{ "format": 1, "software": "Uguisu" }"#).unwrap();
    let old = uguisu
        .archive_path(episodes[1].media_path.as_deref().unwrap())
        .with_file_name("episode.json");
    std::fs::write(&old, r#"{ "guid": "ep-1", "title": "First" }"#).unwrap();
    assert_eq!(uguisu.upgrade_archive().await.unwrap(), 2);
    assert_eq!(read(&uguisu, &episodes[1])["files"][0]["name"], "media.mp3");
    assert!(settings_path.exists());
    assert_eq!(uguisu.upgrade_archive().await.unwrap(), 0);
    let content: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
    assert_eq!(content["format"], 4);
}

/// The database is only an index: after losing it, a rebuild from the archive publishes the
/// very same feed.
#[tokio::test]
async fn the_index_is_rebuilt_from_the_archive() {
    let transcript = "<podcast:transcript xmlns:podcast=\"https://podcastindex.org/namespace/1.0\" \
                      url=\"BASE/files/two.vtt\" type=\"text/vtt\"/><guid>";
    let three = format!(
        "{}{}{}",
        item(3, "Third"),
        item(2, "Second").replace("<guid>", transcript),
        item(1, "First")
    );
    let source: FeedBody = Arc::new(Mutex::new(feed(&three)));
    let base = start_source(source.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let config = || {
        let mut cfg = Config::new(data.path());
        cfg.allow_private_networks = true;
        cfg.min_free_space = 0;
        cfg.host_pause = std::time::Duration::ZERO;
        cfg
    };
    let uguisu = Uguisu::open(config()).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.set_options(id, true, true).await.unwrap();
    uguisu.download_pending().await.unwrap();
    // History: one episode changes, one leaves the feed.
    *source.lock().unwrap() = feed(&format!(
        "{}{}",
        item(3, "Third, corrected"),
        item(2, "Second").replace("<guid>", transcript)
    ));
    uguisu.refresh(id).await.unwrap();

    let publish = |uguisu: Uguisu, id: i64| async move {
        let podcast = uguisu.podcast(id).await.unwrap().unwrap();
        let episodes = uguisu.feed_episodes(id).await.unwrap();
        let attachments = uguisu.attachments(id).await.unwrap();
        rss::build_feed(&podcast, &episodes, &attachments, "https://archive.example")
    };
    let before = publish(uguisu.clone(), id).await;
    assert!(before.contains("Third, corrected") && before.contains("ep-1"));
    assert!(
        before.contains("/a/"),
        "the transcript is published from the archive"
    );
    let revisions_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episode_revisions")
        .fetch_one(uguisu.pool())
        .await
        .unwrap();

    // A rebuild never touches an index that is in use.
    assert!(uguisu.reindex().await.is_err());

    // Lose the database.
    uguisu.pool().close().await;
    drop(uguisu);
    for name in ["uguisu.db", "uguisu.db-wal", "uguisu.db-shm"] {
        let _ = std::fs::remove_file(data.path().join(name));
    }
    let uguisu = Uguisu::open(config()).await.unwrap();
    assert!(uguisu.podcasts().await.unwrap().is_empty());

    let report = uguisu.reindex().await.unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!((report.podcasts, report.episodes), (1, 3));
    assert_eq!(report.files, 4, "three media files and a transcript");

    let id = uguisu.podcasts().await.unwrap()[0].podcast.id;
    let after = publish(uguisu.clone(), id).await;
    assert_eq!(
        before, after,
        "the published feed is the same, address for address"
    );

    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    assert!(podcast.keep_versions && podcast.include_removed);
    assert_eq!(podcast.cover_file.as_deref(), Some("cover.png"));
    let summary = uguisu.podcast_summary(id).await.unwrap().unwrap();
    assert_eq!(summary.removed_count, 1);
    let revisions_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episode_revisions")
        .fetch_one(uguisu.pool())
        .await
        .unwrap();
    assert_eq!(revisions_before, revisions_after);
    let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM feed_snapshots")
        .fetch_one(uguisu.pool())
        .await
        .unwrap();
    assert_eq!(snapshots, 2);

    // The rebuilt index agrees with the files, and the next check of the feed changes nothing.
    let verified = uguisu.verify().await.unwrap();
    assert_eq!(verified.ok, verified.checked);
    uguisu.refresh(id).await.unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 0);
    assert_eq!(before, publish(uguisu.clone(), id).await);
}

/// With the reserve of free disk space used up, downloads wait instead of filling the disk,
/// and waiting does not count against the episode.
#[tokio::test]
async fn downloads_wait_while_the_disk_is_full() {
    let base = start_source(Arc::new(Mutex::new(feed(&item(1, "First"))))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.host_pause = std::time::Duration::ZERO;
    // More than any disk has free.
    cfg.min_free_space = u64::MAX / 2;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    assert!(uguisu.free_space().is_some());
    assert!(uguisu.space_is_low());

    // The feed itself is still archived.
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.download_pending().await.unwrap();
    let episode = uguisu.episodes(id).await.unwrap().remove(0);
    assert!(episode.media_path.is_none());
    assert_eq!(episode.episode.download_attempts, 0);
    assert!(episode.episode.next_attempt_at > uguisu_core::util::now() + 3000);
    let error = episode.episode.download_error.unwrap();
    assert!(error.contains("not enough disk space"), "{error}");
}

/// Copies of the database: each is a complete, usable database, and only the newest are kept.
#[tokio::test]
async fn the_database_is_copied() {
    let base = start_source(Arc::new(Mutex::new(feed(&item(1, "First"))))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    cfg.backups = 2;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();

    let mut copies = Vec::new();
    for _ in 0..3 {
        copies.push(uguisu.backup_database().await.unwrap());
        // The name carries the second.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }
    assert!(!copies[0].exists(), "the oldest copy made room");
    assert!(copies[1].exists() && copies[2].exists());
    assert_eq!(
        std::fs::read_dir(data.path().join("backups"))
            .unwrap()
            .count(),
        2
    );

    // A copy put in the place of the database is a working database.
    let restored = tempfile::tempdir().unwrap();
    std::fs::copy(&copies[2], restored.path().join("uguisu.db")).unwrap();
    let from_copy = Uguisu::open(Config::new(restored.path())).await.unwrap();
    let podcasts = from_copy.podcasts().await.unwrap();
    assert_eq!(podcasts.len(), 1);
    assert_eq!(podcasts[0].episode_count, 1);
}

/// Repairing a damaged file: the same file from the source simply replaces it; if the source
/// has something else by now, the damaged copy is kept beside the new file.
#[tokio::test]
async fn a_damaged_file_is_not_lost_to_a_different_one() {
    let source: FeedBody = Arc::new(Mutex::new(feed(&item(1, "First"))));
    let base = start_source(source.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();
    uguisu.download_pending().await.unwrap();
    let media = uguisu.archive_path(
        uguisu.episodes(id).await.unwrap()[0]
            .media_path
            .as_deref()
            .unwrap(),
    );
    let names = |media: &std::path::Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(media.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("media"))
            .collect();
        names.sort();
        names
    };

    // The source still has the same file: a plain repair.
    std::fs::write(&media, "bit rot").unwrap();
    assert_eq!(uguisu.verify().await.unwrap().corrupt.len(), 1);
    uguisu.download_pending().await.unwrap();
    assert_eq!(std::fs::read_to_string(&media).unwrap(), "fake audio 1.mp3");
    assert_eq!(names(&media), ["media.mp3", "media.mp3.blake3"]);

    // The source has replaced the file in the meantime.
    std::fs::write(&media, "bit rot again").unwrap();
    assert_eq!(uguisu.verify().await.unwrap().corrupt.len(), 1);
    *source.lock().unwrap() = feed(&item(1, "First").replace("audio/1.mp3", "audio/9.mp3"));
    uguisu.refresh(id).await.unwrap();
    uguisu.download_pending().await.unwrap();
    assert_eq!(std::fs::read_to_string(&media).unwrap(), "fake audio 9.mp3");
    let kept = names(&media);
    assert_eq!(kept.len(), 3, "{kept:?}");
    let aside = kept
        .iter()
        .find(|name| name.starts_with("media.damaged-"))
        .expect("the damaged copy is kept");
    assert_eq!(
        std::fs::read_to_string(media.with_file_name(aside)).unwrap(),
        "bit rot again"
    );
    let verified = uguisu.verify().await.unwrap();
    assert_eq!(verified.ok, verified.checked);
}

/// A start date: earlier episodes are listed but not downloaded, do not count as outstanding,
/// and are fetched after all once the date is cleared.
#[tokio::test]
async fn episodes_before_the_start_date_are_not_downloaded() {
    let three = format!(
        "{}{}{}",
        item(3, "Third"),
        item(2, "Second"),
        item(1, "First")
    );
    let base = start_source(Arc::new(Mutex::new(feed(&three)))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();

    // The episodes are from 1, 2 and 3 January 2024; start with the third.
    let from = uguisu_core::util::parse_rfc3339("2024-01-03T00:00:00Z").unwrap();
    let id = uguisu
        .add_podcast_from(&format!("{base}/feed.xml"), Some(from))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);
    let archived = |episodes: Vec<uguisu_core::model::EpisodeView>| -> Vec<bool> {
        episodes.iter().map(|e| e.media_path.is_some()).collect()
    };
    assert_eq!(
        archived(uguisu.episodes(id).await.unwrap()),
        [true, false, false]
    );
    let summary = uguisu.podcast_summary(id).await.unwrap().unwrap();
    assert_eq!((summary.episode_count, summary.pending_count), (3, 0));
    assert_eq!(uguisu.podcasts().await.unwrap()[0].pending_count, 0);

    // Moving the date back lets one more in; clearing it, the rest.
    uguisu
        .set_archive_from(id, Some(from - 86_400))
        .await
        .unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);
    uguisu.set_archive_from(id, None).await.unwrap();
    assert_eq!(uguisu.download_pending().await.unwrap(), 1);
    assert_eq!(
        archived(uguisu.episodes(id).await.unwrap()),
        [true, true, true]
    );
    // A later date removes nothing.
    uguisu
        .set_archive_from(id, Some(from + 86_400))
        .await
        .unwrap();
    assert_eq!(
        archived(uguisu.episodes(id).await.unwrap()),
        [true, true, true]
    );
    assert_eq!(
        uguisu.podcast(id).await.unwrap().unwrap().archive_from,
        Some(from + 86_400)
    );
}

/// Files downloaded earlier by some podcast app are taken into the archive: matched by the
/// name in the media address or by title, and what the feed no longer lists is kept on request.
#[tokio::test]
async fn existing_files_are_imported() {
    let two = format!("{}{}", item(2, "Second episode"), item(1, "First episode"));
    let base = start_source(Arc::new(Mutex::new(feed(&two)))).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();

    let collection = tempfile::tempdir().unwrap();
    std::fs::create_dir(collection.path().join("old")).unwrap();
    let write = |name: &str, content: &str| {
        std::fs::write(collection.path().join(name), content).unwrap();
    };
    write("1.mp3", "my copy of one"); // the name in the media address
    write("2024 - Second Episode (final).mp3", "my copy of two"); // the title
    write("old/Episode 0 - the pilot.mp3", "the lost pilot"); // not in the feed
    write("notes.txt", "not a media file");

    // Without asking for it, a file that fits no episode is left alone.
    let report = uguisu
        .import_directory(id, collection.path(), false)
        .await
        .unwrap();
    assert_eq!(report.matched.len(), 2, "{report:?}");
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert!(report.added.is_empty());

    let episodes = uguisu.episodes(id).await.unwrap();
    assert_eq!(episodes.len(), 2);
    for (view, content) in episodes.iter().zip(["my copy of two", "my copy of one"]) {
        let path = uguisu.archive_path(view.media_path.as_deref().expect("imported"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(path.with_file_name("media.mp3.blake3").exists());
    }
    // Nothing is left to download, and the files verify.
    assert_eq!(uguisu.download_pending().await.unwrap(), 0);
    let verified = uguisu.verify().await.unwrap();
    assert_eq!((verified.checked, verified.ok), (2, 2));

    // Again, keeping the rest: the known files are recognised, the pilot becomes an episode.
    let report = uguisu
        .import_directory(id, collection.path(), true)
        .await
        .unwrap();
    assert_eq!(
        (report.known.len(), report.added.len()),
        (2, 1),
        "{report:?}"
    );
    let episodes = uguisu.episodes(id).await.unwrap();
    assert_eq!(episodes.len(), 3);
    let pilot = episodes
        .iter()
        .find(|view| view.episode.title == "Episode 0 - the pilot")
        .unwrap();
    assert!(
        pilot.episode.removed_at.is_some(),
        "marked as not in the feed"
    );
    assert!(
        collection.path().join("1.mp3").exists(),
        "the collection is untouched"
    );

    // The pilot is published with the others and survives the next check of the feed.
    uguisu.refresh(id).await.unwrap();
    let podcast = uguisu.podcast(id).await.unwrap().unwrap();
    let published = uguisu.feed_episodes(id).await.unwrap();
    let xml = rss::build_feed(&podcast, &published, &[], "https://archive.example");
    assert_valid_feed(&xml);
    let republished = uguisu_core::feed::parse(xml.as_bytes()).unwrap();
    assert_eq!(republished.items.len(), 3);
    assert!(xml.contains("Episode 0 - the pilot"));
}

/// Titles and show notes of the whole archive can be searched.
#[tokio::test]
async fn the_archive_is_searchable() {
    let notes = |n: u32, title: &str, text: &str| {
        item(n, title).replace(
            "<guid>",
            &format!("<description><![CDATA[<p>{text}</p>]]></description><guid>"),
        )
    };
    let source: FeedBody = Arc::new(Mutex::new(feed(&format!(
        "{}{}",
        notes(
            2,
            "Nightingales at dusk",
            "A <strong>recording</strong> from the K&ouml;nigsforst, with café."
        ),
        notes(
            1,
            "Warblers",
            "The bush warbler is called uguisu; \"quoted\" AND (odd) *syntax."
        ),
    ))));
    let base = start_source(source.clone()).await;
    let data = tempfile::tempdir().unwrap();
    let mut cfg = Config::new(data.path());
    cfg.allow_private_networks = true;
    cfg.min_free_space = 0;
    cfg.host_pause = std::time::Duration::ZERO;
    let uguisu = Uguisu::open(cfg).await.unwrap();
    let id = uguisu
        .add_podcast(&format!("{base}/feed.xml"))
        .await
        .unwrap();

    let titles = |uguisu: Uguisu, query: &'static str| async move {
        uguisu
            .search_episodes(query, 20)
            .await
            .unwrap()
            .into_iter()
            .map(|hit| hit.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(titles(uguisu.clone(), "warbler").await, ["Warblers"]);
    assert_eq!(
        titles(uguisu.clone(), "night").await,
        ["Nightingales at dusk"],
        "a beginning"
    );
    assert_eq!(
        titles(uguisu.clone(), "recording dusk").await,
        ["Nightingales at dusk"]
    );
    assert_eq!(
        titles(uguisu.clone(), "CAFE").await,
        ["Nightingales at dusk"],
        "accents and case"
    );
    assert!(
        titles(uguisu.clone(), "warbler nightingales")
            .await
            .is_empty(),
        "every word"
    );
    assert!(
        titles(uguisu.clone(), "strong").await.is_empty(),
        "markup is not indexed"
    );
    // Nothing a person types is read as query syntax.
    for odd in ["\"quoted", "AND (odd) *", "a:b NEAR(x y)", "   ", "-"] {
        uguisu.search_episodes(odd, 20).await.unwrap();
    }
    let hit = &uguisu.search_episodes("uguisu", 20).await.unwrap()[0];
    assert_eq!(hit.podcast_title, "Test Cast");
    assert!(
        hit.snippet.contains("\u{1}uguisu\u{2}"),
        "{:?}",
        hit.snippet
    );

    // A changed entry is found by its new words; a removed podcast is not found at all.
    *source.lock().unwrap() = feed(&notes(1, "Warblers", "Now about wrens."));
    uguisu.refresh(id).await.unwrap();
    assert_eq!(titles(uguisu.clone(), "wrens").await, ["Warblers"]);
    assert!(titles(uguisu.clone(), "uguisu").await.is_empty());
    uguisu.remove_podcast(id, false).await.unwrap();
    assert!(titles(uguisu.clone(), "wrens").await.is_empty());
}
