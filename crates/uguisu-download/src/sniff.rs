//! Container detection from the first bytes of a download. Recorded for
//! diagnostics only: a publisher's wrong `Content-Type` never fails a
//! download, but an HTML error page served as audio becomes visible.

/// Bytes needed for a confident guess.
pub const SNIFF_LEN: usize = 16;

/// Guesses the container of `head` (at least the first 12 bytes).
#[must_use]
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    if head.len() < 4 {
        return None;
    }
    if head.starts_with(b"ID3") {
        return Some("mp3");
    }
    if head.starts_with(b"fLaC") {
        return Some("flac");
    }
    if head.starts_with(b"OggS") {
        return Some("ogg");
    }
    if head.starts_with(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WAVE" {
        return Some("wav");
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some("mp4");
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("webm");
    }
    if head.starts_with(b"ADIF") {
        return Some("aac");
    }
    // MPEG audio frame sync (11 set bits) — MP3 without a tag, or ADTS AAC.
    if head[0] == 0xFF && (head[1] & 0xE0) == 0xE0 {
        return Some(if (head[1] & 0xF6) == 0xF0 {
            "aac"
        } else {
            "mp3"
        });
    }
    let text = head
        .iter()
        .take(SNIFF_LEN)
        .map(u8::to_ascii_lowercase)
        .collect::<Vec<u8>>();
    let trimmed = text
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .copied()
        .collect::<Vec<u8>>();
    if trimmed.starts_with(b"<!doctype") || trimmed.starts_with(b"<html") {
        return Some("html");
    }
    if trimmed.starts_with(b"<?xml") {
        return Some("xml");
    }
    if trimmed.starts_with(b"{") || trimmed.starts_with(b"[") {
        return Some("json");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_containers_and_error_pages() {
        assert_eq!(sniff(b"ID3\x04\x00\x00\x00\x00\x00\x00"), Some("mp3"));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x00, 0, 0, 0, 0]), Some("mp3"));
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80, 0, 0, 0, 0]), Some("aac"));
        assert_eq!(sniff(b"fLaC\x00\x00\x00\x22"), Some("flac"));
        assert_eq!(sniff(b"OggS\x00\x02\x00\x00"), Some("ogg"));
        assert_eq!(sniff(b"RIFF\x24\x08\x00\x00WAVEfmt "), Some("wav"));
        assert_eq!(sniff(b"\x00\x00\x00\x20ftypM4A \x00\x00"), Some("mp4"));
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0]), Some("webm"));
        assert_eq!(sniff(b"<!DOCTYPE html><html>"), Some("html"));
        assert_eq!(sniff(b"  <html lang=en>"), Some("html"));
        assert_eq!(sniff(b"<?xml version=\"1.0\"?>"), Some("xml"));
        assert_eq!(sniff(b"{\"error\":\"nope\"}"), Some("json"));
        assert_eq!(sniff(b"\x00\x01\x02\x03\x04\x05\x06\x07"), None);
        assert_eq!(sniff(b"ID"), None);
    }
}
