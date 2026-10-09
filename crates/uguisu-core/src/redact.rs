//! What a log line may say about a URL.
//!
//! A feed or enclosure URL can carry a credential in its userinfo, its
//! query (`?token=…`, signed CDN parameters) or its fragment, whatever the
//! parameter is called. Log output keeps scheme, host, port and path and
//! never anything else; API responses, events and the database keep the URL
//! whole. See `docs/SECURITY.md` §3.5.

use std::fmt;

use crate::config::REDACTED;

/// Displays `text` with the userinfo, query and fragment of every URL in it
/// replaced by [`REDACTED`].
///
/// `text` is a URL or a message quoting URLs, such as an HTTP error. A URL
/// runs from its scheme to the next whitespace, double quote or angle
/// bracket, which a parsed URL percent-encodes outside its host, or to where
/// another URL starts before its query. An apostrophe or a backtick can be
/// part of a URL, so a closing one after a query or fragment is dropped with
/// it rather than ending the URL early and leaking the rest.
#[must_use]
pub fn urls(text: &str) -> Urls<'_> {
    Urls(text)
}

/// The [`Display`](fmt::Display) of [`urls`].
#[derive(Debug, Clone, Copy)]
pub struct Urls<'a>(&'a str);

impl fmt::Display for Urls<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rest = self.0;
        while let Some(sep) = rest.find("://") {
            let start = scheme_start(&rest[..sep]);
            f.write_str(&rest[..sep + 3])?;
            let after = &rest[sep + 3..];
            if start == sep {
                rest = after;
                continue;
            }
            let mut end = after
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '<' | '>'))
                .unwrap_or(after.len());
            // A URL glued on before this one's query (`'a','b'`) gets its own
            // pass; one inside the query or fragment is redacted with it.
            let head = &after[..after[..end].find(['?', '#']).unwrap_or(end)];
            if let Some(glued) = head
                .match_indices("://")
                .map(|(i, _)| (scheme_start(&head[..i]), i))
                .find(|&(start, sep)| start < sep)
            {
                end = glued.0;
            }
            write_after_scheme(f, &after[..end])?;
            rest = &after[end..];
        }
        f.write_str(rest)
    }
}

/// Where the scheme ending at the end of `before` starts.
fn scheme_start(before: &str) -> usize {
    before
        .char_indices()
        .rev()
        .find(|&(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
        .map_or(0, |(i, c)| i + c.len_utf8())
}

/// Writes `authority` + path of one URL, with userinfo, query and fragment
/// replaced.
fn write_after_scheme(f: &mut fmt::Formatter<'_>, url: &str) -> fmt::Result {
    let (authority, tail) = url.split_at(url.find(['/', '?', '#']).unwrap_or(url.len()));
    match authority.rfind('@') {
        Some(at) => write!(f, "{REDACTED}@{}", &authority[at + 1..])?,
        None => f.write_str(authority)?,
    }
    let (path, tail) = tail.split_at(tail.find(['?', '#']).unwrap_or(tail.len()));
    f.write_str(path)?;
    let fragment = match tail.strip_prefix('?') {
        Some(query) => {
            write!(f, "?{REDACTED}")?;
            query.find('#').map(|i| &query[i..])
        }
        None => Some(tail).filter(|t| !t.is_empty()),
    };
    if fragment.is_some() {
        write!(f, "#{REDACTED}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> String {
        urls(text).to_string()
    }

    #[test]
    fn query_replaced_whole() {
        assert_eq!(
            r("https://feeds.example.com/show.xml?token=SEKRIT&a=1"),
            "https://feeds.example.com/show.xml?[redacted]"
        );
    }

    #[test]
    fn userinfo_replaced() {
        assert_eq!(
            r("https://alice:hunter2@example.com:8443/feed"),
            "https://[redacted]@example.com:8443/feed"
        );
        assert_eq!(
            r("http://user@pass@example.com"),
            "http://[redacted]@example.com"
        );
    }

    #[test]
    fn fragment_replaced() {
        assert_eq!(
            r("https://example.com/p#access_token=x"),
            "https://example.com/p#[redacted]"
        );
        assert_eq!(
            r("https://example.com/p?q=1#frag?x"),
            "https://example.com/p?[redacted]#[redacted]"
        );
    }

    #[test]
    fn ipv6_host_kept() {
        assert_eq!(
            r("http://u:p@[2001:db8::1]:8080/a/b?k=v"),
            "http://[redacted]@[2001:db8::1]:8080/a/b?[redacted]"
        );
    }

    #[test]
    fn plain_url_unchanged() {
        for url in [
            "https://example.com",
            "https://example.com/",
            "http://127.0.0.1:8484/feed/@user/ep.mp3",
        ] {
            assert_eq!(r(url), url);
        }
    }

    #[test]
    fn urls_inside_messages() {
        assert_eq!(
            r("error sending request for url (http://h/f?token=SEKRIT): refused"),
            "error sending request for url (http://h/f?[redacted] refused"
        );
        assert_eq!(
            r("feed url changed to https://a@b/x?s=1 (verified); then wss://c/d?e \"ftp://f?g\""),
            "feed url changed to https://[redacted]@b/x?[redacted] (verified); then wss://c/d?[redacted] \"ftp://f?[redacted]\""
        );
    }

    #[test]
    fn apostrophe_or_backtick_never_leaks() {
        assert_eq!(
            r("https://cdn.example/Bob's/ep.mp3?token=SEKRIT"),
            "https://cdn.example/Bob's/ep.mp3?[redacted]"
        );
        assert_eq!(r("https://bob:it's@h/f"), "https://[redacted]@h/f");
        assert_eq!(r("https://h/f?sig=ab`cd"), "https://h/f?[redacted]");
        assert_eq!(
            r("cannot resolve `https://h/f?t=x`: not a feed"),
            "cannot resolve `https://h/f?[redacted] not a feed"
        );
        assert_eq!(
            r("cannot resolve `https://h/f`: not a feed"),
            "cannot resolve `https://h/f`: not a feed"
        );
    }

    #[test]
    fn glued_urls_redacted_separately() {
        assert_eq!(
            r("'https://a/x','https://u:p@b/y'"),
            "'https://a/x','https://[redacted]@b/y'"
        );
        assert_eq!(
            r("`https://a`,`https://u:p@b`"),
            "`https://a`,`https://[redacted]@b`"
        );
        assert_eq!(
            r("https://web.example/2020/https://h/f?token=x"),
            "https://web.example/2020/https://h/f?[redacted]"
        );
        assert_eq!(
            r("https://a/r?u=https://b/private"),
            "https://a/r?[redacted]"
        );
    }

    #[test]
    fn text_without_urls_unchanged() {
        for text in ["", "no url here? a@b", "://x?y", "ünïcode → ://?z"] {
            assert_eq!(r(text), text);
        }
    }

    #[test]
    fn unicode_around_scheme() {
        assert_eq!(
            r("→https://ü.example/ä?ö"),
            "→https://ü.example/ä?[redacted]"
        );
    }
}
