//! Moving from Podgrab (ADR 0050): its database names each file exactly,
//! and the whole migration - subscriptions, then files - downloads
//! nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::{Harness, tree_digest};
use uguisu_core::UguisuError;
use uguisu_core::archive::{ArchivePolicy, PolicyMode, policy_reason};
use uguisu_core::model::Episode;
use uguisu_engine::import::{Action, ImportOptions};
use uguisu_engine::opml::OpmlOptions;
use uguisu_http::CancellationToken;
use uguisu_storage::podgrab::{SampleItem, write_sample};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// The three episodes, as (title, guid, publication date, media path).
const EPISODES: [(&str, &str, &str, &str); 3] = [
    (
        "Der Einbruch",
        "guid-1",
        "Fri, 05 Jan 2024 10:00:00 +0000",
        "/normal/2048",
    ),
    (
        "Die Spur",
        "guid-2",
        "Sat, 10 Feb 2024 10:00:00 +0000",
        "/normal/2049",
    ),
    (
        "Folge 12: Das Finale",
        "guid-3",
        "Fri, 01 Mar 2024 10:00:00 +0000",
        "/normal/2050",
    ),
];

/// Serves the feed. Its enclosures are on the media server, so a download,
/// if one started, would show there.
async fn serve_feed(h: &Harness) -> String {
    let items: String = EPISODES
        .iter()
        .map(|(title, guid, date, media)| {
            format!(
                "<item><title>{title}</title><guid isPermaLink=\"false\">{guid}</guid>\
                 <pubDate>{date}</pubDate><enclosure url=\"{}{media}\" type=\"audio/mpeg\" \
                 length=\"0\"/></item>",
                h.media.base()
            )
        })
        .collect::<Vec<_>>()
        .concat();
    let feed = format!(
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Darknet Diaries</title>\
         <link>https://darknet.example/</link><description>d</description>{items}</channel></rss>"
    );
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(feed)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&h.server)
        .await;
    h.url("/feed.xml")
}

async fn library(h: &Harness) -> Vec<Episode> {
    let feed = serve_feed(h).await;
    let podcast = h
        .engine
        .add_podcast(&feed, CancellationToken::new())
        .await
        .unwrap()
        .podcast;
    h.engine
        .episodes(podcast.id, None, 10)
        .await
        .unwrap()
        .episodes
}

/// A Podgrab installation: `assets/darknet-diaries/<file>` and
/// `config/podgrab.db`, one downloaded row per file.
struct Podgrab {
    _dir: tempfile::TempDir,
    root: PathBuf,
    assets: PathBuf,
    db: PathBuf,
}

async fn podgrab(feed_url: &str, files: &[(&str, SampleItem<'_>)]) -> Podgrab {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let show = root.join("assets").join("darknet-diaries");
    std::fs::create_dir_all(&show).unwrap();
    std::fs::create_dir_all(root.join("config")).unwrap();
    let mut rows = Vec::new();
    let paths: Vec<String> = files
        .iter()
        .map(|(name, _)| format!("/assets/darknet-diaries/{name}"))
        .collect();
    for ((name, row), download_path) in files.iter().zip(&paths) {
        std::fs::write(show.join(name), format!("ID3\x04podgrab fixture {name}")).unwrap();
        rows.push(SampleItem {
            feed_url,
            download_path,
            ..row.clone()
        });
    }
    let db = root.join("config").join("podgrab.db");
    write_sample(&db, &rows).await.unwrap();
    Podgrab {
        _dir: dir,
        assets: root.join("assets"),
        root,
        db,
    }
}

fn row<'a>(index: usize) -> SampleItem<'a> {
    let dates = [
        "2024-01-05 10:00:00+00:00",
        "2024-02-10 10:00:00+00:00",
        "2024-03-01 11:00:00+01:00",
    ];
    SampleItem {
        podcast_title: "Darknet Diaries",
        feed_url: "",
        title: EPISODES[index].0,
        guid: EPISODES[index].1,
        file_url: "",
        pub_date: dates[index],
        download_path: "",
        download_status: 2,
    }
}

fn with_db(db: &Path) -> ImportOptions {
    ImportOptions {
        podgrab_db: Some(db.to_path_buf()),
        ..ImportOptions::default()
    }
}

fn titled<'a>(episodes: &'a [Episode], title: &str) -> &'a Episode {
    episodes.iter().find(|e| e.title == title).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn podgrab_records_name_the_episodes() {
    let h = Harness::new().await;
    let episodes = library(&h).await;
    let enclosure = format!("{}/normal/2050", h.media.base());
    let feed = h.url("/feed.xml");
    // Names that say nothing: only the database knows which file is which.
    let pg = podgrab(
        &feed,
        &[
            ("a.mp3", row(0)),
            ("b.mp3", row(1)),
            (
                "c.mp3",
                SampleItem {
                    guid: "",
                    file_url: &enclosure,
                    ..row(2)
                },
            ),
        ],
    )
    .await;

    let plan = h
        .engine
        .import_plan(&pg.assets, &with_db(&pg.db))
        .await
        .unwrap();
    for (item, title) in plan
        .items
        .iter()
        .zip(["Der Einbruch", "Die Spur", "Folge 12: Das Finale"])
    {
        assert_eq!(item.action, Action::Import, "{item:?}");
        assert_eq!(
            item.matched_by.as_deref(),
            Some("source_database"),
            "{item:?}"
        );
        assert_eq!(
            item.episode_id,
            Some(titled(&episodes, title).id),
            "{item:?}"
        );
    }
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_podgrab_path_is_scored() {
    let h = Harness::new().await;
    let episodes = library(&h).await;
    let feed = h.url("/feed.xml");
    let pg = podgrab(&feed, &[("der-einbruch.mp3", row(0))]).await;
    // A second row pointing at the same file: Podgrab found it already there.
    write_sample_again(&pg.db, &feed).await;

    let plan = h
        .engine
        .import_plan(&pg.assets, &with_db(&pg.db))
        .await
        .unwrap();
    let item = &plan.items[0];
    assert_eq!(item.action, Action::Import, "{item:?}");
    assert_eq!(item.matched_by.as_deref(), Some("scored"), "{item:?}");
    assert_eq!(item.episode_id, Some(titled(&episodes, "Der Einbruch").id));
    h.engine.close().await;
}

/// Rebuilds the database with a second row for the first file.
async fn write_sample_again(db: &Path, feed: &str) {
    std::fs::remove_file(db).unwrap();
    let path = "/assets/darknet-diaries/der-einbruch.mp3";
    write_sample(
        db,
        &[
            SampleItem {
                feed_url: feed,
                download_path: path,
                ..row(0)
            },
            SampleItem {
                feed_url: feed,
                download_path: path,
                ..row(1)
            },
        ],
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_feed_falls_back_to_title() {
    let h = Harness::new().await;
    let episodes = library(&h).await;
    // Podgrab subscribed under an address the library never had.
    let pg = podgrab("https://old.example/darknet.xml", &[("x.mp3", row(1))]).await;

    let plan = h
        .engine
        .import_plan(&pg.assets, &with_db(&pg.db))
        .await
        .unwrap();
    let item = &plan.items[0];
    assert_eq!(
        item.matched_by.as_deref(),
        Some("source_database"),
        "{item:?}"
    );
    assert_eq!(item.episode_id, Some(titled(&episodes, "Die Spur").id));
    h.engine.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_format_refuses_podgrab_db() {
    let h = Harness::new().await;
    let pg = podgrab("https://feeds.example/x.xml", &[("x.mp3", row(0))]).await;
    let err = h
        .engine
        .import_plan(
            &pg.assets,
            &ImportOptions {
                format: Some(uguisu_archive::import::ImportFormat::Generic),
                ..with_db(&pg.db)
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Invalid(_)), "{err:?}");
    h.engine.close().await;
}

/// The acceptance of Phase 10: a Podgrab data directory imports without
/// re-downloading.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn podgrab_migration_downloads_nothing() {
    let h = Harness::new().await;
    let feed = serve_feed(&h).await;
    let pg = podgrab(
        &feed,
        &[
            ("der-einbruch.mp3", row(0)),
            ("2024-02-10-die-spur.mp3", row(1)),
            ("3-2024-03-01-folge-12-das-finale.mp3", row(2)),
        ],
    )
    .await;
    let before = tree_digest(&pg.root);

    // 1. The subscriptions, from Podgrab's OPML export.
    let opml = format!(
        "<opml version=\"2.0\"><body><outline text=\"Darknet Diaries\" xmlUrl=\"{feed}\"/></body></opml>"
    );
    let subscribed = h
        .engine
        .import_opml(
            &opml,
            OpmlOptions {
                apply: true,
                policy: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let podcast = subscribed.items[0].podcast_id.unwrap();
    // 2. Their episodes.
    h.refresh(podcast, false).await;
    // 3. The files.
    let plan = h
        .engine
        .import_plan(&pg.assets, &with_db(&pg.db))
        .await
        .unwrap();
    assert_eq!(plan.counts.imported, 3, "{:?}", plan.items);
    let report = h
        .engine
        .import_apply(&pg.assets, &with_db(&pg.db))
        .await
        .unwrap();
    assert_eq!(report.counts.imported, 3, "{:?}", report.items);
    assert!(
        report
            .items
            .iter()
            .all(|i| i.matched_by.as_deref() == Some("source_database"))
    );

    // Nothing was downloaded, and an automatic policy over every episode
    // has nothing left to fetch.
    let episodes: Vec<_> = h
        .engine
        .episodes(podcast, None, 10)
        .await
        .unwrap()
        .episodes
        .into_iter()
        .map(|e| e.id)
        .collect();
    h.engine
        .set_policy(&ArchivePolicy {
            podcast_id: podcast,
            mode: PolicyMode::Auto,
            max_backlog: Some(0),
            max_age_days: Some(0),
            priority: None,
            updated_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    let outcome = h.engine.apply_policy(podcast, &episodes).await.unwrap();
    assert_eq!(outcome.queued, 0, "{outcome:?}");
    assert_eq!(
        outcome.reasons.get(policy_reason::ALREADY_ARCHIVED),
        Some(&3),
        "{outcome:?}"
    );
    let jobs = h
        .engine
        .downloads()
        .list(&uguisu_download::JobFilter {
            podcast_id: Some(podcast),
            limit: 50,
            ..uguisu_download::JobFilter::default()
        })
        .await
        .unwrap()
        .jobs;
    assert!(jobs.is_empty(), "{jobs:?}");
    assert!(h.media.requests().is_empty(), "no enclosure was fetched");
    assert_eq!(
        tree_digest(&pg.root),
        before,
        "Podgrab's files and database are untouched"
    );
    h.engine.close().await;
}
