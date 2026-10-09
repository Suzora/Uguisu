//! Persisted settings end to end (ADR 0028): what a write changes, what
//! it refuses, and what survives a restart.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use uguisu_core::config::Origin;
use uguisu_core::events::EventKind;

#[tokio::test]
async fn a_stored_setting_outlives_the_process() {
    let mut h = Harness::new().await;
    assert_eq!(h.engine.config().archive.max_backlog, 3);
    assert_eq!(
        h.engine.config().origin("UGUISU_ARCHIVE_MAX_BACKLOG"),
        Origin::Default
    );

    let mut events = h.engine.subscribe();
    let described = h
        .engine
        .set_setting("UGUISU_ARCHIVE_MAX_BACKLOG", "7", Some("test"))
        .await
        .unwrap();
    assert_eq!(described.value, "7");
    assert_eq!(described.origin, Origin::Settings);
    assert!(!described.pinned && described.persistable && described.live);
    assert_eq!(h.engine.config().archive.max_backlog, 7);

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.kind,
        EventKind::SettingsChanged {
            key: "UGUISU_ARCHIVE_MAX_BACKLOG".to_owned(),
            removed: false,
            restart_required: false,
        }
    );

    // A restart reads it back: this is the whole point of the table.
    h.restart(None).await;
    assert_eq!(h.engine.config().archive.max_backlog, 7);
    assert_eq!(
        h.engine.config().origin("UGUISU_ARCHIVE_MAX_BACKLOG"),
        Origin::Settings
    );

    assert!(
        h.engine
            .unset_setting("UGUISU_ARCHIVE_MAX_BACKLOG", Some("test"))
            .await
            .unwrap()
    );
    assert_eq!(h.engine.config().archive.max_backlog, 3);
    assert!(
        !h.engine
            .unset_setting("UGUISU_ARCHIVE_MAX_BACKLOG", Some("test"))
            .await
            .unwrap(),
        "clearing what is not there is not an error, and not a change"
    );
}

#[tokio::test]
async fn stored_setting_keeps_media_root() {
    // The harness assigns its data directory by hand, as `--data-dir` and
    // the desktop's archive folder do.
    let mut h = Harness::new().await;
    let root = h.media_dir();
    h.engine
        .set_setting("UGUISU_FEED_REMOVAL_STREAK", "3", Some("test"))
        .await
        .unwrap();
    assert_eq!(h.media_dir(), root);

    // Opening with a stored row merges it at startup too.
    h.restart(None).await;
    assert_eq!(h.engine.config().feed.removal_streak, 3);
    assert_eq!(h.media_dir(), root);
}

#[tokio::test]
async fn restart_applies_stored_setting() {
    let mut h = Harness::new().await;
    let described = h
        .engine
        .set_setting("UGUISU_DOWNLOAD_MAX_ATTEMPTS", "5", Some("test"))
        .await
        .unwrap();
    assert!(!described.live);
    assert_eq!(h.engine.downloads().deps().config.max_attempts, 3);

    h.restart(None).await;
    assert_eq!(h.engine.downloads().deps().config.max_attempts, 5);
}

#[tokio::test]
async fn an_ineffective_write_is_refused() {
    let h = Harness::new().await;
    let refused = |e: uguisu_core::UguisuError| e.kind().to_owned();

    // Nothing Uguisu reads.
    assert_eq!(
        refused(
            h.engine
                .set_setting("UGUISU_NOT_A_KEY", "1", None)
                .await
                .unwrap_err()
        ),
        "invalid"
    );
    // Never in the database: a secret, a directory, the SSRF allowlist.
    for key in [
        "UGUISU_PODCASTINDEX_SECRET",
        "UGUISU_DATA_DIR",
        "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS",
    ] {
        assert_eq!(
            refused(h.engine.set_setting(key, "x", None).await.unwrap_err()),
            "invalid",
            "{key} must never be storable"
        );
    }
    // Invalid on its own, and invalid in company — both quote the parser.
    let err = h
        .engine
        .set_setting("UGUISU_ARCHIVE_MAX_BACKLOG", "lots", None)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "invalid");
    assert!(err.to_string().contains("UGUISU_ARCHIVE_MAX_BACKLOG"));
    let err = h
        .engine
        .set_setting("UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY", "99", None)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY"),
        "{err}"
    );

    // Nothing was written by any of that.
    let mut reader = h.engine.storage().reader().await.unwrap();
    assert!(
        uguisu_storage::settings::list(&mut reader)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!h.engine.settings_report().has_problems());
}

/// A credential can only reach the settings table through a hand-written row —
/// `set_setting` and `config set` both refuse an unstorable key — and the
/// report is what would hand it back to anything that can read
/// `GET /api/v1/settings`.
#[tokio::test]
async fn a_stored_secret_is_never_echoed_back() {
    let mut h = Harness::new().await;
    let mut tx = h.engine.storage().begin().await.unwrap();
    uguisu_storage::settings::set(
        &mut tx,
        "UGUISU_PODCASTINDEX_SECRET",
        "s3cret-value",
        Some("sqlite3"),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    h.restart(None).await;
    let report = h.engine.settings_report();
    let printed = serde_json::to_string(&report).unwrap();
    assert!(
        !printed.contains("s3cret-value"),
        "the settings report echoed a credential: {printed}"
    );
    let unused = report
        .unused
        .iter()
        .find(|u| u.key == "UGUISU_PODCASTINDEX_SECRET")
        .expect("a stored unstorable key is reported, not silently dropped");
    assert_eq!(unused.value, "[redacted]");
    assert_eq!(unused.reason, "not persistable");

    // And the row is still there: it is the only copy of what somebody meant.
    let mut reader = h.engine.storage().reader().await.unwrap();
    assert!(
        uguisu_storage::settings::get(&mut reader, "UGUISU_PODCASTINDEX_SECRET")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_value_that_stops_parsing_stays() {
    let mut h = Harness::new().await;
    // Written behind the engine's back, the way a hand-edited database or
    // a downgrade produces one: a row the current parser refuses.
    let mut tx = h.engine.storage().begin().await.unwrap();
    uguisu_storage::settings::set(
        &mut tx,
        "UGUISU_ARCHIVE_MAX_BACKLOG",
        "seven",
        Some("sqlite3"),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    h.restart(None).await;
    // The engine started, and started with the default.
    assert_eq!(h.engine.config().archive.max_backlog, 3);
    let report = h.engine.settings_report();
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].key, "UGUISU_ARCHIVE_MAX_BACKLOG");
    assert_eq!(report.rejected[0].value, "seven");

    // The row is still there, unchanged, because it is the only copy of
    // what somebody meant.
    let mut reader = h.engine.storage().reader().await.unwrap();
    let stored = uguisu_storage::settings::get(&mut reader, "UGUISU_ARCHIVE_MAX_BACKLOG")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.value, "seven");
    assert_eq!(stored.updated_by.as_deref(), Some("sqlite3"));
    drop(reader);

    // It is recorded, so the log explains the warning.
    let mut reader = h.engine.storage().reader().await.unwrap();
    let logged = uguisu_storage::events::list_after(&mut reader, None, 100)
        .await
        .unwrap();
    assert!(
        logged.iter().any(|e| e.name() == "settings.rejected"
            && matches!(&e.kind, EventKind::SettingsRejected { key, .. }
                    if key == "UGUISU_ARCHIVE_MAX_BACKLOG")),
        "the quarantine is not in the event log"
    );
    drop(reader);

    // Writing a value that parses clears it; so would `unset`.
    h.engine
        .set_setting("UGUISU_ARCHIVE_MAX_BACKLOG", "7", Some("test"))
        .await
        .unwrap();
    assert_eq!(h.engine.config().archive.max_backlog, 7);
    assert!(h.engine.rejected_settings().is_empty());
}
