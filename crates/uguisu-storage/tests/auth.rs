//! The credential, its sessions and its tokens, against a real database
//! (migration 0006).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use time::OffsetDateTime;
use uguisu_core::auth::{ApiToken, Scope, Session};
use uguisu_core::ids::{ApiTokenId, SessionId};
use uguisu_storage::{Storage, auth};

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(secs)
}

fn session(id: SessionId, digest: &str, idle: i64, absolute: i64) -> Session {
    Session {
        id,
        token_digest: digest.to_owned(),
        csrf_token: "c".repeat(64),
        created_at: at(0),
        last_seen_at: at(0),
        idle_expires_at: at(idle),
        absolute_expires_at: at(absolute),
        revoked_at: None,
    }
}

#[tokio::test]
async fn a_credential_is_set_then_replaced() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    assert!(!auth::credential_set(&mut w).await.unwrap());
    assert!(auth::credential(&mut w).await.unwrap().is_none());

    auth::set_credential(&mut w, "uguisu", "$argon2id$first", at(10))
        .await
        .unwrap();
    assert!(auth::credential_set(&mut w).await.unwrap());
    let c = auth::credential(&mut w).await.unwrap().unwrap();
    assert_eq!(c.username, "uguisu");
    assert_eq!(c.password_hash, "$argon2id$first");
    assert_eq!(c.created_at, at(10));
    assert_eq!(c.updated_at, at(10));

    auth::set_credential(&mut w, "uguisu", "$argon2id$second", at(20))
        .await
        .unwrap();
    let c = auth::credential(&mut w).await.unwrap().unwrap();
    assert_eq!(c.password_hash, "$argon2id$second");
    assert_eq!(c.created_at, at(10), "the first setting is when it began");
    assert_eq!(c.updated_at, at(20));
}

#[tokio::test]
async fn only_one_credential_row_can_exist() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    auth::set_credential(&mut w, "a", "$h", at(0))
        .await
        .unwrap();
    auth::set_credential(&mut w, "b", "$h", at(1))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_credential")
        .fetch_one(&mut *w)
        .await
        .unwrap();
    assert_eq!(count, 1);
    // The CHECK, not just the upsert, is what holds this.
    let refused = sqlx::query("INSERT INTO auth_credential (id, username, password_hash, created_at, updated_at) VALUES (2, 'c', '$h', '', '')")
        .execute(&mut *w)
        .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_session_is_found_by_its_digest_only() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let id = SessionId::new();
    let digest = "a".repeat(64);
    auth::open_session(&mut w, &session(id, &digest, 100, 1000))
        .await
        .unwrap();

    let found = auth::session_by_digest(&mut w, &digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, id);
    assert!(found.usable(at(50)));
    assert!(
        auth::session_by_digest(&mut w, &"b".repeat(64))
            .await
            .unwrap()
            .is_none()
    );
    // The id is not a credential: knowing it opens nothing.
    assert!(
        auth::session_by_digest(&mut w, &id.to_string())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_session_digest_cannot_repeat() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let digest = "a".repeat(64);
    auth::open_session(&mut w, &session(SessionId::new(), &digest, 100, 1000))
        .await
        .unwrap();
    let again = auth::open_session(&mut w, &session(SessionId::new(), &digest, 100, 1000)).await;
    assert!(again.is_err(), "two sessions cannot share one cookie");
}

#[tokio::test]
async fn touching_a_session_moves_its_idle_deadline() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let id = SessionId::new();
    let digest = "a".repeat(64);
    auth::open_session(&mut w, &session(id, &digest, 100, 1000))
        .await
        .unwrap();
    auth::touch_session(&mut w, id, at(90), at(190))
        .await
        .unwrap();
    let found = auth::session_by_digest(&mut w, &digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.last_seen_at, at(90));
    assert_eq!(found.idle_expires_at, at(190));
    assert!(found.usable(at(150)));
}

#[tokio::test]
async fn revoking_keeps_one_session_alive() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let keep = SessionId::new();
    let other = SessionId::new();
    auth::open_session(&mut w, &session(keep, &"a".repeat(64), 100, 1000))
        .await
        .unwrap();
    auth::open_session(&mut w, &session(other, &"b".repeat(64), 100, 1000))
        .await
        .unwrap();

    let revoked = auth::revoke_sessions_except(&mut w, Some(keep), at(10))
        .await
        .unwrap();
    assert_eq!(revoked, 1);
    assert!(
        auth::session_by_digest(&mut w, &"a".repeat(64))
            .await
            .unwrap()
            .unwrap()
            .usable(at(20))
    );
    assert!(
        !auth::session_by_digest(&mut w, &"b".repeat(64))
            .await
            .unwrap()
            .unwrap()
            .usable(at(20))
    );

    assert!(auth::revoke_session(&mut w, keep, at(30)).await.unwrap());
    assert!(
        !auth::revoke_session(&mut w, keep, at(40)).await.unwrap(),
        "revoking twice is not a second revocation"
    );
}

#[tokio::test]
async fn pruning_removes_only_what_cannot_authenticate() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let live = SessionId::new();
    let idle = SessionId::new();
    let absolute = SessionId::new();
    let revoked = SessionId::new();
    auth::open_session(&mut w, &session(live, &"a".repeat(64), 1000, 2000))
        .await
        .unwrap();
    auth::open_session(&mut w, &session(idle, &"b".repeat(64), 10, 2000))
        .await
        .unwrap();
    auth::open_session(&mut w, &session(absolute, &"c".repeat(64), 1000, 20))
        .await
        .unwrap();
    auth::open_session(&mut w, &session(revoked, &"d".repeat(64), 1000, 2000))
        .await
        .unwrap();
    auth::revoke_session(&mut w, revoked, at(5)).await.unwrap();

    let gone = auth::prune_sessions(&mut w, at(100)).await.unwrap();
    assert_eq!(gone, 3);
    assert!(
        auth::session_by_digest(&mut w, &"a".repeat(64))
            .await
            .unwrap()
            .is_some(),
        "a usable session survives housekeeping"
    );
}

#[tokio::test]
async fn a_token_round_trips_with_its_scope() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let id = ApiTokenId::new();
    let digest = "e".repeat(64);
    let token = ApiToken {
        id,
        name: "laptop".to_owned(),
        scope: Scope::Read,
        created_at: at(0),
        last_used_at: None,
        expires_at: Some(at(500)),
        revoked_at: None,
    };
    auth::issue_token(&mut w, &token, &digest).await.unwrap();

    let found = auth::token_by_digest(&mut w, &digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, token);
    assert!(found.usable(at(100)));
    assert!(!found.usable(at(600)));

    auth::touch_token(&mut w, id, at(100)).await.unwrap();
    assert!(auth::revoke_token(&mut w, id, at(200)).await.unwrap());
    assert!(!auth::revoke_token(&mut w, id, at(300)).await.unwrap());

    let listed = auth::list_tokens(&mut w).await.unwrap();
    assert_eq!(listed.len(), 1, "a revoked token stays listed");
    assert_eq!(listed[0].last_used_at, Some(at(100)));
    assert_eq!(listed[0].revoked_at, Some(at(200)));
    assert!(!listed[0].usable(at(250)));
}

#[tokio::test]
async fn a_token_digest_cannot_repeat() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    let digest = "f".repeat(64);
    let mut token = ApiToken {
        id: ApiTokenId::new(),
        name: "one".to_owned(),
        scope: Scope::Write,
        created_at: at(0),
        last_used_at: None,
        expires_at: None,
        revoked_at: None,
    };
    auth::issue_token(&mut w, &token, &digest).await.unwrap();
    token.id = ApiTokenId::new();
    assert!(auth::issue_token(&mut w, &token, &digest).await.is_err());
}

#[tokio::test]
async fn an_unknown_scope_in_the_database_is_refused() {
    let s = Storage::open_temp().await.unwrap();
    let mut w = s.writer().await.unwrap();
    // The CHECK constraint is the first line of defence.
    let refused = sqlx::query(
        "INSERT INTO auth_tokens (id, name, token_digest, scope, created_at) \
         VALUES ('01J0000000000000000000000T', 'x', 'g', 'admin', '1970-01-01T00:00:00Z')",
    )
    .execute(&mut *w)
    .await;
    assert!(refused.is_err(), "`admin` is not a scope this API has");
}
