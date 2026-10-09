//! Byte decoding and entity resolution for feed documents.

use std::borrow::Cow;

/// Result of decoding a feed body into text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    /// The document as UTF-8.
    pub text: String,
    /// Encoding that was applied (`utf-8`, `iso-8859-1`, `utf-16le`, …).
    pub encoding: &'static str,
    /// Notes about lossy or approximate decoding.
    pub warnings: Vec<String>,
}

/// Decodes raw bytes using the BOM or the XML declaration's `encoding`.
///
/// Supported: UTF-8 (default), UTF-16 (with BOM), ISO-8859-1 / Latin-1,
/// US-ASCII, and Windows-1252 (decoded with the Latin-1 table plus the
/// common 0x80–0x9F punctuation). Anything else is an error so the caller
/// can report it instead of producing mojibake.
pub fn decode_bytes(bytes: &[u8]) -> Result<DecodedText, UnsupportedEncoding> {
    let mut warnings = Vec::new();
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Ok(utf8(rest, "utf-8", warnings));
    }
    if bytes.len() >= 2 && (bytes[..2] == [0xFF, 0xFE] || bytes[..2] == [0xFE, 0xFF]) {
        let little = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if little {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        let text = String::from_utf16_lossy(&units);
        if text.contains('\u{FFFD}') {
            warnings.push("invalid UTF-16 sequences were replaced".to_owned());
        }
        return Ok(DecodedText {
            text,
            encoding: if little { "utf-16le" } else { "utf-16be" },
            warnings,
        });
    }
    let declared = declared_encoding(bytes).map(str::to_ascii_lowercase);
    match declared.as_deref() {
        None | Some("utf-8" | "utf8") => Ok(utf8(bytes, "utf-8", warnings)),
        Some("iso-8859-1" | "latin1" | "latin-1" | "iso8859-1" | "l1" | "us-ascii" | "ascii") => {
            Ok(DecodedText {
                text: latin1(bytes, false),
                encoding: "iso-8859-1",
                warnings,
            })
        }
        Some("windows-1252" | "cp1252" | "iso-8859-15" | "latin9") => {
            warnings.push(
                "windows-1252/latin-9 decoded with the latin-1 table plus common punctuation"
                    .to_owned(),
            );
            Ok(DecodedText {
                text: latin1(bytes, true),
                encoding: "windows-1252",
                warnings,
            })
        }
        Some(other) => Err(UnsupportedEncoding(other.to_owned())),
    }
}

/// Declared encoding that Uguisu cannot decode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported document encoding `{0}`")]
pub struct UnsupportedEncoding(pub String);

fn utf8(bytes: &[u8], encoding: &'static str, mut warnings: Vec<String>) -> DecodedText {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return DecodedText {
            text: s.to_owned(),
            encoding,
            warnings,
        };
    }
    warnings.push("invalid UTF-8 sequences were replaced".to_owned());
    DecodedText {
        text: String::from_utf8_lossy(bytes).into_owned(),
        encoding,
        warnings,
    }
}

fn latin1(bytes: &[u8], cp1252_punctuation: bool) -> String {
    bytes
        .iter()
        .map(|&b| {
            if cp1252_punctuation {
                match b {
                    0x80 => '€',
                    0x82 => '‚',
                    0x84 => '„',
                    0x85 => '…',
                    0x91 => '‘',
                    0x92 => '’',
                    0x93 => '“',
                    0x94 => '”',
                    0x95 => '•',
                    0x96 => '–',
                    0x97 => '—',
                    0x99 => '™',
                    _ => char::from(b),
                }
            } else {
                char::from(b)
            }
        })
        .collect()
}

/// Reads `encoding="…"` from an XML declaration in the first 200 bytes.
fn declared_encoding(bytes: &[u8]) -> Option<&str> {
    let head = &bytes[..bytes.len().min(200)];
    let head = std::str::from_utf8(head)
        .unwrap_or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or(""));
    let decl_end = head.find("?>")?;
    let decl = &head[..decl_end];
    if !decl.starts_with("<?xml") {
        return None;
    }
    let idx = decl.find("encoding")?;
    let rest = decl[idx + "encoding".len()..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].trim())
}

/// Resolves an entity or character reference name (without `&` and `;`).
///
/// Only the five predefined XML entities and numeric references are
/// resolved; DTD-defined entities are never expanded (entity-bomb safety).
pub(crate) fn resolve_reference(name: &str) -> Option<Cow<'static, str>> {
    match name {
        "amp" => Some(Cow::Borrowed("&")),
        "lt" => Some(Cow::Borrowed("<")),
        "gt" => Some(Cow::Borrowed(">")),
        "quot" => Some(Cow::Borrowed("\"")),
        "apos" => Some(Cow::Borrowed("'")),
        // A handful of HTML entities that show up in real feeds.
        "nbsp" => Some(Cow::Borrowed("\u{a0}")),
        "hellip" => Some(Cow::Borrowed("…")),
        "mdash" => Some(Cow::Borrowed("—")),
        "ndash" => Some(Cow::Borrowed("–")),
        "rsquo" => Some(Cow::Borrowed("’")),
        "lsquo" => Some(Cow::Borrowed("‘")),
        "rdquo" => Some(Cow::Borrowed("”")),
        "ldquo" => Some(Cow::Borrowed("“")),
        "copy" => Some(Cow::Borrowed("©")),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = if let Some(hex) = digits.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                digits.parse::<u32>().ok()?
            };
            char::from_u32(code)
                .filter(|c| *c != '\0')
                .map(|c| Cow::Owned(c.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn decodes_utf8_with_and_without_bom() {
        let d = decode_bytes("<?xml version=\"1.0\"?><rss/>".as_bytes()).expect("utf8");
        assert_eq!(d.encoding, "utf-8");
        let d = decode_bytes(b"\xEF\xBB\xBF<rss/>").expect("bom");
        assert_eq!(d.text, "<rss/>");
    }

    #[test]
    fn decodes_latin1_declaration() {
        let d = decode_bytes(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>Gem\xfctlich</t>")
            .expect("latin1");
        assert_eq!(d.encoding, "iso-8859-1");
        assert!(d.text.contains("Gemütlich"));
    }

    #[test]
    fn decodes_utf16le_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "<a/>".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let d = decode_bytes(&bytes).expect("utf16");
        assert_eq!((d.text.as_str(), d.encoding), ("<a/>", "utf-16le"));
    }

    #[test]
    fn unknown_encoding_is_refused() {
        assert!(decode_bytes(b"<?xml version='1.0' encoding='Shift_JIS'?><a/>").is_err());
        let d = decode_bytes(b"<?xml version=\"1.0\"?><t>\xff</t>").expect("lossy");
        assert!(!d.warnings.is_empty());
        assert!(d.text.contains('\u{FFFD}'));
    }

    #[test]
    fn resolves_only_safe_references() {
        assert_eq!(resolve_reference("amp").as_deref(), Some("&"));
        assert_eq!(resolve_reference("#8217").as_deref(), Some("’"));
        assert_eq!(resolve_reference("#x2014").as_deref(), Some("—"));
        assert_eq!(resolve_reference("lol5"), None);
        assert_eq!(resolve_reference("#0"), None);
    }
}
