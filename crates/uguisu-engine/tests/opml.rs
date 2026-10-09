//! OPML import and export (ADR 0049): the plan sends no request, applying
//! verifies each feed and adds it once with its policy, and an exported
//! library imports back.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use common::{Harness, fixture, synthetic_feed_with_media};
use uguisu_core::UguisuError;
use uguisu_core::archive::PolicyMode;
use uguisu_core::download::Priority;
use uguisu_core::model::{FetchState, PodcastStatus};
use uguisu_engine::opml::{OpmlAction, OpmlImport, OpmlOptions, PolicyDefaults};
use uguisu_http::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn show(name: &str, guid: Option<&str>) -> Vec<u8> {
    let guid = guid.map_or_else(String::new, |g| format!("<podcast:guid>{g}</podcast:guid>"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:podcast="https://podcastindex.org/namespace/1.0"><channel>
<title>{name}</title><link>https://{name}.example/</link><description>A show</description>{guid}
<item><title>One</title><guid>{name}-1</guid><pubDate>Mon, 01 Sep 2025 10:00:00 +0000</pubDate>
<enclosure url="https://cdn.example/{name}-1.mp3" type="audio/mpeg" length="1"/></item>
</channel></rss>"#
    )
    .into_bytes()
}

fn opml(urls: &[&str]) -> String {
    let mut body = String::new();
    for u in urls {
        let _ = write!(
            body,
            r#"<outline type="rss" text="Feed" xmlUrl="{}"/>"#,
            u.replace('&', "&amp;")
        );
    }
    format!(r#"<?xml version="1.0"?><opml version="2.0"><body>{body}</body></opml>"#)
}

async fn import(
    h: &Harness,
    text: &str,
    apply: bool,
    policy: Option<PolicyDefaults>,
) -> OpmlImport {
    h.engine
        .import_opml(
            text,
            OpmlOptions { apply, policy },
            CancellationToken::new(),
        )
        .await
        .unwrap()
}

fn actions(report: &OpmlImport) -> Vec<OpmlAction> {
    report.items.iter().map(|i| i.action).collect()
}

async fn requests(h: &Harness) -> usize {
    h.server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn dry_run_sends_no_request() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let report = import(
        &h,
        &opml(&[&h.url("/a.xml"), &h.url("/b.xml")]),
        false,
        None,
    )
    .await;
    assert!(!report.applied);
    assert_eq!(actions(&report), [OpmlAction::Add, OpmlAction::Add]);
    assert_eq!(report.counts.add, 2);
    assert_eq!(requests(&h).await, 0);
    assert!(h.engine.list_podcasts().await.unwrap().is_empty());
}

#[tokio::test]
async fn apply_adds_verified_feeds() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    h.serve("/b.xml", &show("beta", None), None, None).await;
    let mut events = h.engine.subscribe();
    let report = import(&h, &opml(&[&h.url("/a.xml"), &h.url("/b.xml")]), true, None).await;
    assert!(report.applied);
    assert_eq!(actions(&report), [OpmlAction::Add, OpmlAction::Add]);
    assert!(report.items.iter().all(|i| i.podcast_id.is_some()));

    let library = h.engine.list_podcasts().await.unwrap();
    assert_eq!(library.len(), 2);
    for detail in &library {
        let source = detail.source.as_ref().unwrap();
        assert_eq!(source.provider, "opml");
        assert_eq!(source.fetch.state, FetchState::NeverFetched);
        assert_eq!(detail.podcast.next_refresh_at, None);
        assert_eq!(detail.episodes_total, 0, "an import does not refresh");
    }
    let mut added = 0;
    while let Some(event) = events.try_recv() {
        added += usize::from(event.name() == "podcast.added");
    }
    assert_eq!(added, 2);
    assert!(h.engine.policies().await.unwrap().is_empty());
}

#[tokio::test]
async fn policy_lands_with_the_podcast() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let policy = PolicyDefaults {
        mode: PolicyMode::Auto,
        max_backlog: Some(5),
        max_age_days: None,
        priority: Some(Priority::High),
    };
    let report = import(&h, &opml(&[&h.url("/a.xml")]), true, Some(policy)).await;
    let id = report.items[0].podcast_id.unwrap();
    let stored = h.engine.policies().await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].podcast_id, id);
    assert_eq!(
        (
            stored[0].mode,
            stored[0].max_backlog,
            stored[0].max_age_days,
            stored[0].priority
        ),
        (PolicyMode::Auto, Some(5), None, Some(Priority::High))
    );
}

#[tokio::test]
async fn auto_policy_queues_first_refresh() {
    let h = Harness::new().await;
    let feed = synthetic_feed_with_media(4, h.media.base(), &["/normal/2048"]);
    h.serve("/media.xml", &feed, None, None).await;
    let policy = PolicyDefaults {
        mode: PolicyMode::Auto,
        max_backlog: Some(2),
        max_age_days: Some(0),
        priority: None,
    };
    let report = import(&h, &opml(&[&h.url("/media.xml")]), true, Some(policy)).await;
    let id = report.items[0].podcast_id.unwrap();
    h.refresh(id, false).await;
    let jobs = h
        .engine
        .downloads()
        .list(&uguisu_download::JobFilter {
            podcast_id: Some(id),
            limit: 50,
            ..uguisu_download::JobFilter::default()
        })
        .await
        .unwrap()
        .jobs;
    assert_eq!(
        jobs.len(),
        2,
        "the imported backlog limit, not the global one: {jobs:#?}"
    );
    h.engine.close().await;
}

#[tokio::test]
async fn existing_podcast_keeps_its_policy() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    h.engine
        .add_podcast(&h.url("/a.xml"), CancellationToken::new())
        .await
        .unwrap();
    let policy = PolicyDefaults {
        mode: PolicyMode::Auto,
        max_backlog: None,
        max_age_days: None,
        priority: None,
    };
    let report = import(&h, &opml(&[&h.url("/a.xml")]), true, Some(policy)).await;
    assert_eq!(actions(&report), [OpmlAction::AlreadyPresent]);
    assert!(h.engine.policies().await.unwrap().is_empty());
}

#[tokio::test]
async fn scheme_variant_is_already_present() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let added = h
        .engine
        .add_podcast(&h.url("/a.xml"), CancellationToken::new())
        .await
        .unwrap();
    let before = requests(&h).await;
    let https = h.url("/a.xml/").replacen("http://", "https://", 1);
    let report = import(&h, &opml(&[&https]), true, None).await;
    assert_eq!(actions(&report), [OpmlAction::AlreadyPresent]);
    assert_eq!(report.items[0].podcast_id, Some(added.podcast.id));
    assert_eq!(requests(&h).await, before, "nothing to verify");
}

#[tokio::test]
async fn replaced_source_is_already_present() {
    let h = Harness::new().await;
    let added = h.add_fixture("episodes_v1.xml").await;
    h.reset().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/moved.xml"))
        .mount(&h.server)
        .await;
    h.serve("/moved.xml", &fixture("episodes_v2.xml"), None, None)
        .await;
    h.refresh(added.podcast.id, true).await;
    let current = h.engine.podcast(added.podcast.id).await.unwrap().source;
    assert_eq!(current.unwrap().feed_url.as_str(), h.url("/moved.xml"));

    let report = import(&h, &opml(&[&h.url("/feed.xml")]), false, None).await;
    assert_eq!(actions(&report), [OpmlAction::AlreadyPresent]);
    assert_eq!(report.items[0].podcast_id, Some(added.podcast.id));
}

#[tokio::test]
async fn repeated_feed_is_a_duplicate() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let a = h.url("/a.xml");
    let tracked = format!("{a}?utm_source=app");
    let report = import(&h, &opml(&[&a, &tracked, &a]), true, None).await;
    assert_eq!(
        actions(&report),
        [
            OpmlAction::Add,
            OpmlAction::Duplicate,
            OpmlAction::Duplicate
        ]
    );
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn unusable_urls_are_invalid() {
    let h = Harness::new().await;
    let long = format!("https://a.example/{}", "x".repeat(4096));
    let report = import(
        &h,
        &opml(&[
            "feed://a.example/rss",
            "itpc://a.example/rss",
            "not a url",
            "ftp://a.example/rss",
            "https://",
            &long,
        ]),
        true,
        None,
    )
    .await;
    assert_eq!(report.counts.invalid, 6, "{:#?}", report.items);
    let details: Vec<_> = report
        .items
        .iter()
        .map(|i| i.detail.clone().unwrap())
        .collect();
    assert_eq!(details[0], "feed: is not http or https");
    assert!(details[5].contains("4096"), "{}", details[5]);
    assert_eq!(requests(&h).await, 0);
}

#[tokio::test]
async fn unreachable_feed_fails_alone() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let report = import(
        &h,
        &opml(&[&h.url("/missing.xml"), &h.url("/a.xml")]),
        true,
        None,
    )
    .await;
    assert_eq!(actions(&report), [OpmlAction::Failed, OpmlAction::Add]);
    let detail = report.items[0].detail.as_deref().unwrap();
    assert!(detail.contains("404"), "{detail}");
    assert_eq!(report.counts.failed, 1);
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn private_address_is_refused() {
    let h = Harness::new().await;
    let report = import(&h, &opml(&["http://10.0.0.1/feed.xml"]), true, None).await;
    assert_eq!(actions(&report), [OpmlAction::Failed]);
    let detail = report.items[0].detail.as_deref().unwrap();
    assert!(detail.contains("policy"), "{detail}");
    assert!(h.engine.list_podcasts().await.unwrap().is_empty());
}

#[tokio::test]
async fn redirected_feed_is_added() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/old.xml"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/new.xml"))
        .mount(&h.server)
        .await;
    h.serve("/new.xml", &show("alpha", None), None, None).await;
    let report = import(&h, &opml(&[&h.url("/old.xml")]), true, None).await;
    assert_eq!(actions(&report), [OpmlAction::Add], "{:#?}", report.items);
    let source = h.engine.list_podcasts().await.unwrap()[0]
        .source
        .clone()
        .unwrap();
    assert_eq!(source.feed_url.as_str(), h.url("/new.xml"));
}

#[tokio::test]
async fn website_outline_needs_review() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/site"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(
                    r#"<!doctype html><html><head><title>Alpha</title>
<link rel="alternate" type="application/rss+xml" href="/a.xml"></head><body></body></html>"#,
                ),
        )
        .mount(&h.server)
        .await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let report = import(&h, &opml(&[&h.url("/site")]), true, None).await;
    assert_eq!(actions(&report), [OpmlAction::NeedsReview]);
    let detail = report.items[0].detail.as_deref().unwrap();
    assert!(detail.contains(&h.url("/a.xml")), "{detail}");
    assert!(h.engine.list_podcasts().await.unwrap().is_empty());
}

#[tokio::test]
async fn shared_guid_is_a_conflict() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", Some("guid-1")), None, None)
        .await;
    h.serve("/b.xml", &show("mirror", Some("guid-1")), None, None)
        .await;
    h.engine
        .add_podcast(&h.url("/a.xml"), CancellationToken::new())
        .await
        .unwrap();
    let report = import(&h, &opml(&[&h.url("/b.xml")]), true, None).await;
    assert_eq!(actions(&report), [OpmlAction::Conflict]);
    assert!(
        report.items[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("guid-1")
    );
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn two_urls_one_feed_add_once() {
    let h = Harness::new().await;
    for alias in ["/alias-1.xml", "/alias-2.xml"] {
        Mock::given(method("GET"))
            .and(path(alias))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/a.xml"))
            .mount(&h.server)
            .await;
    }
    h.serve("/a.xml", &show("alpha", Some("guid-a")), None, None)
        .await;
    let report = import(
        &h,
        &opml(&[
            &h.url("/alias-1.xml"),
            &h.url("/alias-2.xml"),
            &h.url("/a.xml"),
        ]),
        true,
        None,
    )
    .await;
    assert_eq!(report.counts.add, 1, "{:#?}", report.items);
    assert_eq!(report.counts.already_present, 2, "{:#?}", report.items);
    assert_eq!(report.counts.conflict, 0);
    let ids: BTreeSet<_> = report.items.iter().map(|i| i.podcast_id.unwrap()).collect();
    assert_eq!(ids.len(), 1);
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn second_apply_adds_nothing() {
    let h = Harness::new().await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let text = opml(&[&h.url("/a.xml")]);
    let first = import(&h, &text, true, None).await;
    let before = requests(&h).await;
    let second = import(&h, &text, true, None).await;
    assert_eq!(actions(&second), [OpmlAction::AlreadyPresent]);
    assert_eq!(second.items[0].podcast_id, first.items[0].podcast_id);
    assert_eq!(requests(&h).await, before);
}

#[tokio::test]
async fn unreadable_document_is_invalid() {
    let h = Harness::new().await;
    for text in ["<rss version=\"2.0\"/>", "", "<!doctype html><html></html>"] {
        let err = h
            .engine
            .import_opml(text, OpmlOptions::default(), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, UguisuError::Invalid(_)), "{text:?}: {err:?}");
    }
}

#[tokio::test]
async fn export_lists_every_podcast() {
    let h = Harness::new().await;
    h.serve("/z.xml", &show("zulu", None), None, None).await;
    h.serve("/a.xml", &show("alpha", None), None, None).await;
    let mut ids = Vec::new();
    for p in ["/z.xml", "/a.xml"] {
        let added = h
            .engine
            .add_podcast(&h.url(p), CancellationToken::new())
            .await
            .unwrap();
        ids.push(added.podcast.id);
    }
    assert_eq!(
        h.engine.pause_podcast(ids[0]).await.unwrap(),
        PodcastStatus::Paused
    );

    let exported = h.engine.export_opml().await.unwrap();
    let outlines = uguisu_feed::opml::parse(&exported).unwrap();
    let listed: Vec<_> = outlines
        .iter()
        .map(|o| {
            (
                o.title.as_deref().unwrap(),
                o.xml_url.as_str(),
                o.html_url.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("alpha", h.url("/a.xml").as_str(), "https://alpha.example/"),
            ("zulu", h.url("/z.xml").as_str(), "https://zulu.example/"),
        ]
    );
    assert_eq!(exported, h.engine.export_opml().await.unwrap());
}

#[tokio::test]
async fn export_then_import_adds_nothing() {
    let h = Harness::new().await;
    for (p, name) in [("/a.xml", "alpha"), ("/b.xml", "beta")] {
        h.serve(p, &show(name, None), None, None).await;
        h.engine
            .add_podcast(&h.url(p), CancellationToken::new())
            .await
            .unwrap();
    }
    let before = requests(&h).await;
    let exported = h.engine.export_opml().await.unwrap();
    let report = import(&h, &exported, true, None).await;
    assert_eq!(
        actions(&report),
        [OpmlAction::AlreadyPresent, OpmlAction::AlreadyPresent]
    );
    assert_eq!(requests(&h).await, before);
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 2);
}

#[tokio::test]
async fn export_moves_to_fresh_library() {
    let h = Harness::new().await;
    for (p, name) in [("/a.xml", "alpha"), ("/b.xml", "beta")] {
        h.serve(p, &show(name, None), None, None).await;
        h.engine
            .add_podcast(&h.url(p), CancellationToken::new())
            .await
            .unwrap();
    }
    let exported = h.engine.export_opml().await.unwrap();

    let fresh = Harness::new().await;
    let report = import(&fresh, &exported, true, None).await;
    assert_eq!(report.counts.add, 2, "{:#?}", report.items);
    let feeds = |h: &Harness| {
        let engine = h.engine.clone();
        async move {
            engine
                .list_podcasts()
                .await
                .unwrap()
                .into_iter()
                .map(|d| d.source.unwrap().feed_url.to_string())
                .collect::<BTreeSet<_>>()
        }
    };
    assert_eq!(feeds(&fresh).await, feeds(&h).await);
    assert_eq!(fresh.engine.export_opml().await.unwrap(), exported);
}
