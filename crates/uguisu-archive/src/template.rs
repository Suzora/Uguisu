//! The path template language of ADR 0009 (grammar in ADR 0022).
//!
//! ```text
//! template := part ('/' part)*
//! part     := element*
//! element  := literal | variable | '[' element* ']'
//! variable := '{' name (':' format)? ('|' filter (':' arg)?)* '}'
//! ```
//!
//! A variable can never introduce a path separator: its value is
//! sanitized per segment before it is placed. A bracketed group renders
//! only when every variable inside it has a value, which is how
//! `[S{episode.season|pad:2}E{episode.number|pad:2} - ]` disappears for a
//! podcast without seasons instead of leaving `SE - ` behind.
//!
//! Rendering is a pure function: it reads a context, never the filesystem.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uguisu_core::archive::{ArchiveErrorKind, PathProfile};
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_core::model::{Episode, Podcast};

use crate::path::{PathError, RelativePath};
use crate::sanitize::{self, MAX_PATH_CHARS};

/// Why a template cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    /// A `{` was never closed, or a `]` has no `[`.
    #[error("template syntax at position {position}: {detail}")]
    Syntax {
        /// Byte offset in the template.
        position: usize,
        /// What was wrong.
        detail: String,
    },
    /// The template names a variable that does not exist.
    #[error("unknown variable `{0}`")]
    UnknownVariable(String),
    /// The template names a filter that does not exist.
    #[error("unknown filter `{0}`")]
    UnknownFilter(String),
    /// A filter was given an argument it cannot use.
    #[error("filter `{filter}`: {detail}")]
    BadFilterArgument {
        /// The filter.
        filter: String,
        /// What was wrong.
        detail: String,
    },
    /// A date format string could not be parsed.
    #[error("date format `{0}` is not supported")]
    BadDateFormat(String),
    /// The rendered path could not be used at all.
    #[error("rendered path is unusable: {0}")]
    Path(#[from] PathError),
}

impl TemplateError {
    /// How the error is classified for the API and the CLI.
    #[must_use]
    pub const fn kind(&self) -> ArchiveErrorKind {
        match self {
            Self::Path(_) => ArchiveErrorKind::PathInvalid,
            _ => ArchiveErrorKind::TemplateInvalid,
        }
    }
}

/// One filter applied to a value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Filter {
    Lower,
    Upper,
    Slug,
    Ascii,
    Pad(usize),
    Truncate(usize),
    Default(String),
}

impl Filter {
    fn parse(name: &str, arg: Option<&str>) -> Result<Self, TemplateError> {
        let number = |what: &str| -> Result<usize, TemplateError> {
            arg.ok_or_else(|| TemplateError::BadFilterArgument {
                filter: what.to_owned(),
                detail: "needs a number, e.g. `|pad:2`".to_owned(),
            })?
            .parse()
            .map_err(|_| TemplateError::BadFilterArgument {
                filter: what.to_owned(),
                detail: format!("`{}` is not a number", arg.unwrap_or_default()),
            })
        };
        Ok(match name {
            "lower" => Self::Lower,
            "upper" => Self::Upper,
            "slug" => Self::Slug,
            "ascii" => Self::Ascii,
            "pad" => Self::Pad(number("pad")?),
            "truncate" => Self::Truncate(number("truncate")?),
            "default" => {
                Self::Default(arg.map(|a| a.trim_matches('"').to_owned()).ok_or_else(|| {
                    TemplateError::BadFilterArgument {
                        filter: "default".to_owned(),
                        detail: "needs a value, e.g. `|default:\"Unknown\"`".to_owned(),
                    }
                })?)
            }
            other => return Err(TemplateError::UnknownFilter(other.to_owned())),
        })
    }

    fn apply(&self, value: String) -> String {
        match self {
            Self::Lower => value.to_lowercase(),
            Self::Upper => value.to_uppercase(),
            Self::Ascii => deunicode::deunicode(&value),
            Self::Slug => slug(&value),
            Self::Pad(width) => {
                if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
                    value
                } else {
                    format!("{value:0>width$}", width = *width)
                }
            }
            Self::Truncate(max) => sanitize::truncate_chars(&value, *max),
            Self::Default(fallback) => {
                if value.is_empty() {
                    fallback.clone()
                } else {
                    value
                }
            }
        }
    }

    /// Whether the filter can turn "no value" into a value, which decides
    /// whether an optional group still counts as satisfied.
    const fn supplies_value(&self) -> bool {
        matches!(self, Self::Default(_))
    }
}

fn slug(value: &str) -> String {
    let ascii = deunicode::deunicode(value).to_lowercase();
    let mut out = String::with_capacity(ascii.len());
    let mut dash = false;
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// One piece of a template part.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Element {
    Literal(String),
    Variable {
        name: String,
        format: Option<String>,
        filters: Vec<Filter>,
    },
    Optional(Vec<Element>),
}

/// A parsed, reusable template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Vec<Element>>,
    source: String,
}

impl Template {
    /// Parses a template, checking every variable and filter name.
    pub fn parse(source: &str) -> Result<Self, TemplateError> {
        let mut parts = Vec::new();
        let mut current = Vec::new();
        let mut literal = String::new();
        let mut stack: Vec<Vec<Element>> = Vec::new();
        let bytes: Vec<char> = source.chars().collect();
        let mut i = 0;

        macro_rules! flush {
            () => {
                if !literal.is_empty() {
                    let target = stack.last_mut().unwrap_or(&mut current);
                    target.push(Element::Literal(std::mem::take(&mut literal)));
                }
            };
        }

        while i < bytes.len() {
            match bytes[i] {
                '/' if stack.is_empty() => {
                    flush!();
                    parts.push(std::mem::take(&mut current));
                    i += 1;
                }
                '{' => {
                    flush!();
                    let end = bytes[i..].iter().position(|c| *c == '}').ok_or_else(|| {
                        TemplateError::Syntax {
                            position: i,
                            detail: "`{` is never closed".to_owned(),
                        }
                    })? + i;
                    let inner: String = bytes[i + 1..end].iter().collect();
                    let element = parse_variable(&inner, i)?;
                    let target = stack.last_mut().unwrap_or(&mut current);
                    target.push(element);
                    i = end + 1;
                }
                '}' => {
                    return Err(TemplateError::Syntax {
                        position: i,
                        detail: "`}` without `{`".to_owned(),
                    });
                }
                '[' => {
                    flush!();
                    stack.push(Vec::new());
                    i += 1;
                }
                ']' => {
                    flush!();
                    let group = stack.pop().ok_or_else(|| TemplateError::Syntax {
                        position: i,
                        detail: "`]` without `[`".to_owned(),
                    })?;
                    let target = stack.last_mut().unwrap_or(&mut current);
                    target.push(Element::Optional(group));
                    i += 1;
                }
                c => {
                    literal.push(c);
                    i += 1;
                }
            }
        }
        if !stack.is_empty() {
            return Err(TemplateError::Syntax {
                position: source.len(),
                detail: "`[` is never closed".to_owned(),
            });
        }
        flush!();
        parts.push(current);
        // Empty parts (a leading, trailing or doubled `/`) are dropped, so a
        // template can never produce an absolute or an empty component.
        let parts: Vec<Vec<Element>> = parts.into_iter().filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Err(TemplateError::Syntax {
                position: 0,
                detail: "template renders nothing".to_owned(),
            });
        }
        Ok(Self {
            parts,
            source: source.to_owned(),
        })
    }

    /// The template as it was written.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// How many path components the template produces at most.
    #[must_use]
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    /// Renders the template for one episode.
    ///
    /// The last part becomes the file name, every other part a directory.
    /// Parts that render empty are dropped, so a missing value never leaves
    /// an empty directory, and the file name always ends in the context's
    /// extension: a `.{extension}` the template wrote is recognized and not
    /// repeated, and a template that omitted it still gets one.
    ///
    /// The result is at most [`MAX_PATH_CHARS`] characters; if the values
    /// are longer, directory components are shortened deterministically
    /// while the file name is left intact.
    pub fn render(
        &self,
        ctx: &Context<'_>,
        profile: PathProfile,
    ) -> Result<RelativePath, TemplateError> {
        let mut components: Vec<String> = Vec::with_capacity(self.parts.len());
        let last = self.parts.len() - 1;
        for (index, part) in self.parts.iter().enumerate() {
            let rendered = render_elements(part, ctx)?;
            if index == last {
                // The file name. The template writes the extension itself
                // (`… .{extension}`), but the renderer owns it: it is split
                // off here and appended again after the stem has been
                // sanitized, truncated and given its collision suffix. So it
                // is never doubled, never truncated away, and present even in
                // a template that forgot it.
                let text = rendered.trim();
                let stem_text = strip_extension(text, ctx.extension);
                let stem = if stem_text.chars().any(is_meaningful) {
                    stem_text
                } else {
                    // Nothing but punctuation survived (an untitled episode
                    // in a `{date} - {title}` template): the episode id keeps
                    // the file addressable and unique.
                    ctx.episode_id_str.as_str()
                };
                components.push(sanitize::file_name(
                    stem,
                    ctx.extension,
                    ctx.suffix.as_deref(),
                    profile,
                ));
            } else {
                let dir = sanitize::segment(&rendered, profile);
                if !dir.is_empty() {
                    components.push(dir);
                }
            }
        }
        let path = RelativePath::from_components(components)?;
        if path.char_len() <= MAX_PATH_CHARS {
            return Ok(path);
        }
        // Too long overall: shorten directory components from the deepest
        // one, keeping the file name intact. Deterministic, so the same
        // episode always lands in the same place.
        let mut parts: Vec<String> = path.components().map(str::to_owned).collect();
        let file = parts.pop().unwrap_or_default();
        while !parts.is_empty() {
            let current: usize =
                parts.iter().map(|p| p.chars().count() + 1).sum::<usize>() + file.chars().count();
            if current <= MAX_PATH_CHARS {
                break;
            }
            let over = current - MAX_PATH_CHARS;
            let longest = parts
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| p.chars().count())
                .map_or(0, |(i, _)| i);
            let len = parts[longest].chars().count();
            if len <= 8 {
                parts.remove(longest);
            } else {
                let keep = len.saturating_sub(over).max(8);
                // Sanitized again so the cut does not end in what the
                // profile trims, a dot on Windows: that name would not be
                // the one stored.
                parts[longest] =
                    sanitize::segment(&sanitize::truncate_chars(&parts[longest], keep), profile);
            }
        }
        parts.push(file);
        Ok(RelativePath::from_components(parts)?)
    }
}

/// Whether a character carries meaning, as opposed to being filler a
/// template left behind (`" - "` for an episode with neither date nor
/// title). Letters, digits and symbols count; ASCII punctuation and
/// whitespace do not.
fn is_meaningful(c: char) -> bool {
    c.is_alphanumeric() || !(c.is_whitespace() || c.is_ascii_punctuation())
}

/// Removes a trailing `.<extension>` the template rendered, so the renderer
/// can append exactly one. Comparison ignores ASCII case, because that is
/// the only way file extensions differ.
fn strip_extension<'a>(text: &'a str, extension: &str) -> &'a str {
    if extension.is_empty() {
        return text;
    }
    let dotted = format!(".{extension}");
    let Some(start) = text.len().checked_sub(dotted.len()) else {
        return text;
    };
    match text.get(start..) {
        Some(tail) if tail.eq_ignore_ascii_case(&dotted) => &text[..start],
        _ => text,
    }
}

fn parse_variable(inner: &str, position: usize) -> Result<Element, TemplateError> {
    let mut pieces = inner.split('|');
    let head = pieces.next().unwrap_or_default().trim();
    if head.is_empty() {
        return Err(TemplateError::Syntax {
            position,
            detail: "`{}` names no variable".to_owned(),
        });
    }
    // `{episode.published:%Y-%m}`: the part after the first `:` is a date
    // format, which only the date variables accept.
    let (name, format) = match head.split_once(':') {
        Some((n, f)) => (n.trim().to_owned(), Some(f.trim().to_owned())),
        None => (head.to_owned(), None),
    };
    if !is_known_variable(&name) {
        return Err(TemplateError::UnknownVariable(name));
    }
    if format.is_some() && name != "episode.published" {
        return Err(TemplateError::BadDateFormat(format!(
            "`{name}` takes no format; only `episode.published` does"
        )));
    }
    let mut filters = Vec::new();
    for piece in pieces {
        let piece = piece.trim();
        let (fname, arg) = match piece.split_once(':') {
            Some((f, a)) => (f.trim(), Some(a.trim())),
            None => (piece, None),
        };
        filters.push(Filter::parse(fname, arg)?);
    }
    Ok(Element::Variable {
        name,
        format,
        filters,
    })
}

/// Every variable the language knows (ADR 0009 plus the two identifiers
/// the archive engine needs for deterministic fallbacks).
pub const VARIABLES: [&str; 17] = [
    "podcast.title",
    "podcast.author",
    "podcast.id",
    "episode.title",
    "episode.id",
    "episode.guid",
    "episode.identity",
    "episode.date",
    "episode.year",
    "episode.month",
    "episode.day",
    "episode.season",
    "episode.number",
    "episode.type",
    "episode.duration",
    "episode.published",
    "extension",
];

fn is_known_variable(name: &str) -> bool {
    VARIABLES.contains(&name)
}

/// What a rendered element produced, and whether it had a value at all.
struct Rendered {
    text: String,
    had_value: bool,
}

fn render_elements(elements: &[Element], ctx: &Context<'_>) -> Result<String, TemplateError> {
    let mut out = String::new();
    for element in elements {
        let r = render_element(element, ctx)?;
        out.push_str(&r.text);
    }
    Ok(out)
}

fn render_element(element: &Element, ctx: &Context<'_>) -> Result<Rendered, TemplateError> {
    match element {
        Element::Literal(text) => Ok(Rendered {
            text: text.clone(),
            had_value: true,
        }),
        Element::Variable {
            name,
            format,
            filters,
        } => {
            let raw = ctx.value(name, format.as_deref())?;
            let had_value = raw.is_some() || filters.iter().any(Filter::supplies_value);
            let mut value = raw.unwrap_or_default();
            for f in filters {
                value = f.apply(value);
            }
            Ok(Rendered {
                text: value,
                had_value,
            })
        }
        Element::Optional(inner) => {
            let mut text = String::new();
            let mut all_present = true;
            let mut saw_variable = false;
            for e in inner {
                if matches!(e, Element::Variable { .. } | Element::Optional(_)) {
                    saw_variable = true;
                }
                let r = render_element(e, ctx)?;
                if matches!(e, Element::Variable { .. } | Element::Optional(_)) && !r.had_value {
                    all_present = false;
                }
                text.push_str(&r.text);
            }
            let keep = all_present && (saw_variable || !text.is_empty());
            Ok(Rendered {
                text: if keep { text } else { String::new() },
                had_value: keep,
            })
        }
    }
}

/// Everything a template may read. Built once per episode; holds no
/// filesystem state, so rendering is pure and cheap.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    podcast_title: &'a str,
    podcast_author: Option<&'a str>,
    podcast_id_str: String,
    episode_title: &'a str,
    episode_id_str: String,
    episode_guid: Option<&'a str>,
    episode_identity: String,
    published: Option<OffsetDateTime>,
    season: Option<u32>,
    number: Option<u32>,
    episode_type: Option<&'a str>,
    duration_secs: Option<u32>,
    /// Extension without the dot; appended to the file name by the renderer.
    extension: &'a str,
    /// Disambiguating suffix appended to the file stem, when one is needed.
    suffix: Option<String>,
}

impl<'a> Context<'a> {
    /// Builds a context from the stored podcast and episode.
    #[must_use]
    pub fn new(podcast: &'a Podcast, episode: &'a Episode, extension: &'a str) -> Self {
        Self {
            podcast_title: &podcast.title,
            podcast_author: podcast.author.as_deref(),
            podcast_id_str: podcast.id.to_string(),
            episode_title: &episode.title,
            episode_id_str: episode.id.to_string(),
            episode_guid: episode.guid.as_deref(),
            episode_identity: identity_short(&episode.identity.key),
            published: episode.published_at,
            season: episode.season,
            number: episode.episode_number,
            episode_type: episode.episode_type.as_deref(),
            duration_secs: episode.duration_secs,
            extension,
            suffix: None,
        }
    }

    /// The identifiers, for a caller that needs the deterministic fallbacks.
    #[must_use]
    pub fn ids(&self) -> (&str, &str) {
        (&self.podcast_id_str, &self.episode_id_str)
    }

    /// Adds a disambiguating suffix to the file stem (collision handling).
    #[must_use]
    pub fn with_suffix(mut self, suffix: Option<String>) -> Self {
        self.suffix = suffix;
        self
    }

    /// A context for previewing a template without stored rows.
    #[must_use]
    pub fn synthetic(
        podcast_title: &'a str,
        episode_title: &'a str,
        extension: &'a str,
        podcast_id: PodcastId,
        episode_id: EpisodeId,
        published: Option<OffsetDateTime>,
    ) -> Self {
        Self {
            podcast_title,
            podcast_author: None,
            podcast_id_str: podcast_id.to_string(),
            episode_title,
            episode_id_str: episode_id.to_string(),
            episode_guid: None,
            episode_identity: identity_short(&episode_id.to_string()),
            published,
            season: None,
            number: None,
            episode_type: None,
            duration_secs: None,
            extension,
            suffix: None,
        }
    }

    fn value(&self, name: &str, format: Option<&str>) -> Result<Option<String>, TemplateError> {
        let text = |s: &str| -> Option<String> {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_owned())
        };
        Ok(match name {
            "podcast.title" => text(self.podcast_title),
            "podcast.author" => self.podcast_author.and_then(text),
            "podcast.id" => Some(self.podcast_id_str.clone()),
            "episode.title" => text(self.episode_title),
            "episode.id" => Some(self.episode_id_str.clone()),
            "episode.guid" => self.episode_guid.and_then(text),
            "episode.identity" => Some(self.episode_identity.clone()),
            "episode.date" => self
                .published
                .map(|d| format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())),
            "episode.year" => self.published.map(|d| format!("{:04}", d.year())),
            "episode.month" => self
                .published
                .map(|d| format!("{:02}", u8::from(d.month()))),
            "episode.day" => self.published.map(|d| format!("{:02}", d.day())),
            "episode.season" => self.season.map(|s| s.to_string()),
            "episode.number" => self.number.map(|n| n.to_string()),
            "episode.type" => self.episode_type.and_then(text),
            "episode.duration" => self.duration_secs.map(|d| {
                let (h, m, s) = (d / 3600, (d / 60) % 60, d % 60);
                let mut out = String::new();
                let _ = write!(out, "{h:02}h{m:02}m{s:02}s");
                out
            }),
            "episode.published" => match (self.published, format) {
                (Some(d), Some(f)) => Some(format_date(d, f)?),
                (Some(d), None) => Some(format!(
                    "{:04}-{:02}-{:02}",
                    d.year(),
                    u8::from(d.month()),
                    d.day()
                )),
                (None, _) => None,
            },
            "extension" => text(self.extension),
            other => return Err(TemplateError::UnknownVariable(other.to_owned())),
        })
    }
}

/// The strftime subset ADR 0009 allows: date and time fields only, so a
/// format can never introduce a separator or read anything else.
fn format_date(d: OffsetDateTime, format: &str) -> Result<String, TemplateError> {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => {
                let _ = write!(out, "{:04}", d.year());
            }
            Some('m') => {
                let _ = write!(out, "{:02}", u8::from(d.month()));
            }
            Some('d') => {
                let _ = write!(out, "{:02}", d.day());
            }
            Some('H') => {
                let _ = write!(out, "{:02}", d.hour());
            }
            Some('M') => {
                let _ = write!(out, "{:02}", d.minute());
            }
            Some('S') => {
                let _ = write!(out, "{:02}", d.second());
            }
            Some('%') => out.push('%'),
            other => {
                return Err(TemplateError::BadDateFormat(format!(
                    "%{}",
                    other.unwrap_or(' ')
                )));
            }
        }
    }
    Ok(out)
}

/// Short, stable fingerprint of an episode's identity, for templates that
/// want a compact unique piece.
#[must_use]
pub fn identity_short(identity_key: &str) -> String {
    let digest = Sha256::digest(identity_key.as_bytes());
    hex::encode(&digest[..4])
}

#[cfg(test)]
mod tests {
    // The rendered extension is always the lowercase value the context
    // carries, so an exact suffix assertion is the point.
    #![allow(clippy::case_sensitive_file_extension_comparisons)]
    #![allow(clippy::unwrap_used)]

    use time::Month;
    use uguisu_core::ids::{EpisodeId, PodcastId};

    use super::*;

    fn at(y: i32, m: Month, d: u8) -> OffsetDateTime {
        time::Date::from_calendar_date(y, m, d)
            .unwrap()
            .with_hms(13, 5, 9)
            .unwrap()
            .assume_utc()
    }

    fn ctx<'a>(title: &'a str, ep: &'a str, ext: &'a str) -> Context<'a> {
        Context::synthetic(
            title,
            ep,
            ext,
            PodcastId::new(),
            EpisodeId::new(),
            Some(at(2024, Month::January, 15)),
        )
    }

    fn render(template: &str, c: &Context<'_>) -> String {
        Template::parse(template)
            .unwrap()
            .render(c, PathProfile::Portable)
            .unwrap()
            .as_str()
            .to_owned()
    }

    #[test]
    fn the_default_template_produces_a_readable_path() {
        let c = ctx("Darknet Diaries", "The Pizza Problem", "mp3");
        assert_eq!(
            render(
                "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}",
                &c
            ),
            "Darknet Diaries/2024/2024-01-15 - The Pizza Problem.mp3"
        );
    }

    #[test]
    fn every_variable_renders() {
        let p = Podcast {
            author: Some("Jack".into()),
            ..uguisu_storage_sample::podcast("Show")
        };
        let mut e = uguisu_storage_sample::episode(p.id, "Episode One");
        e.published_at = Some(at(2023, Month::March, 7));
        e.season = Some(2);
        e.episode_number = Some(9);
        e.episode_type = Some("full".into());
        e.duration_secs = Some(3725);
        e.guid = Some("guid-1".into());
        let c = Context::new(&p, &e, "m4a");

        assert_eq!(render("{podcast.title}/x.{extension}", &c), "Show/x.m4a");
        assert_eq!(render("{podcast.author}/x.{extension}", &c), "Jack/x.m4a");
        assert_eq!(
            render("{podcast.id}/x.{extension}", &c),
            format!("{}/x.m4a", p.id)
        );
        assert_eq!(render("{episode.title}.{extension}", &c), "Episode One.m4a");
        assert_eq!(
            render("{episode.id}.{extension}", &c),
            format!("{}.m4a", e.id)
        );
        assert_eq!(render("{episode.guid}.{extension}", &c), "guid-1.m4a");
        assert_eq!(render("{episode.date}.{extension}", &c), "2023-03-07.m4a");
        assert_eq!(render("{episode.year}.{extension}", &c), "2023.m4a");
        assert_eq!(render("{episode.month}.{extension}", &c), "03.m4a");
        assert_eq!(render("{episode.day}.{extension}", &c), "07.m4a");
        assert_eq!(render("{episode.season}.{extension}", &c), "2.m4a");
        assert_eq!(render("{episode.number}.{extension}", &c), "9.m4a");
        assert_eq!(render("{episode.type}.{extension}", &c), "full.m4a");
        assert_eq!(
            render("{episode.duration}.{extension}", &c),
            "01h02m05s.m4a"
        );
        assert_eq!(
            render("{episode.published:%Y%m%d-%H%M}.{extension}", &c),
            "20230307-1305.m4a"
        );
        let identity = render("{episode.identity}.{extension}", &c);
        assert_eq!(identity.len(), "xxxxxxxx.m4a".len());
        assert_eq!(
            identity,
            render("{episode.identity}.{extension}", &c),
            "the identity fragment is stable"
        );
    }

    #[test]
    fn filters_change_the_value_not_the_shape() {
        let c = ctx("Über Grüße", "Folge Zwei", "mp3");
        assert_eq!(
            render("{podcast.title|lower}/x.{extension}", &c),
            "über grüße/x.mp3"
        );
        assert_eq!(
            render("{podcast.title|upper}/x.{extension}", &c),
            "ÜBER GRÜSSE/x.mp3"
        );
        assert_eq!(
            render("{podcast.title|ascii}/x.{extension}", &c),
            "Uber Grusse/x.mp3"
        );
        assert_eq!(
            render("{podcast.title|slug}/x.{extension}", &c),
            "uber-grusse/x.mp3"
        );
        assert_eq!(
            render("{episode.title|truncate:5}.{extension}", &c),
            "Folge.mp3"
        );
        assert_eq!(
            render("{episode.month|pad:4}.{extension}", &c),
            "0001.mp3",
            "pad works on the numeric fields"
        );
        assert_eq!(
            render("{podcast.author|default:\"Unknown\"}/x.{extension}", &c),
            "Unknown/x.mp3"
        );
    }

    #[test]
    fn optional_groups_disappear_when_empty() {
        let c = ctx("Show", "Pilot", "mp3");
        // No season and no number in the synthetic context.
        assert_eq!(
            render(
                "{podcast.title}/[S{episode.season|pad:2}E{episode.number|pad:2} - ]{episode.title}.{extension}",
                &c
            ),
            "Show/Pilot.mp3"
        );
        let p = uguisu_storage_sample::podcast("Show");
        let mut e = uguisu_storage_sample::episode(p.id, "Pilot");
        e.season = Some(1);
        e.episode_number = Some(4);
        let full = Context::new(&p, &e, "mp3");
        assert_eq!(
            Template::parse(
                "{podcast.title}/[S{episode.season|pad:2}E{episode.number|pad:2} - ]{episode.title}.{extension}"
            )
            .unwrap()
            .render(&full, PathProfile::Portable)
            .unwrap()
            .as_str(),
            "Show/S01E04 - Pilot.mp3"
        );
        // A `default` filter inside a group makes the group render again.
        assert_eq!(
            render(
                "{podcast.title}/[{episode.season|default:\"S0\"} ]{episode.title}.{extension}",
                &c
            ),
            "Show/S0 Pilot.mp3"
        );
    }

    #[test]
    fn missing_values_leave_no_holes() {
        let c = Context::synthetic("  ", "", "mp3", PodcastId::new(), EpisodeId::new(), None);
        let (_, episode_id) = c.ids();
        let episode_id = episode_id.to_owned();
        // Empty directory parts vanish; the file name falls back to the id.
        let rendered = render(
            "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}",
            &c,
        );
        assert_eq!(rendered, format!("{episode_id}.mp3"));
        assert!(!rendered.contains("//"), "{rendered}");
        assert!(!rendered.starts_with('/'), "{rendered}");
        assert!(!rendered.contains(" - ."), "{rendered}");
    }

    #[test]
    fn a_template_can_never_escape_the_archive() {
        let c = ctx("../../etc", "../passwd", "mp3");
        let rendered = render("{podcast.title}/{episode.title}.{extension}", &c);
        assert_eq!(rendered, "..-..-etc/..-passwd.mp3");
        assert!(RelativePath::parse(&rendered).is_ok());
        // A literal traversal in the template itself is dropped, because a
        // part that sanitizes to nothing is not written.
        assert_eq!(
            render("../{episode.title}.{extension}", &c),
            "..-passwd.mp3"
        );
    }

    #[test]
    fn syntax_and_name_errors_are_specific() {
        assert!(matches!(
            Template::parse("{podcast.title").unwrap_err(),
            TemplateError::Syntax { .. }
        ));
        assert!(matches!(
            Template::parse("a}b").unwrap_err(),
            TemplateError::Syntax { .. }
        ));
        assert!(matches!(
            Template::parse("[{episode.title}").unwrap_err(),
            TemplateError::Syntax { .. }
        ));
        assert!(matches!(
            Template::parse("{episode.title}]").unwrap_err(),
            TemplateError::Syntax { .. }
        ));
        assert_eq!(
            Template::parse("{episode.name}").unwrap_err(),
            TemplateError::UnknownVariable("episode.name".into())
        );
        assert_eq!(
            Template::parse("{episode.title|shout}").unwrap_err(),
            TemplateError::UnknownFilter("shout".into())
        );
        assert!(matches!(
            Template::parse("{episode.title|pad:x}").unwrap_err(),
            TemplateError::BadFilterArgument { .. }
        ));
        assert!(matches!(
            Template::parse("{episode.published:%Q}")
                .unwrap()
                .render(&ctx("a", "b", "mp3"), PathProfile::Portable)
                .unwrap_err(),
            TemplateError::BadDateFormat(_)
        ));
        assert!(matches!(
            Template::parse("{episode.title:%Y}").unwrap_err(),
            TemplateError::BadDateFormat(_)
        ));
        assert!(Template::parse("").is_err());
        assert!(Template::parse("///").is_err());
    }

    #[test]
    fn shortened_folders_stay_sanitized() {
        let podcast = "a".repeat(73) + "." + &"b".repeat(46);
        let episode = "e".repeat(300);
        let c = ctx(&podcast, &episode, "mp3");
        let path = render(
            "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}",
            &c,
        );
        for component in path.split('/') {
            assert_eq!(
                sanitize::segment(component, PathProfile::Portable),
                component,
                "{path}"
            );
        }
    }

    #[test]
    fn long_values_stay_within_the_limit() {
        let long_show = "L".repeat(300);
        let long_ep = "E".repeat(300);
        let c = ctx(&long_show, &long_ep, "mp3");
        let rendered = render(
            "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}",
            &c,
        );
        let path = RelativePath::parse(&rendered).unwrap();
        assert!(
            path.char_len() <= MAX_PATH_CHARS,
            "{} chars: {rendered}",
            path.char_len()
        );
        assert!(rendered.ends_with(".mp3"));
        for component in path.components() {
            assert!(component.chars().count() <= sanitize::MAX_SEGMENT_CHARS);
            assert!(!component.is_empty());
        }
        // Deterministic: the same input renders the same path.
        assert_eq!(
            rendered,
            render(
                "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}",
                &c
            )
        );
    }

    #[test]
    fn a_suffix_disambiguates_without_touching_the_extension() {
        let c = ctx("Show", "Same Title", "mp3").with_suffix(Some("[a1b2c3]".into()));
        assert_eq!(
            render("{podcast.title}/{episode.title}.{extension}", &c),
            "Show/Same Title [a1b2c3].mp3"
        );
    }

    /// Minimal stand-ins so the template tests do not depend on storage.
    mod uguisu_storage_sample {
        use time::OffsetDateTime;
        use uguisu_core::ids::{EpisodeId, PodcastId};
        use uguisu_core::model::{
            ArchiveState, DateQuality, Episode, EpisodeExtras, EpisodeIdentity, FeedKind,
            IdentitySource, Podcast, PodcastStatus,
        };

        pub fn podcast(title: &str) -> Podcast {
            let now = OffsetDateTime::now_utc();
            Podcast {
                id: PodcastId::new(),
                title: title.to_owned(),
                sort_title: title.to_lowercase(),
                subtitle: None,
                author: None,
                publisher: None,
                owner_name: None,
                owner_email: None,
                description_html: None,
                description_text: None,
                website: None,
                language: None,
                categories: Vec::new(),
                explicit: None,
                artwork_url: None,
                copyright: None,
                podcast_guid: None,
                feed_kind: FeedKind::Rss2,
                status: PodcastStatus::Active,
                directory_name: None,
                metadata_hash: "h".into(),
                last_refresh_at: None,
                next_refresh_at: None,
                refresh_interval_secs: None,
                last_error: None,
                created_at: now,
                updated_at: now,
            }
        }

        pub fn episode(podcast_id: PodcastId, title: &str) -> Episode {
            let now = OffsetDateTime::now_utc();
            Episode {
                id: EpisodeId::new(),
                podcast_id,
                guid: None,
                guid_is_permalink: None,
                identity: EpisodeIdentity {
                    key: format!("guid:{title}"),
                    source: IdentitySource::Guid,
                    guid_key: None,
                    enclosure_key: None,
                    fingerprint_key: None,
                    reason: "test".into(),
                },
                title: title.to_owned(),
                subtitle: None,
                sort_title: title.to_lowercase(),
                description_html: None,
                description_text: None,
                link: None,
                published_at: None,
                published_at_raw: None,
                published_at_quality: DateQuality::Invalid,
                updated_at_source: None,
                sort_at: now,
                duration_secs: None,
                duration_raw: None,
                season: None,
                episode_number: None,
                episode_type: None,
                explicit: None,
                artwork_url: None,
                author: None,
                content_hash: "c".into(),
                malformed: false,
                malformed_reason: None,
                duplicate_of_episode_id: None,
                duplicate_reasons: Vec::new(),
                archive_state: ArchiveState::Expected,
                skip_reason: None,
                missing_streak: 0,
                removed_from_feed_at: None,
                first_seen_at: now,
                last_seen_in_feed_at: now,
                source_metadata: None,
                enclosures: Vec::new(),
                extras: EpisodeExtras::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }
}
