//! The credential, its sessions and its tokens, through the engine.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use time::OffsetDateTime;
use uguisu_core::auth::Scope;
use uguisu_core::secret::Secret;

fn password(s: &str) -> Secret<String> {
    Secret::new(s.to_owned())
}

#[tokio::test]
async fn authentication_is_off_until_a_password_is_set() {
    let h = Harness::new().await;
    assert!(!h.engine.credential_set().await.unwrap());
    assert!(h.engine.credential_username().await.unwrap().is_none());

    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();
    assert!(h.engine.credential_set().await.unwrap());
    assert_eq!(
        h.engine.credential_username().await.unwrap(),
        Some("uguisu".to_owned())
    );
    h.engine.close().await;
}

#[tokio::test]
async fn a_password_is_verified_not_stored() {
    let h = Harness::new().await;
    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();

    // The stored value is an argon2id PHC string, not the password.
    let mut r = h.engine.storage().reader().await.unwrap();
    let stored = uguisu_storage::auth::credential(&mut r)
        .await
        .unwrap()
        .unwrap();
    drop(r);
    assert!(
        stored.password_hash.starts_with("$argon2id$v=19$"),
        "{}",
        stored.password_hash
    );
    assert!(!stored.password_hash.contains("correct horse"));

    assert!(
        h.engine
            .login("uguisu", &password("correct horse"))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        h.engine
            .login("uguisu", &password("correct hors"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        h.engine
            .login("someone", &password("correct horse"))
            .await
            .unwrap()
            .is_none(),
        "the username has to match too"
    );
    h.engine.close().await;
}

/// An unknown username must cost the same as a wrong password, or the timing
/// answers "does this account exist". Generous bounds: this asserts the dummy
/// verification happens at all, not a precise duration.
#[tokio::test]
async fn an_unknown_username_still_costs_a_hash() {
    let h = Harness::new().await;
    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();

    let wrong_password = std::time::Instant::now();
    h.engine
        .login("uguisu", &password("wrong wrong"))
        .await
        .unwrap();
    let wrong_password = wrong_password.elapsed();

    let unknown_name = std::time::Instant::now();
    h.engine
        .login("nobody", &password("wrong wrong"))
        .await
        .unwrap();
    let unknown_name = unknown_name.elapsed();

    assert!(
        unknown_name * 4 > wrong_password,
        "an unknown username returned far too fast to have hashed anything: \
         {unknown_name:?} against {wrong_password:?}"
    );
    h.engine.close().await;
}

#[tokio::test]
async fn a_login_opens_a_session_the_cookie_alone_finds() {
    let h = Harness::new().await;
    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();
    let opened = h
        .engine
        .login("uguisu", &password("correct horse"))
        .await
        .unwrap()
        .unwrap();

    let found = h
        .engine
        .session_for(opened.cookie.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, opened.session.id);
    assert_eq!(found.csrf_token, opened.session.csrf_token);

    // The stored digest is not the cookie, and the id is not a credential.
    assert_ne!(found.token_digest, *opened.cookie.expose());
    assert!(
        h.engine
            .session_for(&found.id.to_string())
            .await
            .unwrap()
            .is_none()
    );
    assert!(h.engine.session_for("nonsense").await.unwrap().is_none());
    h.engine.close().await;
}

#[tokio::test]
async fn logging_out_closes_the_session() {
    let h = Harness::new().await;
    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();
    let opened = h
        .engine
        .login("uguisu", &password("correct horse"))
        .await
        .unwrap()
        .unwrap();
    assert!(h.engine.logout(opened.session.id).await.unwrap());
    assert!(
        !h.engine.logout(opened.session.id).await.unwrap(),
        "closing it twice is not a second close"
    );
    assert!(
        h.engine
            .session_for(opened.cookie.expose())
            .await
            .unwrap()
            .is_none()
    );
    h.engine.close().await;
}

#[tokio::test]
async fn changing_the_password_closes_the_other_sessions() {
    let h = Harness::new().await;
    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();
    let kept = h
        .engine
        .login("uguisu", &password("correct horse"))
        .await
        .unwrap()
        .unwrap();
    let elsewhere = h
        .engine
        .login("uguisu", &password("correct horse"))
        .await
        .unwrap()
        .unwrap();

    let revoked = h
        .engine
        .set_password("uguisu", &password("new staple"), Some(kept.session.id))
        .await
        .unwrap();
    assert_eq!(revoked, 1);
    assert!(
        h.engine
            .session_for(kept.cookie.expose())
            .await
            .unwrap()
            .is_some(),
        "the browser that changed it stays logged in"
    );
    assert!(
        h.engine
            .session_for(elsewhere.cookie.expose())
            .await
            .unwrap()
            .is_none(),
        "a session opened with the old password must not outlive it"
    );
    assert!(
        h.engine
            .login("uguisu", &password("correct horse"))
            .await
            .unwrap()
            .is_none()
    );
    h.engine.close().await;
}

#[tokio::test]
async fn a_password_has_to_be_long_enough() {
    let h = Harness::new().await;
    assert!(
        h.engine
            .set_password("uguisu", &password("short"), None)
            .await
            .is_err()
    );
    assert!(
        h.engine
            .set_password("", &password("long enough"), None)
            .await
            .is_err()
    );
    assert!(!h.engine.credential_set().await.unwrap());
    h.engine.close().await;
}

#[tokio::test]
async fn a_token_is_shown_once_and_stored_hashed() {
    let h = Harness::new().await;
    let issued = h
        .engine
        .issue_token("laptop", Scope::Read, None)
        .await
        .unwrap();
    assert_eq!(issued.token.name, "laptop");
    assert_eq!(issued.token.scope, Scope::Read);

    let found = h
        .engine
        .token_for(issued.secret.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, issued.token.id);
    assert!(h.engine.token_for("nonsense").await.unwrap().is_none());

    // Nothing that is listed can be presented.
    let listed = h.engine.list_tokens().await.unwrap();
    assert_eq!(listed.len(), 1);
    let serialized = serde_json::to_string(&listed).unwrap();
    assert!(
        !serialized.contains(issued.secret.expose()),
        "the secret must not survive anywhere it can be read back"
    );

    assert!(h.engine.revoke_token(issued.token.id).await.unwrap());
    assert!(
        h.engine
            .token_for(issued.secret.expose())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        h.engine.list_tokens().await.unwrap().len(),
        1,
        "a revoked token stays listed so it can be recognised"
    );
    h.engine.close().await;
}

#[tokio::test]
async fn an_expired_token_stops_working() {
    let h = Harness::new().await;
    let past = OffsetDateTime::now_utc() - time::Duration::seconds(1);
    assert!(
        h.engine
            .issue_token("stale", Scope::Write, Some(past))
            .await
            .is_err(),
        "issuing one that is already expired is refused"
    );
    let issued = h
        .engine
        .issue_token("brief", Scope::Write, None)
        .await
        .unwrap();
    assert!(
        h.engine
            .token_for(issued.secret.expose())
            .await
            .unwrap()
            .is_some()
    );

    // Move the deadline into the past rather than waiting for one to arrive:
    // what is under test is that `token_for` reads it, not the clock.
    let mut w = h.engine.storage().writer().await.unwrap();
    sqlx::query("UPDATE auth_tokens SET expires_at = ?1 WHERE id = ?2")
        .bind("1971-01-01T00:00:00Z")
        .bind(issued.token.id.to_string())
        .execute(&mut *w)
        .await
        .unwrap();
    drop(w);
    assert!(
        h.engine
            .token_for(issued.secret.expose())
            .await
            .unwrap()
            .is_none()
    );
    h.engine.close().await;
}

#[tokio::test]
async fn maintenance_closes_what_can_no_longer_authenticate() {
    let h = Harness::new().await;
    h.engine
        .set_password("uguisu", &password("correct horse"), None)
        .await
        .unwrap();
    let live = h
        .engine
        .login("uguisu", &password("correct horse"))
        .await
        .unwrap()
        .unwrap();
    let doomed = h
        .engine
        .login("uguisu", &password("correct horse"))
        .await
        .unwrap()
        .unwrap();
    h.engine.logout(doomed.session.id).await.unwrap();

    let report = h.engine.run_maintenance().await.unwrap();
    assert_eq!(report.sessions_pruned, 1);
    assert!(
        h.engine
            .session_for(live.cookie.expose())
            .await
            .unwrap()
            .is_some()
    );
    h.engine.close().await;
}
