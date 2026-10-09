//! Cross-crate error type.
//!
//! Each crate keeps its own detailed error enums; `UguisuError` is what the
//! engine returns to frontends, carrying enough structure for HTTP status
//! and exit-code mapping without leaking implementation types.

use serde::{Deserialize, Serialize};

use crate::archive::ArchiveErrorKind;
use crate::feed::FetchErrorKind;

/// Error returned by engine services.
#[derive(
    Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum UguisuError {
    /// Database failure.
    #[error("storage: {0}")]
    Storage(String),
    /// Network failure outside a refresh (e.g. while adding a podcast).
    #[error("network ({kind}): {detail}")]
    Network {
        /// Classification.
        kind: FetchErrorKind,
        /// Detail.
        detail: String,
    },
    /// The feed could not be used.
    #[error("feed ({kind}): {detail}")]
    Feed {
        /// Classification.
        kind: FetchErrorKind,
        /// Detail.
        detail: String,
    },
    /// The input could not be resolved to a feed.
    #[error("cannot resolve `{input}`: {detail}")]
    Unresolvable {
        /// The input.
        input: String,
        /// Detail.
        detail: String,
    },
    /// Refused by the network policy.
    #[error("blocked by network policy: {0}")]
    BlockedByPolicy(String),
    /// An entity does not exist.
    #[error("{entity} `{id}` not found")]
    NotFound {
        /// Entity name.
        entity: String,
        /// Identifier.
        id: String,
    },
    /// The operation conflicts with existing state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Invalid input.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// Configuration problem.
    #[error("configuration: {0}")]
    Config(String),
    /// Another process holds the data directory.
    #[error("data directory is locked by another Uguisu process: {0}")]
    Locked(String),
    /// Cancelled or timed out.
    #[error("cancelled: {0}")]
    Cancelled(String),
    /// A file system operation failed.
    #[error("i/o at {path}: {detail}")]
    Io {
        /// Path involved.
        path: String,
        /// Detail.
        detail: String,
    },
    /// The media file system has no room for the download.
    #[error("disk full at {path}: {needed} bytes needed, {available} available")]
    DiskFull {
        /// Directory checked.
        path: String,
        /// Bytes the operation needs (including the reserve).
        needed: u64,
        /// Bytes available.
        available: u64,
    },
    /// An archive artifact or a path could not be used.
    #[error("archive ({kind}): {detail}")]
    Archive {
        /// Classification.
        kind: ArchiveErrorKind,
        /// Detail.
        detail: String,
    },
    /// Unexpected failure.
    #[error("internal: {0}")]
    Internal(String),
}

impl UguisuError {
    /// A stable, machine-readable kind.
    ///
    /// An archive failure reports the [`ArchiveErrorKind`] itself rather than
    /// a flat `"archive"`: there are 21 of them across five HTTP statuses, and
    /// a caller that has to read the prose message to tell a hash mismatch
    /// from a missing file has no machine-readable kind at all.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Storage(_) => "storage",
            Self::Network { .. } => "network",
            Self::Feed { .. } => "feed",
            Self::Unresolvable { .. } => "unresolvable",
            Self::BlockedByPolicy(_) => "blocked_by_policy",
            Self::NotFound { .. } => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Invalid(_) => "invalid",
            Self::Config(_) => "config",
            Self::Locked(_) => "locked",
            Self::Cancelled(_) => "cancelled",
            Self::Io { .. } => "io",
            Self::DiskFull { .. } => "disk_full",
            Self::Archive { kind, .. } => kind.as_str(),
            Self::Internal(_) => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_errors_carry_their_kind() {
        let e = UguisuError::Archive {
            kind: ArchiveErrorKind::HashMismatch,
            detail: "sha256 differs".into(),
        };
        assert_eq!(e.kind(), "hash_mismatch");
        assert_eq!(e.to_string(), "archive (hash_mismatch): sha256 differs");
        for kind in ArchiveErrorKind::ALL {
            let e = UguisuError::Archive {
                kind,
                detail: String::new(),
            };
            assert_eq!(e.kind(), kind.as_str());
        }
    }

    #[test]
    fn kinds_and_messages() {
        let e = UguisuError::NotFound {
            entity: "podcast".to_owned(),
            id: "x".into(),
        };
        assert_eq!(e.kind(), "not_found");
        assert_eq!(e.to_string(), "podcast `x` not found");
        assert_eq!(
            UguisuError::Feed {
                kind: FetchErrorKind::MalformedXml,
                detail: "d".into()
            }
            .to_string(),
            "feed (malformed_xml): d"
        );
    }
}
