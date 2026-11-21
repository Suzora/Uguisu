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

/// The spoken words of a transcript, for searching. Transcripts come as WebVTT, SubRip, the
/// JSON of the podcast namespace, HTML or plain text; cue numbers, time codes and markup are
/// left out. `extension` is that of the archived file.
pub fn transcript_text(bytes: &[u8], extension: &str) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let mut out = match extension {
        "json" => {
            let mut words = String::new();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
                collect_bodies(&value, &mut words);
            }
            words
        }
        "html" | "htm" => html_to_text(text),
        _ => {
            let mut words = String::new();
            for line in text.lines() {
                let line = line.trim();
                let is_cue_number = !line.is_empty() && line.chars().all(|c| c.is_ascii_digit());
                if line.contains("-->")
                    || is_cue_number
                    || line.starts_with("WEBVTT")
                    || line.starts_with("NOTE")
                {
                    continue;
                }
                // Voice and formatting tags of WebVTT (`<v Name>`, `<i>`).
                words.push_str(&html_to_text(line));
                words.push(' ');
            }
            words
        }
    };
    const MAX_CHARS: usize = 2_000_000;
    if out.len() > MAX_CHARS {
        let mut end = MAX_CHARS;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// Gathers every `body` (and `text`) string of a JSON transcript, in document order.
fn collect_bodies(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Array(items) => items.iter().for_each(|item| collect_bodies(item, out)),
        serde_json::Value::Object(fields) => {
            for (name, field) in fields {
                match field {
                    serde_json::Value::String(text) if name == "body" || name == "text" => {
                        out.push_str(text);
                        out.push(' ');
                    }
                    serde_json::Value::String(_) => {}
                    other => collect_bodies(other, out),
                }
            }
        }
        _ => {}
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
    #[test]
    fn transcripts_as_text() {
        let vtt = "WEBVTT\n\nNOTE made by hand\n\n1\n00:01.000 --> 00:04.000 align:start\n\
                   <v Ada>Hello &amp; welcome.</v>\n\n00:05.000 --> 00:06.000\n<i>Second</i> cue\n";
        assert_eq!(
            super::transcript_text(vtt.as_bytes(), "vtt")
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["Hello", "&", "welcome.", "Second", "cue"]
        );
        let srt = "1\r\n00:00:01,000 --> 00:00:02,000\r\nFirst line\r\n\r\n2\r\n\
                   00:00:03,000 --> 00:00:04,000\r\nIn 2024 we went\r\n";
        assert_eq!(
            super::transcript_text(srt.as_bytes(), "srt")
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["First", "line", "In", "2024", "we", "went"]
        );
        let json = r#"{"version":"1.0.0","segments":[
            {"speaker":"Ada","startTime":0.5,"body":"Good"},
            {"speaker":"Ada","startTime":0.9,"body":"evening"}]}"#;
        assert_eq!(
            super::transcript_text(json.as_bytes(), "json").trim(),
            "Good evening"
        );
        assert_eq!(super::transcript_text(b"{ not json", "json"), "");
        let html = "<p><cite>Ada:</cite> Good <b>night</b></p>";
        assert_eq!(
            super::transcript_text(html.as_bytes(), "html")
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["Ada:", "Good", "night"]
        );
        assert_eq!(
            super::transcript_text(b"Just words.", "txt").trim(),
            "Just words."
        );
    }

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
