//! The per-launch credential's lifetime (ADR 0042).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use uguisu_core::config::{Config, DataConfig};
use uguisu_engine::Engine;

#[path = "../src/launch.rs"]
mod launch;

async fn engine(dir: &tempfile::TempDir) -> Engine {
    let mut config = Config::from_lookup(|_| None).unwrap();
    config.data = DataConfig {
        data_dir: Some(dir.path().to_path_buf()),
        media_dir: None,
    };
    Engine::open(config).await.unwrap()
}

#[tokio::test]
async fn the_credential_is_handed_out_once_and_then_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir).await;
    let launch = launch::Launch::mint(&engine).await.unwrap();

    let secret = launch.take().expect("the first caller gets it");
    assert!(
        launch.take().is_none(),
        "a second caller must get nothing: the page needs it once"
    );
    assert!(
        engine.token_for(secret.expose()).await.unwrap().is_some(),
        "while the shell is up the credential still authenticates"
    );

    launch.revoke(&engine).await;
    assert!(
        engine.token_for(secret.expose()).await.unwrap().is_none(),
        "after shutdown the credential authenticates nothing"
    );
    engine.close().await;
}

#[tokio::test]
async fn an_unused_credential_is_revoked_too() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir).await;
    let launch = launch::Launch::mint(&engine).await.unwrap();
    launch.revoke(&engine).await;
    let secret = launch.take();
    assert!(
        secret.is_none(),
        "revoking spends it, so nothing can be handed out afterwards"
    );
    engine.close().await;
}
