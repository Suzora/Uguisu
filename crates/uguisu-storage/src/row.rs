//! Row decoding helpers shared by the repositories.

use serde::de::DeserializeOwned;
use time::OffsetDateTime;
use uguisu_core::ids::Id;
use url::Url;

use crate::{StorageError, parse_db_ts};

pub(crate) fn corrupt(table: &'static str, id: &str, detail: impl Into<String>) -> StorageError {
    StorageError::Corrupt {
        table,
        id: id.to_owned(),
        detail: detail.into(),
    }
}

pub(crate) fn id<T>(table: &'static str, row_id: &str, value: &str) -> Result<Id<T>, StorageError> {
    Id::parse(value).map_err(|e| corrupt(table, row_id, e.to_string()))
}

pub(crate) fn opt_id<T>(
    table: &'static str,
    row_id: &str,
    value: Option<&str>,
) -> Result<Option<Id<T>>, StorageError> {
    value.map(|v| id(table, row_id, v)).transpose()
}

pub(crate) fn ts(
    table: &'static str,
    row_id: &str,
    value: &str,
) -> Result<OffsetDateTime, StorageError> {
    parse_db_ts(value).ok_or_else(|| corrupt(table, row_id, format!("bad timestamp `{value}`")))
}

pub(crate) fn opt_ts(
    table: &'static str,
    row_id: &str,
    value: Option<&str>,
) -> Result<Option<OffsetDateTime>, StorageError> {
    value.map(|v| ts(table, row_id, v)).transpose()
}

/// URLs are stored as written; a value that no longer parses is dropped
/// with a warning instead of failing the whole row.
pub(crate) fn opt_url(value: Option<&str>) -> Option<Url> {
    value.and_then(|v| Url::parse(v).ok())
}

pub(crate) fn req_url(table: &'static str, row_id: &str, value: &str) -> Result<Url, StorageError> {
    Url::parse(value).map_err(|e| corrupt(table, row_id, format!("bad url `{value}`: {e}")))
}

pub(crate) fn json<T: DeserializeOwned>(
    table: &'static str,
    column: &'static str,
    row_id: &str,
    value: &str,
) -> Result<T, StorageError> {
    serde_json::from_str(value).map_err(|source| StorageError::Json {
        table,
        column,
        id: row_id.to_owned(),
        source,
    })
}

pub(crate) fn opt_json<T: DeserializeOwned>(
    table: &'static str,
    column: &'static str,
    row_id: &str,
    value: Option<&str>,
) -> Result<Option<T>, StorageError> {
    value.map(|v| json(table, column, row_id, v)).transpose()
}

pub(crate) fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

pub(crate) fn bool_from(v: Option<i64>) -> Option<bool> {
    v.map(|n| n != 0)
}

pub(crate) fn u32_from(v: Option<i64>) -> Option<u32> {
    v.and_then(|n| u32::try_from(n).ok())
}

pub(crate) fn u64_from(v: Option<i64>) -> Option<u64> {
    v.and_then(|n| u64::try_from(n).ok())
}

pub(crate) fn i64_from_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

pub(crate) fn parse_enum<T>(
    table: &'static str,
    row_id: &str,
    column: &str,
    value: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<T, StorageError> {
    parse(value).ok_or_else(|| corrupt(table, row_id, format!("unknown {column} `{value}`")))
}
