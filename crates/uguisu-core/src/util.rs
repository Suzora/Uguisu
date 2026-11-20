use std::time::{SystemTime, UNIX_EPOCH};

/// Current time as unix seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
