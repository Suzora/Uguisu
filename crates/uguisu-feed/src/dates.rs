//! Lenient date parsing for feed timestamps.

use time::OffsetDateTime;
use time::format_description::well_known::{Rfc2822, Rfc3339};

/// Parses RFC 2822 (`pubDate`) and RFC 3339 / ISO 8601 (`published`,
/// `updated`, `dc:date`) timestamps, tolerating the common deviations found
/// in real feeds: `GMT`/`UTC`/`Z` zone names, a missing weekday, a missing
/// seconds field, and a date without time.
pub fn parse_date(raw: &str) -> Option<OffsetDateTime> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc3339) {
        return Some(dt);
    }
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc2822) {
        return Some(dt);
    }
    // RFC 2822 variants: normalize zone names and drop the weekday.
    let mut candidate = s.to_owned();
    for (from, to) in [
        (" GMT", " +0000"),
        (" UTC", " +0000"),
        (" UT", " +0000"),
        (" Z", " +0000"),
    ] {
        if candidate.ends_with(from) {
            candidate = format!("{}{to}", &candidate[..candidate.len() - from.len()]);
        }
    }
    if let Some((_, rest)) = candidate.split_once(", ")
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        candidate = rest.to_owned();
    }
    // Missing seconds: "01 Sep 2025 08:00 +0000"
    let parts: Vec<&str> = candidate.split_whitespace().collect();
    if parts.len() == 5 && parts[3].matches(':').count() == 1 {
        candidate = format!(
            "{} {} {} {}:00 {}",
            parts[0], parts[1], parts[2], parts[3], parts[4]
        );
    }
    if let Ok(dt) = OffsetDateTime::parse(&candidate, &Rfc2822) {
        return Some(dt);
    }
    // Date-only ISO: 2025-09-01
    if s.len() == 10
        && s.as_bytes()[4] == b'-'
        && let Ok(dt) = OffsetDateTime::parse(&format!("{s}T00:00:00Z"), &Rfc3339)
    {
        return Some(dt);
    }
    // ISO without zone: 2025-09-01T10:00:00
    if s.len() == 19
        && s.as_bytes()[10] == b'T'
        && let Ok(dt) = OffsetDateTime::parse(&format!("{s}Z"), &Rfc3339)
    {
        return Some(dt);
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn ts(s: &str) -> i64 {
        parse_date(s).map_or(-1, OffsetDateTime::unix_timestamp)
    }

    #[test]
    fn parses_standard_and_lenient_forms() {
        let expected = 1_756_713_600; // 2025-09-01T08:00:00Z
        assert_eq!(ts("Mon, 01 Sep 2025 08:00:00 +0000"), expected);
        assert_eq!(ts("Mon, 01 Sep 2025 08:00:00 GMT"), expected);
        assert_eq!(ts("01 Sep 2025 08:00:00 UTC"), expected);
        assert_eq!(ts("Mon, 01 Sep 2025 08:00 GMT"), expected);
        assert_eq!(ts("2025-09-01T08:00:00Z"), expected);
        assert_eq!(ts("2025-09-01T10:00:00+02:00"), expected);
        assert_eq!(ts("2025-09-01T08:00:00"), expected);
        assert_eq!(ts("2025-09-01"), expected - 8 * 3600);
        assert_eq!(ts("yesterday"), -1);
        assert_eq!(ts(""), -1);
    }
}
