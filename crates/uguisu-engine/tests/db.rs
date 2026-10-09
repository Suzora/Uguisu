//! Database maintenance (ADR 0056): a backup is a database that opens with
//! everything in it, never overwrites a file, and checking and vacuuming a
//! sound database find nothing and lose nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use uguisu_core::UguisuError;
use uguisu_storage::{Storage, podcasts};

#[tokio::test]
async fn backup_opens_with_every_row() {
    let h = Harness::new().await;
    let id = h.add_fixture("episodes_v1.xml").await.podcast.id;
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("copy.db");

    let backup = h.engine.backup_database(Some(&dest)).await.unwrap();
    assert_eq!(backup.path, dest.display().to_string());
    assert_eq!(backup.bytes, std::fs::metadata(&dest).unwrap().len());
    let names: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["copy.db"], "nothing beside the copy");

    let copy = Storage::open_path(&dest).await.unwrap();
    assert!(copy.schema().applied.is_empty(), "{:?}", copy.schema());
    let podcast = podcasts::get(&mut copy.reader().await.unwrap(), id)
        .await
        .unwrap();
    assert!(podcast.is_some(), "the podcast is in the copy");
    copy.close().await;

    let err = h.engine.backup_database(Some(&dest)).await.unwrap_err();
    assert!(matches!(err, UguisuError::Conflict(_)), "{err}");
}

#[tokio::test]
async fn backup_defaults_to_data_dir() {
    let h = Harness::new().await;
    let backup = h.engine.backup_database(None).await.unwrap();
    let path = std::path::Path::new(&backup.path);
    assert_eq!(path.parent().unwrap(), h.dir.path().join("backups"));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("uguisu-") && name.ends_with("Z.db"),
        "{name}"
    );
    assert!(path.is_file());
}

#[tokio::test]
async fn sound_database_checks_clean() {
    let h = Harness::new().await;
    h.add_fixture("episodes_v1.xml").await;
    let check = h.engine.check_database().await.unwrap();
    assert!(check.ok, "{check:?}");
    let vacuum = h.engine.vacuum_database().await.unwrap();
    assert!(vacuum.bytes_after > 0, "{vacuum:?}");
    assert_eq!(h.engine.list_podcasts().await.unwrap().len(), 1);
    assert!(h.engine.check_database().await.unwrap().ok);
}
