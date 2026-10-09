//! Timestamp encoding: UTC, second precision, RFC 3339 with a trailing `Z`.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Encodes a timestamp for a TEXT column so that lexical order equals
/// chronological order regardless of the source offset or sub-seconds.
#[must_use]
pub fn to_db_ts(t: OffsetDateTime) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .replace_nanosecond(0)
        .unwrap_or(t)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Decodes a timestamp written by [`to_db_ts`].
#[must_use]
pub fn parse_db_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use time::macros::datetime;

    #[test]
    fn equal_instants_encode_identically_and_order_lexically() {
        let a = datetime!(2026-09-17 10:00:00.750 +02:00);
        let b = datetime!(2026-09-17 08:00:00 UTC);
        assert_eq!(to_db_ts(a), "2026-09-17T08:00:00Z");
        assert_eq!(to_db_ts(a), to_db_ts(b));
        let later = datetime!(2026-09-17 09:00:00 -05:00); // 14:00Z
        assert!(to_db_ts(later) > to_db_ts(b));
        assert_eq!(
            parse_db_ts(&to_db_ts(later)).unwrap(),
            later.replace_nanosecond(0).unwrap()
        );
        assert!(parse_db_ts("garbage").is_none());
    }
}
