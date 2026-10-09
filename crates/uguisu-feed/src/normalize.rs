//! Normalization of parsed values into the domain model
//! (`docs/FEED_ENGINE.md` "Normalization").
//!
//! Rules: keep the raw value next to the normalized one where it matters
//! (dates, durations); never invent data (missing stays `None`); never drop
//! an item because one field is bad; classify instead of guessing.

use time::OffsetDateTime;
use uguisu_core::ids::{EnclosureId, EpisodeId};
use uguisu_core::model::{DateQuality, Enclosure, EnclosureKind, EpisodeExtras, sort_title};
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::dates::parse_date;
use crate::parser::{EnclosureOrigin, ParsedChannel, ParsedEnclosure, ParsedItem};

/// A normalized date with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDate {
    /// The value as written.
    pub raw: String,
    /// The normalized instant (UTC), when the value could be parsed.
    pub value: Option<OffsetDateTime>,
    /// Classification of the result.
    pub quality: DateQuality,
}

/// Dates before this year are classified `Ancient` (podcasting predates
/// nothing here; feeds carrying 1970 or 1900 dates are misconfigured).
pub const ANCIENT_BEFORE_YEAR: i32 = 1990;

/// Parses a feed timestamp and classifies the result relative to `now`.
#[must_use]
pub fn parse_date_detailed(raw: &str, now: OffsetDateTime) -> ParsedDate {
    let trimmed = raw.trim();
    let Some(value) = parse_date(trimmed) else {
        return ParsedDate {
            raw: trimmed.to_owned(),
            value: None,
            quality: DateQuality::Invalid,
        };
    };
    let utc = value.to_offset(time::UtcOffset::UTC);
    let assumed_utc = !has_explicit_offset(trimmed);
    let quality = if utc > now + time::Duration::days(1) {
        DateQuality::Future
    } else if utc.year() < ANCIENT_BEFORE_YEAR {
        DateQuality::Ancient
    } else if assumed_utc {
        DateQuality::AssumedUtc
    } else {
        DateQuality::Exact
    };
    ParsedDate {
        raw: trimmed.to_owned(),
        value: Some(utc),
        quality,
    }
}

/// Whether the string names a zone or offset explicitly.
fn has_explicit_offset(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    if upper.ends_with('Z') {
        return true;
    }
    for zone in [
        " GMT", " UTC", " UT", " EST", " EDT", " CST", " CDT", " MST", " MDT", " PST", " PDT",
    ] {
        if upper.ends_with(zone) {
            return true;
        }
    }
    // A trailing +HHMM / -HH:MM / +HH offset.
    let tail: String = upper
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let bytes = tail.as_bytes();
    if let Some(pos) = tail.rfind(['+', '-']) {
        let rest = &tail[pos + 1..];
        let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
        // Avoid matching the date part (e.g. "2025-09-01"): require the sign
        // to follow a time component.
        let before = &s[..s.len().saturating_sub(tail.len() - pos)];
        if (digits.len() == 2 || digits.len() == 4)
            && rest.chars().all(|c| c.is_ascii_digit() || c == ':')
            && (before.contains(':') || before.ends_with('T'))
        {
            return true;
        }
    }
    let _ = bytes;
    false
}

/// Quality of a normalized duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationQuality {
    /// Parsed from a recognised form.
    Exact,
    /// Parsed, but implausibly long (> 30 days).
    Implausible,
    /// Could not be parsed.
    Invalid,
}

/// Longest duration accepted as plausible.
pub const MAX_PLAUSIBLE_SECS: u32 = 30 * 24 * 3600;

/// Parses `itunes:duration` in its common shapes: `HH:MM:SS`, `MM:SS`,
/// `SS`, fractional seconds, `1h 2m 3s` and ISO 8601 `PT1H2M3S`.
#[must_use]
pub fn parse_duration(raw: &str) -> (Option<u32>, DurationQuality) {
    let s = raw.trim();
    if s.is_empty() {
        return (None, DurationQuality::Invalid);
    }
    let secs = parse_clock(s)
        .or_else(|| parse_seconds(s))
        .or_else(|| parse_words(s))
        .or_else(|| parse_iso8601(s));
    match secs {
        None => (None, DurationQuality::Invalid),
        Some(n) if n > MAX_PLAUSIBLE_SECS => (Some(n), DurationQuality::Implausible),
        Some(n) => (Some(n), DurationQuality::Exact),
    }
}

fn parse_clock(s: &str) -> Option<u32> {
    if !s.contains(':') {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let mut total: u64 = 0;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let n: u64 = if last {
            parse_seconds_part(part)?
        } else {
            part.trim().parse().ok()?
        };
        total = total.checked_mul(60)?.checked_add(n)?;
    }
    u32::try_from(total).ok()
}

fn parse_seconds_part(part: &str) -> Option<u64> {
    let p = part.trim();
    if let Ok(n) = p.parse::<u64>() {
        return Some(n);
    }
    let f: f64 = p.parse().ok()?;
    if !f.is_finite() || f < 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // checked non-negative and finite
    Some(f.round() as u64)
}

fn parse_seconds(s: &str) -> Option<u32> {
    let n = parse_seconds_part(s)?;
    u32::try_from(n).ok()
}

fn parse_words(s: &str) -> Option<u32> {
    // "1h 2m 3s", "1 hr 2 min", "45 minutes"
    let lower = s.to_ascii_lowercase();
    let mut total: u64 = 0;
    let mut number = String::new();
    let mut matched = false;
    let mut chars = lower.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        if number.is_empty() {
            return None;
        }
        let mut unit = String::from(c);
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphabetic() {
                unit.push(n);
                chars.next();
            } else {
                break;
            }
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        let mult = match unit.as_str() {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            _ => return None,
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let add = (value * mult).round() as u64;
        total = total.checked_add(add)?;
        matched = true;
    }
    if !number.is_empty() || !matched {
        return None;
    }
    u32::try_from(total).ok()
}

fn parse_iso8601(s: &str) -> Option<u32> {
    let rest = s.strip_prefix(['P', 'p'])?;
    let rest = rest.strip_prefix(['T', 't']).unwrap_or(rest);
    let mut total: u64 = 0;
    let mut number = String::new();
    let mut any = false;
    for c in rest.chars() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
            continue;
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        let mult = match c.to_ascii_uppercase() {
            'H' => 3600.0,
            'M' => 60.0,
            'S' => 1.0,
            'D' => 86400.0,
            _ => return None,
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let add = (value * mult).round() as u64;
        total = total.checked_add(add)?;
        any = true;
    }
    if !number.is_empty() || !any {
        return None;
    }
    u32::try_from(total).ok()
}

/// Reduces markup to readable text: tags removed, block boundaries turned
/// into newlines, entities that survived parsing decoded, whitespace
/// collapsed, NFC-normalized, capped at `max_chars`.
#[must_use]
#[allow(clippy::too_many_lines)] // one linear scan; splitting it would obscure the state machine
pub fn html_to_text(html: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip_depth = 0usize; // inside <script>/<style>
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_ascii_lowercase();
                let closing = tag.starts_with('/');
                match name.as_str() {
                    "script" | "style" => {
                        if closing {
                            skip_depth = skip_depth.saturating_sub(1);
                        } else if !tag.ends_with('/') {
                            skip_depth += 1;
                        }
                    }
                    "br" | "p" | "div" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr"
                    | "ul" | "ol" | "blockquote" | "pre" | "hr" | "table"
                        if !closing || name == "p" || name == "div" || name == "li" =>
                    {
                        out.push('\n');
                    }
                    _ => {}
                }
                tag.clear();
            } else {
                tag.push(c);
            }
            continue;
        }
        if c == '<' {
            // Only treat as a tag when it looks like one.
            match chars.peek() {
                Some(n) if n.is_ascii_alphabetic() || *n == '/' || *n == '!' => {
                    in_tag = true;
                    continue;
                }
                _ => {}
            }
        }
        if skip_depth > 0 {
            continue;
        }
        if c == '&' {
            let mut name = String::new();
            let mut lookahead = chars.clone();
            let mut ok = false;
            for _ in 0..10 {
                match lookahead.next() {
                    Some(';') => {
                        ok = true;
                        break;
                    }
                    Some(x) if x.is_ascii_alphanumeric() || x == '#' => name.push(x),
                    _ => break,
                }
            }
            if ok && let Some(resolved) = crate::text::resolve_reference(&name) {
                out.push_str(&resolved);
                chars = lookahead;
                continue;
            }
        }
        out.push(c);
    }
    if in_tag {
        // An unterminated `<...` is text, not markup; keep it so the
        // function is stable when applied twice.
        out.push('<');
        out.push_str(&tag);
    }
    let mut collapsed = String::with_capacity(out.len());
    let mut pending_newlines = 0;
    let mut pending_space = false;
    for c in out.nfc() {
        if c == '\n' {
            pending_newlines += 1;
            pending_space = false;
        } else if c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_newlines > 0 && !collapsed.is_empty() {
                collapsed.push('\n');
            } else if pending_space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            pending_newlines = 0;
            pending_space = false;
            collapsed.push(c);
        }
    }
    let mut text: String = collapsed.chars().take(max_chars).collect();
    if collapsed.chars().count() > max_chars {
        text.push('…');
    }
    text
}

/// NFC-normalizes and collapses whitespace in a short field.
#[must_use]
pub fn clean_field(s: &str) -> String {
    s.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses `itunes:explicit` / `podcast:locked` style flags.
#[must_use]
pub fn parse_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "explicit" | "1" => Some(true),
        "no" | "false" | "clean" | "0" => Some(false),
        _ => None,
    }
}

/// Parses a small non-negative integer (season, episode number).
#[must_use]
pub fn parse_small_number(raw: &str) -> Option<u32> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<u32>().ok().or_else(|| {
        let f: f64 = t.parse().ok()?;
        if f.fract() == 0.0 && f >= 0.0 && f < f64::from(u32::MAX) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(f as u32)
        } else {
            None
        }
    })
}

/// Parses a URL as written, tolerating surrounding whitespace and a
/// missing scheme for `//host/path` forms.
#[must_use]
pub fn parse_url(raw: &str) -> Option<Url> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(u) = Url::parse(t) {
        return Some(u).filter(Url::has_host);
    }
    if t.starts_with("//") {
        return Url::parse(&format!("https:{t}")).ok();
    }
    None
}

/// Normalizes the enclosures of an item into domain enclosures. The first
/// `enclosure`/Atom link/RDF enclosure is primary; `media:content` counts
/// only when nothing else declares media; alternate enclosures follow.
#[must_use]
pub fn normalize_enclosures(
    episode_id: EpisodeId,
    parsed: &[ParsedEnclosure],
    warnings: &mut Vec<String>,
) -> Vec<Enclosure> {
    let has_real = parsed
        .iter()
        .any(|e| !matches!(e.origin, EnclosureOrigin::MediaContent));
    let mut out: Vec<Enclosure> = Vec::new();
    let mut primary_set = false;
    for e in parsed {
        if matches!(e.origin, EnclosureOrigin::MediaContent) && has_real {
            continue;
        }
        let Some(url) = parse_url(&e.url) else {
            warnings.push(format!(
                "enclosure url `{}` is not a valid URL",
                cap(&e.url, 80)
            ));
            continue;
        };
        if out.iter().any(|x| x.url == url) {
            continue;
        }
        let mime_type = e
            .mime_type
            .as_deref()
            .map(|m| m.trim().to_ascii_lowercase())
            .filter(|m| !m.is_empty());
        let length_bytes = e.length.as_deref().and_then(|l| {
            let t = l.trim();
            if t.is_empty() {
                return None;
            }
            let n = t.parse::<u64>().ok();
            if n.is_none() {
                warnings.push(format!("enclosure length `{}` is not a number", cap(t, 40)));
            }
            n
        });
        let is_primary =
            !primary_set && (!matches!(e.origin, EnclosureOrigin::AlternateEnclosure) || e.default);
        if is_primary {
            primary_set = true;
        }
        let position = u32::try_from(out.len()).unwrap_or(u32::MAX);
        out.push(Enclosure {
            id: EnclosureId::new(),
            episode_id,
            url,
            kind: EnclosureKind::from_mime(mime_type.as_deref()),
            mime_type,
            length_bytes,
            is_primary,
            position,
            bitrate: e.bitrate.as_deref().and_then(|b| b.trim().parse().ok()),
            height: e.height.as_deref().and_then(|h| h.trim().parse().ok()),
            codecs: e.codecs.clone(),
            lang: e.lang.clone(),
            title: e.title.clone(),
            integrity_type: e.integrity_type.clone(),
            integrity_value: e.integrity_value.clone(),
            sources: e.sources.clone(),
        });
    }
    if !primary_set && let Some(first) = out.first_mut() {
        first.is_primary = true;
    }
    out
}

/// Builds the Podcasting 2.0 extras document of an item.
#[must_use]
pub fn normalize_extras(item: &ParsedItem) -> EpisodeExtras {
    EpisodeExtras {
        chapters: item.chapters.clone(),
        transcripts: item.transcripts.clone(),
        persons: item.persons.clone(),
        location: item.location.clone(),
        soundbites: item.soundbites.clone(),
        value: item
            .value
            .as_ref()
            .and_then(|v| serde_json::to_value(v).ok()),
        funding: Vec::new(),
        license: item.license.clone(),
        txt: item.txt.clone(),
        raw_extensions: item.raw_extensions.clone(),
    }
}

/// Normalized episode fields derived from one item (identity is computed
/// separately by [`crate::identity`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedItem {
    /// Title (falls back to the enclosure file name or `Untitled episode`).
    pub title: String,
    /// The title the fingerprint is computed from: the item's own text, which
    /// is empty for a title of markup only, or `None` when the item had none
    /// and `title` was invented. Stored identity keys depend on it, so it
    /// never changes for an item whose feed did not change.
    pub identity_title: Option<String>,
    /// Sort key of the title.
    pub sort_title: String,
    /// Subtitle.
    pub subtitle: Option<String>,
    /// Description with markup (longest of description / content / summary).
    pub description_html: Option<String>,
    /// Description as text.
    pub description_text: Option<String>,
    /// Link.
    pub link: Option<Url>,
    /// Publication date.
    pub published: ParsedDate,
    /// Source update date.
    pub updated_at_source: Option<OffsetDateTime>,
    /// Duration in seconds.
    pub duration_secs: Option<u32>,
    /// Duration as written.
    pub duration_raw: Option<String>,
    /// Season.
    pub season: Option<u32>,
    /// Episode number.
    pub episode_number: Option<u32>,
    /// Episode type.
    pub episode_type: Option<String>,
    /// Explicit flag.
    pub explicit: Option<bool>,
    /// Artwork.
    pub artwork_url: Option<Url>,
    /// Author.
    pub author: Option<String>,
    /// Enclosures.
    pub enclosures: Vec<Enclosure>,
    /// Extras.
    pub extras: EpisodeExtras,
    /// Warnings raised while normalizing.
    pub warnings: Vec<String>,
}

/// Longest description text kept.
pub const MAX_DESCRIPTION_CHARS: usize = 20_000;

/// Normalizes one parsed item. `episode_id` is the id the enclosures will
/// belong to (a fresh one for new episodes, the stored one for updates).
#[must_use]
#[allow(clippy::too_many_lines)] // one pass over every item field
pub fn normalize_item(
    item: &ParsedItem,
    episode_id: EpisodeId,
    now: OffsetDateTime,
) -> NormalizedItem {
    let mut warnings = item.warnings.clone();
    let enclosures = normalize_enclosures(episode_id, &item.enclosures, &mut warnings);
    let text = item
        .title
        .as_deref()
        .map(clean_field)
        .filter(|t| !t.is_empty())
        .map(|t| html_to_text(&t, 1024));
    let (title, identity_title) = if let Some(t) = text.as_ref().filter(|t| !t.is_empty()) {
        (t.clone(), text.clone())
    } else {
        warnings.push(
            if text.is_some() {
                "item title has no text"
            } else {
                "item has no title"
            }
            .to_owned(),
        );
        // `itunes_title` is set only beside a `title` (parser.rs), so this
        // reaches the markup-only case alone.
        let fallback = item
            .itunes_title
            .as_deref()
            .map(|t| html_to_text(&clean_field(t), 1024))
            .filter(|t| !t.is_empty())
            .or_else(|| {
                enclosures
                    .first()
                    .and_then(|e| {
                        e.url
                            .path_segments()
                            .and_then(|mut s| s.next_back().map(str::to_owned))
                    })
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "Untitled episode".to_owned());
        (fallback, text)
    };
    let description_html = [
        item.content.as_deref(),
        item.description.as_deref(),
        item.summary.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .max_by_key(|s| s.len())
    .map(str::to_owned);
    let description_text = description_html
        .as_deref()
        .map(|h| html_to_text(h, MAX_DESCRIPTION_CHARS))
        .filter(|t| !t.is_empty());
    let published = match item.pub_date.as_deref() {
        Some(raw) => {
            let d = parse_date_detailed(raw, now);
            match d.quality {
                DateQuality::Invalid => {
                    warnings.push(format!("unparseable date `{}`", cap(raw, 40)));
                }
                DateQuality::Future => warnings.push(format!(
                    "publication date `{}` lies in the future",
                    cap(raw, 40)
                )),
                DateQuality::Ancient => warnings.push(format!(
                    "publication date `{}` is before {ANCIENT_BEFORE_YEAR}",
                    cap(raw, 40)
                )),
                DateQuality::AssumedUtc | DateQuality::Exact => {}
            }
            d
        }
        None => ParsedDate {
            raw: String::new(),
            value: None,
            quality: DateQuality::Invalid,
        },
    };
    let (duration_secs, duration_raw) = match item.duration.as_deref() {
        Some(raw) => {
            let (secs, q) = parse_duration(raw);
            match q {
                DurationQuality::Invalid => {
                    warnings.push(format!("unparseable duration `{}`", cap(raw, 40)));
                }
                DurationQuality::Implausible => {
                    warnings.push(format!("implausible duration `{}`", cap(raw, 40)));
                }
                DurationQuality::Exact => {}
            }
            (secs, Some(raw.trim().to_owned()))
        }
        None => (None, None),
    };
    NormalizedItem {
        sort_title: sort_title(&title),
        title,
        identity_title,
        subtitle: item
            .subtitle
            .as_deref()
            .map(clean_field)
            .filter(|s| !s.is_empty()),
        description_html,
        description_text,
        link: item.link.as_deref().and_then(parse_url),
        published,
        updated_at_source: item
            .updated
            .as_deref()
            .and_then(parse_date)
            .map(|d| d.to_offset(time::UtcOffset::UTC)),
        duration_secs,
        duration_raw,
        season: item.season.as_deref().and_then(parse_small_number),
        episode_number: item.episode.as_deref().and_then(parse_small_number),
        episode_type: item
            .episode_type
            .as_deref()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty()),
        explicit: item.explicit.as_deref().and_then(parse_flag),
        artwork_url: item.image.as_deref().and_then(parse_url),
        author: item
            .author
            .as_deref()
            .map(clean_field)
            .filter(|s| !s.is_empty()),
        enclosures,
        extras: normalize_extras(item),
        warnings,
    }
}

/// Normalized channel fields.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedChannel {
    /// Title (`Untitled podcast` when missing).
    pub title: String,
    /// Sort key.
    pub sort_title: String,
    /// Subtitle.
    pub subtitle: Option<String>,
    /// Author.
    pub author: Option<String>,
    /// Publisher.
    pub publisher: Option<String>,
    /// Owner name.
    pub owner_name: Option<String>,
    /// Owner email.
    pub owner_email: Option<String>,
    /// Description with markup.
    pub description_html: Option<String>,
    /// Description as text.
    pub description_text: Option<String>,
    /// Website.
    pub website: Option<Url>,
    /// Artwork.
    pub artwork_url: Option<Url>,
    /// Language.
    pub language: Option<String>,
    /// Categories.
    pub categories: Vec<String>,
    /// Explicit.
    pub explicit: Option<bool>,
    /// Copyright.
    pub copyright: Option<String>,
    /// `podcast:guid`.
    pub podcast_guid: Option<String>,
    /// `itunes:new-feed-url`.
    pub new_feed_url: Option<Url>,
    /// `atom:link rel=self`.
    pub self_link: Option<Url>,
    /// `podcast:locked`.
    pub locked: Option<bool>,
    /// Warnings.
    pub warnings: Vec<String>,
}

/// Normalizes the channel.
#[must_use]
pub fn normalize_channel(ch: &ParsedChannel) -> NormalizedChannel {
    let mut warnings = Vec::new();
    let title = ch
        .title
        .as_deref()
        .map(|t| html_to_text(&clean_field(t), 512))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            warnings.push("feed has no title".to_owned());
            "Untitled podcast".to_owned()
        });
    let description_html = [ch.description.as_deref(), ch.summary.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .max_by_key(|s| s.len())
        .map(str::to_owned);
    let publisher = ch
        .publisher
        .as_deref()
        .map(clean_field)
        .filter(|p| !p.is_empty());
    let author = ch
        .author
        .as_deref()
        .map(clean_field)
        .filter(|a| !a.is_empty());
    NormalizedChannel {
        sort_title: sort_title(&title),
        title,
        subtitle: ch
            .subtitle
            .as_deref()
            .map(clean_field)
            .filter(|s| !s.is_empty()),
        publisher: publisher.filter(|p| Some(p) != author.as_ref()),
        author,
        owner_name: ch
            .owner_name
            .as_deref()
            .map(clean_field)
            .filter(|s| !s.is_empty()),
        owner_email: ch
            .owner_email
            .as_deref()
            .map(clean_field)
            .filter(|s| !s.is_empty()),
        description_text: description_html
            .as_deref()
            .map(|h| html_to_text(h, MAX_DESCRIPTION_CHARS))
            .filter(|t| !t.is_empty()),
        description_html,
        website: ch.link.as_deref().and_then(parse_url),
        artwork_url: ch.image.as_deref().and_then(parse_url),
        language: ch
            .language
            .as_deref()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty()),
        categories: ch
            .categories
            .iter()
            .map(|c| clean_field(c))
            .filter(|c| !c.is_empty())
            .collect(),
        explicit: ch.explicit.as_deref().and_then(parse_flag),
        copyright: ch
            .copyright
            .as_deref()
            .map(clean_field)
            .filter(|c| !c.is_empty()),
        podcast_guid: ch
            .podcast_guid
            .as_deref()
            .map(|g| g.trim().to_owned())
            .filter(|g| !g.is_empty()),
        new_feed_url: ch.new_feed_url.as_deref().and_then(parse_url),
        self_link: ch.self_link.as_deref().and_then(parse_url),
        locked: ch.locked.as_deref().and_then(parse_flag),
        warnings,
    }
}

fn cap(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-09-17 12:00:00 UTC);

    #[test]
    fn dates_are_classified() {
        let d = parse_date_detailed("Mon, 01 Sep 2025 12:00:00 +0200", NOW);
        assert_eq!(d.quality, DateQuality::Exact);
        assert_eq!(d.value.unwrap(), datetime!(2025-09-01 10:00:00 UTC));
        assert_eq!(
            parse_date_detailed("2025-09-01T10:00:00Z", NOW).quality,
            DateQuality::Exact
        );
        assert_eq!(
            parse_date_detailed("Mon, 01 Sep 2025 10:00:00 GMT", NOW).quality,
            DateQuality::Exact
        );
        assert_eq!(
            parse_date_detailed("2025-09-01T10:00:00", NOW).quality,
            DateQuality::AssumedUtc
        );
        assert_eq!(
            parse_date_detailed("2025-09-01", NOW).quality,
            DateQuality::AssumedUtc
        );
        assert_eq!(
            parse_date_detailed("Fri, 01 Jan 2100 00:00:00 +0000", NOW).quality,
            DateQuality::Future
        );
        assert_eq!(
            parse_date_detailed("Thu, 01 Jan 1970 00:00:00 +0000", NOW).quality,
            DateQuality::Ancient
        );
        let bad = parse_date_detailed(" yesterday-ish ", NOW);
        assert_eq!(bad.quality, DateQuality::Invalid);
        assert_eq!(bad.raw, "yesterday-ish");
        assert!(bad.value.is_none());
        // Tomorrow is still fine (feeds publish "today" in another zone).
        let soon = (NOW + time::Duration::hours(20))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        assert_eq!(parse_date_detailed(&soon, NOW).quality, DateQuality::Exact);
    }

    #[test]
    fn durations_in_every_common_shape() {
        assert_eq!(
            parse_duration("1:02:03"),
            (Some(3723), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration("45:10"),
            (Some(2710), DurationQuality::Exact)
        );
        assert_eq!(parse_duration("3600"), (Some(3600), DurationQuality::Exact));
        assert_eq!(
            parse_duration("1834.5"),
            (Some(1835), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration(" 00:05:00 "),
            (Some(300), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration("1h 2m"),
            (Some(3720), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration("45 minutes"),
            (Some(2700), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration("PT1H2M3S"),
            (Some(3723), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration("00:00:59.9"),
            (Some(60), DurationQuality::Exact)
        );
        assert_eq!(
            parse_duration("99999999"),
            (Some(99_999_999), DurationQuality::Implausible)
        );
        assert_eq!(
            parse_duration("about an hour"),
            (None, DurationQuality::Invalid)
        );
        assert_eq!(parse_duration(""), (None, DurationQuality::Invalid));
        assert_eq!(parse_duration("1:2:3:4"), (None, DurationQuality::Invalid));
        assert_eq!(parse_duration("-5"), (None, DurationQuality::Invalid));
    }

    #[test]
    fn html_becomes_readable_text() {
        let t = html_to_text(
            "<div><h1>Full</h1><p>Notes &copy; 2025<br/>Second   line</p><script>alert(1)</script><p>End</p></div>",
            1000,
        );
        assert_eq!(t, "Full\nNotes © 2025\nSecond line\nEnd");
        assert_eq!(html_to_text("a &lt; b &amp;&amp; c", 100), "a < b && c");
        assert_eq!(html_to_text("5 < 6 and 7 > 3", 100), "5 < 6 and 7 > 3");
        assert_eq!(html_to_text("long text here", 4), "long…");
        assert_eq!(html_to_text("e\u{301}", 10), "é", "NFC");
        assert_eq!(html_to_text("  ", 10), "");
    }

    #[test]
    fn flags_numbers_and_urls() {
        assert_eq!(parse_flag("Yes"), Some(true));
        assert_eq!(parse_flag("clean"), Some(false));
        assert_eq!(parse_flag("maybe"), None);
        assert_eq!(parse_small_number("7"), Some(7));
        assert_eq!(parse_small_number("7.0"), Some(7));
        assert_eq!(parse_small_number("seven"), None);
        assert_eq!(
            parse_url("//cdn.example/x.mp3").unwrap().as_str(),
            "https://cdn.example/x.mp3"
        );
        assert!(parse_url("not a url").is_none());
        assert!(parse_url("mailto:x@y").is_none());
    }

    #[test]
    fn enclosures_pick_a_primary() {
        let id = EpisodeId::new();
        let mut w = Vec::new();
        let parsed = vec![
            ParsedEnclosure {
                url: "https://cdn.example/a.mp3".into(),
                mime_type: Some("Audio/MPEG".into()),
                length: Some("100".into()),
                origin: EnclosureOrigin::Enclosure,
                bitrate: None,
                height: None,
                codecs: None,
                lang: None,
                title: None,
                default: false,
                integrity_type: None,
                integrity_value: None,
                sources: vec![],
            },
            ParsedEnclosure {
                url: "https://cdn.example/a.m4a".into(),
                mime_type: None,
                length: Some("abc".into()),
                origin: EnclosureOrigin::MediaContent,
                bitrate: None,
                height: None,
                codecs: None,
                lang: None,
                title: None,
                default: false,
                integrity_type: None,
                integrity_value: None,
                sources: vec![],
            },
            ParsedEnclosure {
                url: "https://cdn.example/a.opus".into(),
                mime_type: Some("audio/opus".into()),
                length: Some("50".into()),
                origin: EnclosureOrigin::AlternateEnclosure,
                bitrate: Some("64000".into()),
                height: None,
                codecs: None,
                lang: None,
                title: Some("Opus".into()),
                default: false,
                integrity_type: Some("sri".into()),
                integrity_value: Some("x".into()),
                sources: vec!["ipfs://q".into()],
            },
        ];
        let out = normalize_enclosures(id, &parsed, &mut w);
        assert_eq!(out.len(), 2);
        assert!(out[0].is_primary && !out[1].is_primary);
        assert_eq!(out[0].mime_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(out[0].kind, EnclosureKind::Audio);
        assert_eq!(out[1].bitrate, Some(64000));
        assert_eq!(out[1].position, 1);
        assert!(w.is_empty());
        // media:content alone becomes the primary
        let only_media = vec![parsed[1].clone()];
        let out = normalize_enclosures(id, &only_media, &mut w);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_primary);
        assert_eq!(out[0].kind, EnclosureKind::Other);
        assert!(w.iter().any(|x| x.contains("not a number")));
    }

    #[test]
    fn markup_only_title_falls_back() {
        let enclosure = ParsedEnclosure {
            url: "https://cdn.example/dir/final-episode.mp3".into(),
            mime_type: Some("audio/mpeg".into()),
            length: Some("1234".into()),
            origin: EnclosureOrigin::Enclosure,
            bitrate: None,
            height: None,
            codecs: None,
            lang: None,
            title: None,
            default: false,
            integrity_type: None,
            integrity_value: None,
            sources: vec![],
        };
        for (title, itunes, enclosures, shown) in [
            ("<br>", None, vec![enclosure.clone()], "final-episode.mp3"),
            ("<b></b>", None, vec![], "Untitled episode"),
            (
                "<br>",
                Some("Episode 5"),
                vec![enclosure.clone()],
                "Episode 5",
            ),
        ] {
            let item = ParsedItem {
                title: Some(title.into()),
                itunes_title: itunes.map(str::to_owned),
                pub_date: Some("Mon, 01 Sep 2025 12:00:00 +0000".into()),
                enclosures,
                ..ParsedItem::default()
            };
            let n = normalize_item(&item, EpisodeId::new(), NOW);
            assert_eq!(n.title, shown, "{title}");
            assert!(n.warnings.iter().any(|w| w.contains("no text")), "{title}");
            // The fingerprint is the one an empty title always had, so a
            // stored identity key does not move with the fix.
            assert_eq!(n.identity_title.as_deref(), Some(""), "{title}");
            let signals = crate::identity::signals(&item, &n);
            assert_eq!(
                signals.fingerprint,
                Some(crate::identity::fingerprint(
                    "",
                    n.published.value,
                    n.enclosures.first().and_then(|e| e.length_bytes),
                )),
                "{title}"
            );
        }
    }

    #[test]
    fn item_without_title_or_date_still_normalizes() {
        let item = ParsedItem {
            enclosures: vec![ParsedEnclosure {
                url: "https://cdn.example/dir/final-episode.mp3".into(),
                mime_type: Some("audio/mpeg".into()),
                length: None,
                origin: EnclosureOrigin::Enclosure,
                bitrate: None,
                height: None,
                codecs: None,
                lang: None,
                title: None,
                default: false,
                integrity_type: None,
                integrity_value: None,
                sources: vec![],
            }],
            duration: Some("about".into()),
            description: Some("<p>Hi <b>there</b></p>".into()),
            ..ParsedItem::default()
        };
        let n = normalize_item(&item, EpisodeId::new(), NOW);
        assert_eq!(n.title, "final-episode.mp3");
        assert_eq!(n.identity_title, None);
        assert_eq!(n.published.quality, DateQuality::Invalid);
        assert!(n.published.value.is_none());
        assert_eq!(n.description_text.as_deref(), Some("Hi there"));
        assert_eq!(n.duration_secs, None);
        assert_eq!(n.duration_raw.as_deref(), Some("about"));
        assert!(n.warnings.iter().any(|w| w.contains("no title")));
        assert!(
            n.warnings
                .iter()
                .any(|w| w.contains("unparseable duration"))
        );
    }

    #[test]
    fn channel_normalization_keeps_publisher_when_distinct() {
        let ch = ParsedChannel {
            title: Some("  The   Show ".into()),
            author: Some("Host".into()),
            publisher: Some("Host".into()),
            description: Some("<p>Desc</p>".into()),
            link: Some("https://show.example".into()),
            explicit: Some("yes".into()),
            locked: Some("no".into()),
            categories: vec!["Technology".into(), "Technology / Tech News".into()],
            ..ParsedChannel::default()
        };
        let n = normalize_channel(&ch);
        assert_eq!(n.title, "The Show");
        assert_eq!(n.sort_title, "show");
        assert_eq!(n.publisher, None, "publisher equal to author is dropped");
        assert_eq!(n.description_text.as_deref(), Some("Desc"));
        assert_eq!(n.explicit, Some(true));
        assert_eq!(n.locked, Some(false));
        assert_eq!(n.website.unwrap().as_str(), "https://show.example/");
        let empty = normalize_channel(&ParsedChannel::default());
        assert_eq!(empty.title, "Untitled podcast");
        assert!(!empty.warnings.is_empty());
    }
}
