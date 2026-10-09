//! Who is asking, what they may do, and the records behind both (ADR 0035).
//!
//! Uguisu has one operator. There is no role here and no user table: the
//! difference between two callers is whether a request may change anything,
//! which is what [`Scope`] says and nothing else needs to.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{ApiTokenId, SessionId};

/// What a credential is allowed to do.
///
/// Two values, because the API has exactly two authenticated levels. A third
/// would have to mean something no route asks about.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// May read. A mutation is refused.
    Read,
    /// May read and change anything the API exposes.
    #[default]
    Write,
}

impl Scope {
    /// Every variant.
    pub const ALL: [Self; 2] = [Self::Read, Self::Write];

    /// Stable string form, as stored and as sent.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    /// Parses the stable string form.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s2| s2.as_str() == s)
    }

    /// Whether this scope may change something.
    #[must_use]
    pub const fn may_mutate(self) -> bool {
        matches!(self, Self::Write)
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The single credential (`auth_credential`).
///
/// The hash is an argon2id PHC string, so its parameters travel with it and
/// can change without a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The operator's name.
    pub username: String,
    /// argon2id PHC string.
    pub password_hash: String,
    /// When it was first set.
    pub created_at: OffsetDateTime,
    /// When it last changed.
    pub updated_at: OffsetDateTime,
}

/// A browser session (`auth_sessions`), without the secret it was opened with.
///
/// The cookie carries a secret this record only holds the digest of, so
/// nothing that can be read here can be replayed as a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Session {
    /// Identifier; safe to log.
    pub id: SessionId,
    /// SHA-256 of the cookie's secret, hex.
    pub token_digest: String,
    /// The token a mutating request must echo back.
    pub csrf_token: String,
    /// When the session was opened.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When it was last used. Refreshed at most hourly, because the writer
    /// pool has one connection and a write per request would serialize the
    /// whole API behind this table.
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    /// When inactivity ends it.
    #[serde(with = "time::serde::rfc3339")]
    pub idle_expires_at: OffsetDateTime,
    /// When it ends regardless of activity.
    #[serde(with = "time::serde::rfc3339")]
    pub absolute_expires_at: OffsetDateTime,
    /// When it was revoked, if it was.
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

impl Session {
    /// Whether the session may still authenticate a request at `now`.
    #[must_use]
    pub fn usable(&self, now: OffsetDateTime) -> bool {
        self.revoked_at.is_none() && self.idle_expires_at > now && self.absolute_expires_at > now
    }
}

/// An API token (`auth_tokens`), without the secret it was issued with.
///
/// The secret exists once, in the response that creates it. Everything here
/// can be shown to the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApiToken {
    /// Identifier.
    pub id: ApiTokenId,
    /// The operator's label for it.
    pub name: String,
    /// What it may do.
    pub scope: Scope,
    /// When it was issued.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When it was last accepted. Refreshed at most hourly, as for a session.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    /// When it stops working, if it was given a deadline.
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// When it was revoked, if it was.
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

impl ApiToken {
    /// Whether the token may still authenticate a request at `now`.
    #[must_use]
    pub fn usable(&self, now: OffsetDateTime) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_none_or(|at| at > now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_round_trip() {
        for scope in Scope::ALL {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(Scope::parse("admin"), None);
        assert!(!Scope::Read.may_mutate());
        assert!(Scope::Write.may_mutate());
    }

    fn at(secs: i64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(secs)
    }

    #[test]
    fn a_session_stops_being_usable() {
        let mut s = Session {
            id: SessionId::new(),
            token_digest: "0".repeat(64),
            csrf_token: "1".repeat(64),
            created_at: at(0),
            last_seen_at: at(0),
            idle_expires_at: at(100),
            absolute_expires_at: at(1000),
            revoked_at: None,
        };
        assert!(s.usable(at(50)));
        assert!(!s.usable(at(150)), "idle expiry ends it");
        s.idle_expires_at = at(2000);
        assert!(!s.usable(at(1500)), "the absolute deadline still ends it");
        s.absolute_expires_at = at(3000);
        assert!(s.usable(at(1500)));
        s.revoked_at = Some(at(1400));
        assert!(!s.usable(at(1500)));
    }

    #[test]
    fn a_token_without_a_deadline_stays_usable() {
        let mut t = ApiToken {
            id: ApiTokenId::new(),
            name: "laptop".to_owned(),
            scope: Scope::Read,
            created_at: at(0),
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
        };
        assert!(t.usable(at(1_000_000)));
        t.expires_at = Some(at(100));
        assert!(!t.usable(at(150)));
        t.expires_at = None;
        t.revoked_at = Some(at(10));
        assert!(!t.usable(at(150)));
    }
}
