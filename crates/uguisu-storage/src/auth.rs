//! `auth_credential`, `auth_sessions` and `auth_tokens` (ADR 0035).
//!
//! Nothing here hashes or compares anything: this module stores digests and
//! returns them, and `uguisu-engine`'s `auth` module is the only place a
//! plaintext secret exists. A session and a token are both looked up *by
//! digest*, so no query ever takes a caller's secret as a value to match.

use sqlx::{FromRow, SqliteConnection};
use time::OffsetDateTime;
use uguisu_core::auth::{ApiToken, Credential, Scope, Session};
use uguisu_core::ids::{ApiTokenId, SessionId};

use crate::{Result, row, to_db_ts};

const CREDENTIAL: &str = "auth_credential";
const SESSIONS: &str = "auth_sessions";
const TOKENS: &str = "auth_tokens";

#[derive(FromRow)]
struct CredentialRow {
    username: String,
    password_hash: String,
    created_at: String,
    updated_at: String,
}

impl CredentialRow {
    fn into_model(self) -> Result<Credential> {
        Ok(Credential {
            created_at: row::ts(CREDENTIAL, "1", &self.created_at)?,
            updated_at: row::ts(CREDENTIAL, "1", &self.updated_at)?,
            username: self.username,
            password_hash: self.password_hash,
        })
    }
}

/// The credential, when one has been set.
pub async fn credential(conn: &mut SqliteConnection) -> Result<Option<Credential>> {
    let row: Option<CredentialRow> = sqlx::query_as(
        "SELECT username, password_hash, created_at, updated_at FROM auth_credential WHERE id = 1",
    )
    .fetch_optional(conn)
    .await?;
    row.map(CredentialRow::into_model).transpose()
}

/// Whether a credential exists, without reading the hash.
///
/// This is what decides whether authentication is on, so it runs on startup
/// and on every login attempt; the hash is not wanted at either.
pub async fn credential_set(conn: &mut SqliteConnection) -> Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_credential WHERE id = 1")
        .fetch_one(conn)
        .await?;
    Ok(count > 0)
}

/// Sets or replaces the credential.
pub async fn set_credential(
    conn: &mut SqliteConnection,
    username: &str,
    password_hash: &str,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO auth_credential (id, username, password_hash, created_at, updated_at) \
         VALUES (1, ?1, ?2, ?3, ?3) \
         ON CONFLICT (id) DO UPDATE SET username = ?1, password_hash = ?2, updated_at = ?3",
    )
    .bind(username)
    .bind(password_hash)
    .bind(to_db_ts(now))
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct SessionRow {
    id: String,
    token_digest: String,
    csrf_token: String,
    created_at: String,
    last_seen_at: String,
    idle_expires_at: String,
    absolute_expires_at: String,
    revoked_at: Option<String>,
}

impl SessionRow {
    fn into_model(self) -> Result<Session> {
        Ok(Session {
            id: row::id(SESSIONS, &self.id, &self.id)?,
            created_at: row::ts(SESSIONS, &self.id, &self.created_at)?,
            last_seen_at: row::ts(SESSIONS, &self.id, &self.last_seen_at)?,
            idle_expires_at: row::ts(SESSIONS, &self.id, &self.idle_expires_at)?,
            absolute_expires_at: row::ts(SESSIONS, &self.id, &self.absolute_expires_at)?,
            revoked_at: row::opt_ts(SESSIONS, &self.id, self.revoked_at.as_deref())?,
            token_digest: self.token_digest,
            csrf_token: self.csrf_token,
        })
    }
}

const SESSION_COLUMNS: &str = "id, token_digest, csrf_token, created_at, last_seen_at, \
                               idle_expires_at, absolute_expires_at, revoked_at";

/// Opens a session. `token_digest` is the SHA-256 of the cookie's secret.
pub async fn open_session(conn: &mut SqliteConnection, session: &Session) -> Result<()> {
    sqlx::query(
        "INSERT INTO auth_sessions (id, token_digest, csrf_token, created_at, last_seen_at, \
         idle_expires_at, absolute_expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(session.id.to_string())
    .bind(&session.token_digest)
    .bind(&session.csrf_token)
    .bind(to_db_ts(session.created_at))
    .bind(to_db_ts(session.last_seen_at))
    .bind(to_db_ts(session.idle_expires_at))
    .bind(to_db_ts(session.absolute_expires_at))
    .execute(conn)
    .await?;
    Ok(())
}

/// The session a cookie's digest belongs to, expired or not.
///
/// Expiry is the caller's to judge, so that one read answers both "is this a
/// session" and "may it still be used".
pub async fn session_by_digest(
    conn: &mut SqliteConnection,
    token_digest: &str,
) -> Result<Option<Session>> {
    let row: Option<SessionRow> = sqlx::query_as(&format!(
        "SELECT {SESSION_COLUMNS} FROM auth_sessions WHERE token_digest = ?1"
    ))
    .bind(token_digest)
    .fetch_optional(conn)
    .await?;
    row.map(SessionRow::into_model).transpose()
}

/// Moves a session's activity forward.
pub async fn touch_session(
    conn: &mut SqliteConnection,
    id: SessionId,
    last_seen_at: OffsetDateTime,
    idle_expires_at: OffsetDateTime,
) -> Result<()> {
    sqlx::query("UPDATE auth_sessions SET last_seen_at = ?2, idle_expires_at = ?3 WHERE id = ?1")
        .bind(id.to_string())
        .bind(to_db_ts(last_seen_at))
        .bind(to_db_ts(idle_expires_at))
        .execute(conn)
        .await?;
    Ok(())
}

/// Revokes one session. Returns whether it was open.
pub async fn revoke_session(
    conn: &mut SqliteConnection,
    id: SessionId,
    now: OffsetDateTime,
) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE auth_sessions SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
    )
    .bind(id.to_string())
    .bind(to_db_ts(now))
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Revokes every session except `keep`, which a password change uses so the
/// operator is not logged out of the browser they changed it in.
pub async fn revoke_sessions_except(
    conn: &mut SqliteConnection,
    keep: Option<SessionId>,
    now: OffsetDateTime,
) -> Result<u64> {
    let done = sqlx::query(
        "UPDATE auth_sessions SET revoked_at = ?1 \
         WHERE revoked_at IS NULL AND (?2 IS NULL OR id <> ?2)",
    )
    .bind(to_db_ts(now))
    .bind(keep.map(|id| id.to_string()))
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// Deletes sessions that can no longer authenticate anything.
///
/// Called from the maintenance tick. A revoked or expired row carries no
/// information anybody asks for, unlike a revoked token.
pub async fn prune_sessions(conn: &mut SqliteConnection, now: OffsetDateTime) -> Result<u64> {
    let now = to_db_ts(now);
    let done = sqlx::query(
        "DELETE FROM auth_sessions \
         WHERE revoked_at IS NOT NULL OR idle_expires_at <= ?1 OR absolute_expires_at <= ?1",
    )
    .bind(now)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

#[derive(FromRow)]
struct TokenRow {
    id: String,
    name: String,
    scope: String,
    created_at: String,
    last_used_at: Option<String>,
    expires_at: Option<String>,
    revoked_at: Option<String>,
}

impl TokenRow {
    fn into_model(self) -> Result<ApiToken> {
        let scope = Scope::parse(&self.scope)
            .ok_or_else(|| row::corrupt(TOKENS, &self.id, format!("bad scope `{}`", self.scope)))?;
        Ok(ApiToken {
            id: row::id(TOKENS, &self.id, &self.id)?,
            created_at: row::ts(TOKENS, &self.id, &self.created_at)?,
            last_used_at: row::opt_ts(TOKENS, &self.id, self.last_used_at.as_deref())?,
            expires_at: row::opt_ts(TOKENS, &self.id, self.expires_at.as_deref())?,
            revoked_at: row::opt_ts(TOKENS, &self.id, self.revoked_at.as_deref())?,
            name: self.name,
            scope,
        })
    }
}

const TOKEN_COLUMNS: &str = "id, name, scope, created_at, last_used_at, expires_at, revoked_at";

/// Issues a token. `token_digest` is the SHA-256 of the secret handed out.
pub async fn issue_token(
    conn: &mut SqliteConnection,
    token: &ApiToken,
    token_digest: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO auth_tokens (id, name, token_digest, scope, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(token.id.to_string())
    .bind(&token.name)
    .bind(token_digest)
    .bind(token.scope.as_str())
    .bind(to_db_ts(token.created_at))
    .bind(token.expires_at.map(to_db_ts))
    .execute(conn)
    .await?;
    Ok(())
}

/// The token a presented secret's digest belongs to, revoked or not.
pub async fn token_by_digest(
    conn: &mut SqliteConnection,
    token_digest: &str,
) -> Result<Option<ApiToken>> {
    let row: Option<TokenRow> = sqlx::query_as(&format!(
        "SELECT {TOKEN_COLUMNS} FROM auth_tokens WHERE token_digest = ?1"
    ))
    .bind(token_digest)
    .fetch_optional(conn)
    .await?;
    row.map(TokenRow::into_model).transpose()
}

/// Every token, newest first. Revoked ones are included: "the one I wrote
/// down no longer works" is the answer the list exists to give.
pub async fn list_tokens(conn: &mut SqliteConnection) -> Result<Vec<ApiToken>> {
    let rows: Vec<TokenRow> = sqlx::query_as(&format!(
        "SELECT {TOKEN_COLUMNS} FROM auth_tokens ORDER BY created_at DESC, id DESC"
    ))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(TokenRow::into_model).collect()
}

/// Moves a token's last use forward.
pub async fn touch_token(
    conn: &mut SqliteConnection,
    id: ApiTokenId,
    last_used_at: OffsetDateTime,
) -> Result<()> {
    sqlx::query("UPDATE auth_tokens SET last_used_at = ?2 WHERE id = ?1")
        .bind(id.to_string())
        .bind(to_db_ts(last_used_at))
        .execute(conn)
        .await?;
    Ok(())
}

/// Revokes a token. Returns whether it was usable before.
pub async fn revoke_token(
    conn: &mut SqliteConnection,
    id: ApiTokenId,
    now: OffsetDateTime,
) -> Result<bool> {
    let done =
        sqlx::query("UPDATE auth_tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL")
            .bind(id.to_string())
            .bind(to_db_ts(now))
            .execute(conn)
            .await?;
    Ok(done.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::Storage;

    async fn plan(conn: &mut SqliteConnection, sql: &str) -> String {
        // The plan's four columns are (id, parent, notused, detail).
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}"))
                .bind("x")
                .fetch_all(conn)
                .await
                .unwrap();
        rows.into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// A lookup on every request must be a seek, not a table scan that grows
    /// with the number of sessions or tokens ever issued.
    #[tokio::test]
    async fn a_digest_lookup_is_a_seek() {
        let s = Storage::open_temp().await.unwrap();
        let mut r = s.reader().await.unwrap();
        for sql in [
            &format!("SELECT {SESSION_COLUMNS} FROM auth_sessions WHERE token_digest = ?1"),
            &format!("SELECT {TOKEN_COLUMNS} FROM auth_tokens WHERE token_digest = ?1"),
        ] {
            let plan = plan(&mut r, sql).await;
            assert!(
                plan.contains("USING INDEX") && !plan.contains("SCAN"),
                "{sql}\n{plan}"
            );
        }
    }

    /// Housekeeping runs on a timer over a table nothing else waits on, but a
    /// scan here would still be a scan of every session ever opened.
    #[tokio::test]
    async fn pruning_uses_the_expiry_index() {
        let s = Storage::open_temp().await.unwrap();
        let mut r = s.reader().await.unwrap();
        let plan = plan(
            &mut r,
            "SELECT id FROM auth_sessions WHERE idle_expires_at <= ?1",
        )
        .await;
        assert!(plan.contains("idx_auth_sessions_idle"), "{plan}");
    }
}
