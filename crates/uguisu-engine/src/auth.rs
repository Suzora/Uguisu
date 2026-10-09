//! The credential, the sessions it opens and the tokens it issues (ADR 0035).
//!
//! This is the only module where a plaintext secret exists. Everything it
//! hands to storage is a digest, and everything it reads back is compared
//! against a digest, so nothing here needs a constant-time comparison: the
//! database matches SHA-256 of a 256-bit random value, and an attacker who
//! could exploit that timing would already have the preimage. The
//! comparisons that *are* timing-sensitive — a password and a CSRF token —
//! go through argon2's verifier and [`uguisu_core::secret::Secret::ct_eq`].

use std::sync::LazyLock;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uguisu_core::UguisuError;
use uguisu_core::auth::{ApiToken, Scope, Session};
use uguisu_core::ids::{ApiTokenId, SessionId};
use uguisu_core::secret::Secret;
use uguisu_storage::auth as repo;

use crate::Engine;

/// How long a session survives without being used.
pub const SESSION_IDLE: Duration = Duration::days(7);

/// How long a session survives at all, however active.
pub const SESSION_ABSOLUTE: Duration = Duration::days(30);

/// How stale a session's activity may get before a request writes it.
///
/// The writer pool has one connection, so refreshing on every request would
/// serialize the whole API behind `auth_sessions`. An hour of drift costs an
/// idle deadline that can be up to an hour early, which nobody notices at a
/// seven-day idle window.
const TOUCH_AFTER: Duration = Duration::hours(1);

/// Bytes of randomness in a session, CSRF or token secret.
const SECRET_BYTES: usize = 32;

/// argon2id cost: 64 MiB, two passes, one lane.
///
/// Measured at ~110 ms on the reference machine, which is the ≈100 ms
/// `docs/SECURITY.md` §3.5 asks for; argon2's own default (19 MiB, two
/// passes) came out at 19 ms, fast enough to be worth six times more guesses
/// per second. The numbers are recorded in every PHC string, so raising them
/// later leaves existing hashes verifiable under the values they were made
/// with.
const M_COST: u32 = 65_536;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

/// Bounds how much memory password hashing can be holding at once.
///
/// Each verification allocates `M_COST` KiB — 64 MiB — for as long as it
/// runs, so an unauthenticated flood of logins is a memory amplifier long
/// before it is a CPU one: the login limiter caps attempts per peer over five
/// minutes but says nothing about how many arrive at the same instant. Two
/// permits means peak hashing memory is 128 MiB whatever the load, and the
/// rest of the flood waits. Two rather than one because a single operator
/// retrying should not queue behind a stranger's attempt.
static HASHING: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(2));

/// The hasher, with the parameters above.
fn hasher() -> Argon2<'static> {
    // `Params::new` only rejects values outside argon2's own ranges, and
    // these are inside them; the default is the same algorithm with cheaper
    // numbers, so a fallback cannot make anything unsafe.
    let params = Params::new(M_COST, T_COST, P_COST, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// A session and the cookie value that opens it.
///
/// The secret is in the response that creates it and nowhere else: storage
/// holds only its digest.
#[derive(Debug)]
pub struct NewSession {
    /// The stored record.
    pub session: Session,
    /// The value the `uguisu_session` cookie must carry.
    pub cookie: Secret<String>,
}

/// A token and the secret it was issued with, which is shown once.
#[derive(Debug)]
pub struct NewToken {
    /// The stored record.
    pub token: ApiToken,
    /// The value a client sends as `Authorization: Bearer …`.
    pub secret: Secret<String>,
}

/// A fresh hex secret from the OS generator.
fn secret() -> Result<Secret<String>, UguisuError> {
    let mut bytes = [0_u8; SECRET_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| UguisuError::Internal(format!("no randomness available: {e}")))?;
    Ok(Secret::new(hex::encode(bytes)))
}

/// The lookup key for a presented secret.
fn digest(presented: &str) -> String {
    hex::encode(Sha256::digest(presented.as_bytes()))
}

/// Hashes a password with argon2id.
///
/// The PHC string records the parameters, so raising them later leaves every
/// existing hash verifiable with the values it was made under.
fn hash_password(password: &Secret<String>) -> Result<String, UguisuError> {
    let mut salt = [0_u8; 16];
    getrandom::fill(&mut salt)
        .map_err(|e| UguisuError::Internal(format!("no randomness available: {e}")))?;
    let salt =
        SaltString::encode_b64(&salt).map_err(|e| UguisuError::Internal(format!("salt: {e}")))?;
    hasher()
        .hash_password(password.expose().as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| UguisuError::Internal(format!("password hashing failed: {e}")))
}

/// Runs `work` off the async runtime, holding a hashing permit.
///
/// argon2 at these parameters blocks for ~110 ms. On a runtime worker that is
/// 110 ms during which nothing else on that worker progresses, so it goes to
/// the blocking pool; the permit is what bounds the memory (see [`HASHING`]).
async fn off_runtime<T, F>(work: F) -> Result<T, UguisuError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let permit = HASHING
        .acquire()
        .await
        .map_err(|e| UguisuError::Internal(format!("hashing is shut down: {e}")))?;
    let out = tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| UguisuError::Internal(format!("hashing task failed: {e}")))?;
    drop(permit);
    Ok(out)
}

/// Whether `password` is the one `stored` was made from.
///
/// A stored hash this build cannot parse answers `false` rather than failing
/// the request: it is a reason to refuse a login, not to take the API down.
fn password_matches(stored: &str, password: &Secret<String>) -> bool {
    match PasswordHash::new(stored) {
        Ok(hash) => hasher()
            .verify_password(password.expose().as_bytes(), &hash)
            .is_ok(),
        Err(e) => {
            tracing::error!(error = %e, "the stored password hash does not parse");
            false
        }
    }
}

impl Engine {
    /// Whether a credential has been set, which is what decides whether
    /// authentication is on at all.
    pub async fn credential_set(&self) -> Result<bool, UguisuError> {
        let mut r = self.storage().reader().await?;
        Ok(repo::credential_set(&mut r).await?)
    }

    /// The operator's name, when a credential exists.
    pub async fn credential_username(&self) -> Result<Option<String>, UguisuError> {
        let mut r = self.storage().reader().await?;
        Ok(repo::credential(&mut r).await?.map(|c| c.username))
    }

    /// Sets the password, revoking every session but `keep`.
    ///
    /// Revoking is the point: a password is changed because the old one may
    /// be known, and a session opened with it would outlive the change.
    /// `keep` is the browser doing the changing, which should not be logged
    /// out of the page it is on.
    pub async fn set_password(
        &self,
        username: &str,
        password: &Secret<String>,
        keep: Option<SessionId>,
    ) -> Result<u64, UguisuError> {
        let username = username.trim();
        if username.is_empty() || username.chars().count() > 64 {
            return Err(UguisuError::Invalid(
                "a username is 1 to 64 characters".to_owned(),
            ));
        }
        if password.expose().chars().count() < 8 {
            return Err(UguisuError::Invalid(
                "a password is at least 8 characters".to_owned(),
            ));
        }
        let owned = password.clone();
        let hash = off_runtime(move || hash_password(&owned)).await??;
        let now = OffsetDateTime::now_utc();
        let mut tx = self.storage().begin().await?;
        repo::set_credential(&mut tx, username, &hash, now).await?;
        let revoked = repo::revoke_sessions_except(&mut tx, keep, now).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        tracing::info!(revoked, "credential set");
        Ok(revoked)
    }

    /// Verifies a password and opens a session, or answers `None`.
    ///
    /// `None` covers an unknown username, a wrong password and no credential
    /// at all, because the caller must not be able to tell those apart. An
    /// unknown username still costs one verification, so the timing cannot
    /// tell them apart either.
    pub async fn login(
        &self,
        username: &str,
        password: &Secret<String>,
    ) -> Result<Option<NewSession>, UguisuError> {
        let mut r = self.storage().reader().await?;
        let credential = repo::credential(&mut r).await?;
        drop(r);
        // Verify against the real hash when the name matches and against a
        // hash of nothing when it does not, so an unknown username costs the
        // same as a wrong password: a login that returns early is a name
        // oracle with a stopwatch.
        let stored = match &credential {
            Some(c) if c.username == username.trim() => c.password_hash.clone(),
            _ => DUMMY_HASH.to_owned(),
        };
        let real = credential
            .as_ref()
            .is_some_and(|c| c.username == username.trim());
        let owned = password.clone();
        let matched = off_runtime(move || password_matches(&stored, &owned)).await? && real;
        if !matched {
            return Ok(None);
        }
        self.open_session().await.map(Some)
    }

    /// Opens a session without checking a password.
    ///
    /// The caller has already established who is asking by some other means —
    /// today only the desktop shell's per-launch token (ADR 0042) — so this is
    /// deliberately not reachable over HTTP except through a handler that has
    /// done that work. `login` is this function plus the password check.
    pub async fn open_session(&self) -> Result<NewSession, UguisuError> {
        let cookie = secret()?;
        let csrf = secret()?;
        let now = OffsetDateTime::now_utc();
        let session = Session {
            id: SessionId::new(),
            token_digest: digest(cookie.expose()),
            csrf_token: csrf.into_inner(),
            created_at: now,
            last_seen_at: now,
            idle_expires_at: now + SESSION_IDLE,
            absolute_expires_at: now + SESSION_ABSOLUTE,
            revoked_at: None,
        };
        let mut w = self.storage().writer().await?;
        repo::open_session(&mut w, &session).await?;
        tracing::info!(session_id = %session.id, "session opened");
        Ok(NewSession { session, cookie })
    }

    /// The session a cookie opens, or `None` when it opens none.
    ///
    /// Moves the idle deadline forward when it is stale enough to be worth a
    /// write.
    pub async fn session_for(&self, cookie: &str) -> Result<Option<Session>, UguisuError> {
        let mut r = self.storage().reader().await?;
        let found = repo::session_by_digest(&mut r, &digest(cookie)).await?;
        drop(r);
        let now = OffsetDateTime::now_utc();
        let Some(session) = found.filter(|s| s.usable(now)) else {
            return Ok(None);
        };
        if now - session.last_seen_at > TOUCH_AFTER {
            let mut w = self.storage().writer().await?;
            repo::touch_session(&mut w, session.id, now, now + SESSION_IDLE).await?;
        }
        Ok(Some(session))
    }

    /// Ends one session. `false` when it was already closed.
    pub async fn logout(&self, id: SessionId) -> Result<bool, UguisuError> {
        let mut w = self.storage().writer().await?;
        let done = repo::revoke_session(&mut w, id, OffsetDateTime::now_utc()).await?;
        if done {
            tracing::info!(session_id = %id, "session closed");
        }
        Ok(done)
    }

    /// The token a bearer secret belongs to, or `None`.
    pub async fn token_for(&self, presented: &str) -> Result<Option<ApiToken>, UguisuError> {
        let mut r = self.storage().reader().await?;
        let found = repo::token_by_digest(&mut r, &digest(presented)).await?;
        drop(r);
        let now = OffsetDateTime::now_utc();
        let Some(token) = found.filter(|t| t.usable(now)) else {
            return Ok(None);
        };
        if token.last_used_at.is_none_or(|at| now - at > TOUCH_AFTER) {
            let mut w = self.storage().writer().await?;
            repo::touch_token(&mut w, token.id, now).await?;
        }
        Ok(Some(token))
    }

    /// Issues a token. The secret in the answer is the only copy.
    pub async fn issue_token(
        &self,
        name: &str,
        scope: Scope,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<NewToken, UguisuError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(UguisuError::Invalid(
                "a token name is 1 to 64 characters".to_owned(),
            ));
        }
        let now = OffsetDateTime::now_utc();
        if expires_at.is_some_and(|at| at <= now) {
            return Err(UguisuError::Invalid(
                "a token that has already expired is not worth issuing".to_owned(),
            ));
        }
        let secret = secret()?;
        let token = ApiToken {
            id: ApiTokenId::new(),
            name: name.to_owned(),
            scope,
            created_at: now,
            last_used_at: None,
            expires_at,
            revoked_at: None,
        };
        let mut w = self.storage().writer().await?;
        repo::issue_token(&mut w, &token, &digest(secret.expose())).await?;
        tracing::info!(token_id = %token.id, %scope, "token issued");
        Ok(NewToken { token, secret })
    }

    /// Every token, newest first, revoked ones included.
    pub async fn list_tokens(&self) -> Result<Vec<ApiToken>, UguisuError> {
        let mut r = self.storage().reader().await?;
        Ok(repo::list_tokens(&mut r).await?)
    }

    /// Revokes a token. `false` when it was already revoked or unknown.
    pub async fn revoke_token(&self, id: ApiTokenId) -> Result<bool, UguisuError> {
        let mut w = self.storage().writer().await?;
        let done = repo::revoke_token(&mut w, id, OffsetDateTime::now_utc()).await?;
        if done {
            tracing::info!(token_id = %id, "token revoked");
        }
        Ok(done)
    }
}

/// An argon2id hash of a value nobody knows, verified against when the
/// username does not match so that a login costs the same either way.
///
/// Verification runs with the parameters written in the hash, not with
/// `hasher()`'s, so this must carry exactly `M_COST`, `T_COST` and `P_COST`: a
/// cheaper dummy makes an unknown username measurably faster.
const DUMMY_HASH: &str = "$argon2id$v=19$m=65536,t=2,p=1$GG/nHlIOuHkXMb3X2/eIYA$\
                          UwiCIMnkjn7QUMWrqJ2BztD2gJYRy3EE5bNFIIZ159Y";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// If this string did not parse, `password_matches` would log and return
    /// without hashing — and the equal cost of an unknown username, which is
    /// the whole reason the constant exists, would silently be gone.
    #[test]
    fn the_dummy_hash_parses() {
        assert!(PasswordHash::new(DUMMY_HASH).is_ok(), "{DUMMY_HASH}");
        assert!(!password_matches(
            DUMMY_HASH,
            &Secret::new("anything".to_owned())
        ));
    }

    #[test]
    fn the_dummy_costs_a_real_hash() {
        let dummy = Params::try_from(&PasswordHash::new(DUMMY_HASH).unwrap()).unwrap();
        assert_eq!(
            (dummy.m_cost(), dummy.t_cost(), dummy.p_cost()),
            (M_COST, T_COST, P_COST)
        );
    }

    #[test]
    fn a_hash_verifies_only_its_own_password() {
        let hash = hash_password(&Secret::new("correct horse".to_owned())).unwrap();
        assert!(hash.starts_with("$argon2id$v=19$"), "{hash}");
        assert!(password_matches(
            &hash,
            &Secret::new("correct horse".to_owned())
        ));
        assert!(!password_matches(
            &hash,
            &Secret::new("correct hors".to_owned())
        ));
        assert!(!password_matches(
            "not a phc string",
            &Secret::new("x".to_owned())
        ));
    }

    #[test]
    fn two_hashes_of_one_password_differ() {
        let a = hash_password(&Secret::new("correct horse".to_owned())).unwrap();
        let b = hash_password(&Secret::new("correct horse".to_owned())).unwrap();
        assert_ne!(a, b, "each hash carries its own salt");
    }

    #[test]
    fn a_secret_is_a_fresh_full_width_hex_string() {
        let a = secret().unwrap();
        let b = secret().unwrap();
        assert_eq!(a.expose().len(), SECRET_BYTES * 2);
        assert_ne!(a.expose(), b.expose());
        assert!(a.expose().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(digest(a.expose()).len(), 64);
        assert_ne!(digest(a.expose()), *a.expose());
    }
}
