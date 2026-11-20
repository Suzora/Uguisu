//! Single-user authentication: one Argon2 password hash and server-side sessions.

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use sqlx::SqlitePool;
use uguisu_core::db;
use uguisu_core::util::{now, random_token};

pub const SESSION_COOKIE: &str = "uguisu_session";
pub const SESSION_SECONDS: i64 = 60 * 60 * 24 * 30;
pub const MIN_PASSWORD_LENGTH: usize = 10;
const PASSWORD_KEY: &str = "admin_password_hash";

pub async fn password_is_set(pool: &SqlitePool) -> Result<bool> {
    Ok(db::get_setting(pool, PASSWORD_KEY).await?.is_some())
}

pub async fn set_password(pool: &SqlitePool, password: String) -> Result<()> {
    let hash = tokio::task::spawn_blocking(move || {
        let salt = SaltString::encode_b64(random_token(16).as_bytes())
            .map_err(|e| anyhow!("cannot create salt: {e}"))?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow!("cannot hash password: {e}"))
    })
    .await??;
    db::set_setting(pool, PASSWORD_KEY, &hash).await
}

pub async fn check_password(pool: &SqlitePool, password: String) -> Result<bool> {
    let Some(stored) = db::get_setting(pool, PASSWORD_KEY).await? else {
        return Ok(false);
    };
    Ok(tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored)
            .map(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await?)
}

fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

/// Ends every session except the given one, and returns how many were ended.
pub async fn delete_other_sessions(pool: &SqlitePool, keep: Option<&str>) -> Result<u64> {
    let keep = keep.map(token_hash).unwrap_or_default();
    Ok(sqlx::query("DELETE FROM sessions WHERE token_hash != ?")
        .bind(keep)
        .execute(pool)
        .await?
        .rows_affected())
}

/// Number of sessions that are still valid.
pub async fn session_count(pool: &SqlitePool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE expires_at > ?")
            .bind(now())
            .fetch_one(pool)
            .await?,
    )
}

/// Removes the password and ends every session. The next visit asks for a new password, like
/// on a fresh installation. For the owner who has lost the password and has access to the
/// machine.
pub async fn reset_password(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(PASSWORD_KEY)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM sessions").execute(pool).await?;
    Ok(())
}

/// Creates a session and returns the secret token for the cookie. Only its hash is stored.
pub async fn create_session(pool: &SqlitePool) -> Result<String> {
    let token = random_token(32);
    let now = now();
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO sessions (token_hash, created_at, expires_at) VALUES (?, ?, ?)")
        .bind(token_hash(&token))
        .bind(now)
        .bind(now + SESSION_SECONDS)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn session_is_valid(pool: &SqlitePool, token: &str) -> Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sessions WHERE token_hash = ? AND expires_at > ?")
            .bind(token_hash(token))
            .bind(now())
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

pub async fn delete_session(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash(token))
        .execute(pool)
        .await?;
    Ok(())
}

pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECONDS}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}
