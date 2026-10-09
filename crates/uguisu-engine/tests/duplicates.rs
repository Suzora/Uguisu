//! Candidate duplicates, resolved by a person (ADR 0051).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, fixture, synthetic_feed};
use time::OffsetDateTime;
use uguisu_core::archive::{
    ArchiveFile, ArchiveOrigin, ArchivePolicy, PolicyMode, TagState, VerificationState,
};
use uguisu_core::ids::{ArchiveFileId, EpisodeId, PodcastId};
use uguisu_core::model::{ArchiveState, DuplicateResolution, Episode};
use uguisu_core::{EventKind, UguisuError};
use uguisu_http::CancellationToken;
use uguisu_storage::{archive_files, changes, episodes};

/// A parser fixture whose enclosures the scenario media server answers, so
/// a queued download never leaves the machine.
fn served(h: &Harness, name: &str) -> String {
    String::from_utf8(fixture(name)).unwrap().replace(
        "https://cdn.versioned.example/",
        &format!("{}/normal/4096?", h.media.base()),
    )
}

/// Adds `episodes_v1.xml`, then refreshes `guid_changed.xml`, in which
/// Episode One comes back under a new GUID: a candidate and its original.
async fn candidate(h: &Harness) -> (PodcastId, Episode, Episode) {
    h.serve(
        "/feed.xml",
        served(h, "episodes_v1.xml").as_bytes(),
        None,
        None,
    )
    .await;
    let id = h
        .engine
        .add_podcast(&h.url("/feed.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast
        .id;
    h.reset().await;
    h.serve(
        "/feed.xml",
        served(h, "guid_changed.xml").as_bytes(),
        None,
        None,
    )
    .await;
    assert_eq!(h.refresh(id, false).await.episodes.ambiguous, 1);
    let mut page = h.engine.duplicates(Some(id), None, 10).await.unwrap();
    assert_eq!(page.duplicates.len(), 1);
    let pair = page.duplicates.remove(0);
    (
        id,
        pair.candidate,
        pair.original.expect("the original exists"),
    )
}

fn drain(sub: &mut uguisu_engine::Subscription) -> Vec<uguisu_core::Event> {
    let mut out = Vec::new();
    while let Some(e) = sub.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn merged_candidate_stays_merged() {
    let h = Harness::new().await;
    let (id, candidate, original) = candidate(&h).await;
    let mut sub = h.engine.subscribe();
    let done = h
        .engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Same)
        .await
        .unwrap();
    assert_eq!(done.episode.id, original.id);
    assert_eq!(done.episode.identity.key, original.identity.key);
    assert_eq!(done.episode.identity.guid_key, candidate.identity.guid_key);
    assert_eq!(done.episode.missing_streak, 0, "present in the feed again");
    assert!(matches!(
        h.engine.episode(candidate.id).await,
        Err(UguisuError::NotFound { .. })
    ));
    let events = drain(&mut sub);
    let resolved = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::EpisodeDuplicateResolved { .. }))
        .expect("episode.duplicate_resolved");
    assert_eq!(resolved.episode_id, Some(original.id));
    let mut conn = h.engine.storage().reader().await.unwrap();
    let log = changes::list_for_episode(&mut conn, original.id)
        .await
        .unwrap();
    let fields: Vec<(&str, Option<&str>)> = log
        .iter()
        .map(|c| (c.field.as_str(), c.new_value.as_deref()))
        .collect();
    assert!(
        fields.contains(&("duplicate_resolved", Some("same"))),
        "{fields:?}"
    );
    assert!(fields.iter().any(|(f, _)| *f == "guid"), "{fields:?}");
    drop(conn);

    let r = h.refresh(id, true).await;
    assert_eq!((r.episodes.added, r.episodes.ambiguous), (0, 0), "{r:?}");
    assert!(
        !r.warnings
            .iter()
            .any(|w| w.contains("matched stored episode")),
        "the adopted GUID matches without a fallback: {:?}",
        r.warnings
    );
    assert!(
        h.engine
            .duplicates(Some(id), None, 10)
            .await
            .unwrap()
            .duplicates
            .is_empty()
    );
    assert_eq!(
        h.engine
            .episodes(id, None, 10)
            .await
            .unwrap()
            .episodes
            .len(),
        3
    );
}

#[tokio::test]
async fn separated_candidate_stays_separate() {
    let h = Harness::new().await;
    let (id, candidate, original) = candidate(&h).await;
    let done = h
        .engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Separate)
        .await
        .unwrap();
    assert_eq!(done.episode.id, candidate.id);
    assert_eq!(
        (
            done.episode.archive_state,
            done.episode.duplicate_of_episode_id
        ),
        (ArchiveState::Expected, None)
    );
    assert!(!done.queued, "the default policy queues nothing");

    let r = h.refresh(id, true).await;
    assert_eq!((r.episodes.added, r.episodes.ambiguous), (0, 0), "{r:?}");
    let again = h.engine.episode(candidate.id).await.unwrap().episode;
    assert_eq!(again.duplicate_of_episode_id, None);
    assert!(h.engine.episode(original.id).await.is_ok());
}

#[tokio::test]
async fn separation_runs_the_policy() {
    let h = Harness::new().await;
    let (id, candidate, _) = candidate(&h).await;
    h.engine
        .set_policy(&ArchivePolicy {
            podcast_id: id,
            mode: PolicyMode::Auto,
            max_backlog: None,
            max_age_days: None,
            priority: None,
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    let done = h
        .engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Separate)
        .await
        .unwrap();
    assert!(done.queued);
    assert!(h.engine.episode(candidate.id).await.unwrap().job.is_some());
}

#[tokio::test]
async fn both_in_feed_refuse_merge() {
    let h = Harness::new().await;
    let (id, candidate, _) = candidate(&h).await;
    let v1 = served(&h, "episodes_v1.xml");
    let start = v1.find("<item>").unwrap();
    let end = v1.find("</item>").unwrap() + "</item>".len();
    let both = served(&h, "guid_changed.xml").replacen(
        "<item>",
        &format!("{}\n    <item>", &v1[start..end]),
        1,
    );
    h.reset().await;
    h.serve("/feed.xml", both.as_bytes(), None, None).await;
    let r = h.refresh(id, true).await;
    assert_eq!(r.episodes.added, 0, "{r:?}");

    let err = h
        .engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Same)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, UguisuError::Conflict(m) if m.contains("both in the feed")),
        "{err}"
    );
    h.engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Separate)
        .await
        .unwrap();
}

#[tokio::test]
async fn archived_candidate_refuses_merge() {
    let h = Harness::new().await;
    let (id, candidate, _) = candidate(&h).await;
    let now = OffsetDateTime::now_utc();
    {
        let mut w = h.engine.storage().writer().await.unwrap();
        archive_files::upsert(
            &mut w,
            &ArchiveFile {
                id: ArchiveFileId::new(),
                episode_id: candidate.id,
                podcast_id: id,
                relative_path: "Versioned Show/one.mp3".into(),
                size_bytes: 4096,
                content_type: Some("audio/mpeg".into()),
                sniffed_type: Some("mp3".into()),
                hash_algo: "sha256".into(),
                hash_value: "abc".into(),
                source_size_bytes: Some(4096),
                source_hash_algo: Some("sha256".into()),
                source_hash_value: Some("abc".into()),
                mtime_unix: None,
                origin: ArchiveOrigin::Import,
                tag_state: TagState::Untagged,
                tag_mode: None,
                tagged_at: None,
                sidecar_written_at: None,
                original_tags: None,
                source_changed_at: None,
                verification_state: VerificationState::Unchecked,
                verification_reason: None,
                verified_at: None,
                registered_at: now,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        episodes::set_archive_state(&mut w, candidate.id, ArchiveState::Archived, None, now)
            .await
            .unwrap();
    }
    let err = h
        .engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Same)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, UguisuError::Conflict(m) if m.contains("archived file of its own")),
        "{err}"
    );
    let done = h
        .engine
        .resolve_duplicate(candidate.id, DuplicateResolution::Separate)
        .await
        .unwrap();
    assert_eq!(done.episode.archive_state, ArchiveState::Archived);
}

#[tokio::test]
async fn only_candidates_can_be_resolved() {
    let h = Harness::new().await;
    let (_, _, original) = candidate(&h).await;
    let err = h
        .engine
        .resolve_duplicate(original.id, DuplicateResolution::Same)
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::Conflict(_)), "{err}");
    let err = h
        .engine
        .resolve_duplicate(EpisodeId::new(), DuplicateResolution::Separate)
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::NotFound { .. }), "{err}");
}

#[tokio::test]
async fn candidate_cursor_stays_in_filter() {
    let h = Harness::new().await;
    let (id, candidate, original) = candidate(&h).await;
    h.serve("/other.xml", &synthetic_feed(1), None, None).await;
    let other = h
        .engine
        .add_podcast(&h.url("/other.xml"), CancellationToken::new())
        .await
        .unwrap()
        .podcast
        .id;

    let all = h.engine.duplicates(None, None, 10).await.unwrap();
    assert_eq!(all.duplicates.len(), 1);
    assert_eq!(all.next_after, None);
    for (podcast, after) in [
        (Some(other), Some(candidate.id)),
        (Some(id), Some(original.id)),
        (None, Some(EpisodeId::new())),
    ] {
        let err = h.engine.duplicates(podcast, after, 10).await.unwrap_err();
        assert!(
            matches!(err, UguisuError::Invalid(_)),
            "{podcast:?} {after:?}: {err}"
        );
    }
    let err = h
        .engine
        .duplicates(Some(PodcastId::new()), None, 10)
        .await
        .unwrap_err();
    assert!(matches!(err, UguisuError::NotFound { .. }), "{err}");
}
