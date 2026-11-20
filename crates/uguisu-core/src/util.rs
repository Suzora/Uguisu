use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as unix seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn timestamp(seconds: i64) -> Option<jiff::Timestamp> {
    jiff::Timestamp::from_second(seconds).ok()
}

/// Formats unix seconds in UTC with a `strftime` pattern.
pub fn format_utc(seconds: i64, pattern: &str) -> Option<String> {
    Some(timestamp(seconds)?.strftime(pattern).to_string())
}

/// Unix seconds as an RFC 3339 timestamp, e.g. `2024-01-02T10:00:00Z`.
pub fn rfc3339(seconds: i64) -> Option<String> {
    Some(timestamp(seconds)?.to_string())
}

/// Unix seconds as an RFC 2822 date, the format RSS uses.
pub fn rfc2822(seconds: i64) -> Option<String> {
    format_utc(seconds, "%a, %d %b %Y %H:%M:%S +0000")
}

pub fn parse_rfc2822(input: &str) -> Option<i64> {
    jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(input)
        .ok()
        .map(|timestamp| timestamp.as_second())
}

pub fn parse_rfc3339(input: &str) -> Option<i64> {
    input
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|timestamp| timestamp.as_second())
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0xf) as usize] as char);
    }
    out
}

/// Cryptographically random token, hex encoded (`bytes * 2` characters).
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("operating system random source unavailable");
    hex(&buf)
}

/// First 8 hex characters of the BLAKE3 hash of `input`. Keeps directory names unique.
pub fn short_hash(input: &str) -> String {
    blake3::hash(input.as_bytes()).to_hex()[..8].to_string()
}

/// File-system friendly name: lowercase letters and digits (any script) joined by single dashes.
pub fn slugify(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut pending_dash = false;
    for c in input.chars().flat_map(char::to_lowercase) {
        if count >= max_chars {
            break;
        }
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
                count += 1;
            }
            pending_dash = false;
            out.push(c);
            count += 1;
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// HTML as plain text for searching: tags become spaces, the common character references are
/// decoded. Not meant for display.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters other than tab/newline/CR are not allowed in XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Hello,  World! #42", 60), "hello-world-42");
        assert_eq!(slugify("Größe & Maß", 60), "größe-maß");
        assert_eq!(slugify("鶯の声", 60), "鶯の声");
        assert_eq!(slugify("!!!", 60), "untitled");
        assert_eq!(slugify("abcdef", 3), "abc");
    }

    #[test]
    fn time_formats() {
        let t = 1_704_189_600; // 2024-01-02 10:00:00 UTC
        assert_eq!(rfc3339(t).as_deref(), Some("2024-01-02T10:00:00Z"));
        assert_eq!(
            rfc2822(t).as_deref(),
            Some("Tue, 02 Jan 2024 10:00:00 +0000")
        );
        assert_eq!(
            format_utc(t, "%Y%m%dT%H%M%SZ").as_deref(),
            Some("20240102T100000Z")
        );
        assert_eq!(
            format_utc(t, "%-d %b %Y, %H:%M").as_deref(),
            Some("2 Jan 2024, 10:00")
        );
        assert_eq!(parse_rfc2822(&rfc2822(t).unwrap()), Some(t));
        assert_eq!(parse_rfc3339("2024-01-02T11:00:00+01:00"), Some(t));
    }

    #[test]
    fn escaping() {
        assert_eq!(xml_escape("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
    }

    #[test]
    fn tokens_are_random_hex() {
        let a = random_token(16);
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_token(16));
    }
}
