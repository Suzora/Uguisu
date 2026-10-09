//! The per-launch credential (ADR 0042).
//!
//! An ordinary `write` token (ADR 0035), minted when the shell starts and
//! revoked when it stops. It exists so the WebView can open a normal session
//! without a password, and it is deliberately awkward to get at: it lives in
//! this process's memory, it is handed to the page exactly once, and it is
//! never an argument, an environment variable, a URL or a log field.

use std::sync::Mutex;

use time::{Duration, OffsetDateTime};
use uguisu_core::auth::Scope;
use uguisu_core::error::UguisuError;
use uguisu_core::ids::ApiTokenId;
use uguisu_core::secret::Secret;
use uguisu_engine::Engine;

/// What the token is called in `uguisu auth token list`.
const NAME: &str = "desktop launch";

/// How long the credential stays usable.
///
/// The bootstrap happens within a second of starting; everything after it is
/// the session cookie's business. An hour is slack for a slow first run, not
/// a lifetime.
const LIFETIME: Duration = Duration::hours(1);

/// The launch credential, which can be taken exactly once.
#[derive(Debug)]
pub struct Launch {
    id: ApiTokenId,
    secret: Mutex<Option<Secret<String>>>,
}

impl Launch {
    /// Mints the token this launch will bootstrap with.
    pub async fn mint(engine: &Engine) -> Result<Self, UguisuError> {
        let issued = engine
            .issue_token(
                NAME,
                Scope::Write,
                Some(OffsetDateTime::now_utc() + LIFETIME),
            )
            .await?;
        tracing::info!(token_id = %issued.token.id, "launch credential minted");
        Ok(Self {
            id: issued.token.id,
            secret: Mutex::new(Some(issued.secret)),
        })
    }

    /// Hands the secret to the WebView, once.
    ///
    /// Single-use at this boundary: the page needs it for one exchange, and
    /// the session cookie carries every request after that. A second caller
    /// gets nothing, so a page that is somehow asked twice cannot hoard it.
    pub fn take(&self) -> Option<Secret<String>> {
        self.secret.lock().ok()?.take()
    }

    /// Revokes the token, whether or not it was ever used.
    ///
    /// Drops the plaintext first, so that after this there is no way to obtain
    /// a credential at all rather than a way to obtain a dead one.
    pub async fn revoke(&self, engine: &Engine) {
        drop(self.take());
        match engine.revoke_token(self.id).await {
            Ok(true) => tracing::info!(token_id = %self.id, "launch credential revoked"),
            Ok(false) => tracing::debug!(token_id = %self.id, "launch credential already gone"),
            Err(error) => tracing::warn!(%error, "the launch credential could not be revoked"),
        }
    }
}
