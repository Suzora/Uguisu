//! Feed parsing.
//!
//! Two entry points share one hardened XML layer:
//!
//! - [`probe`]: a streaming inspection that answers "is this a
//!   podcast feed, and what is its identity?" without building items.
//! - [`parse`] (ADR 0008): the full parser for RSS 2.0, Atom and
//!   RSS 1.0/RDF with the iTunes and Podcasting 2.0 namespaces, producing
//!   [`ParsedFeed`] with items as written, per-item error isolation and
//!   typed truncation. [`normalize`] and [`identity`] turn parsed items into
//!   episodes.
//!
//! [`opml`] reads and writes subscription lists with the same hardening.
//!
//! Hardening: no entity expansion (references are resolved only for the
//! predefined XML entities and numeric character references), nesting
//! depth, size and count limits (`FeedLimits`), bounded text capture, and
//! lossy handling of bad UTF-8 with warnings instead of failures.

mod dates;
pub mod identity;
pub mod normalize;
pub mod opml;
pub mod parser;
mod probe;
mod text;

pub use dates::parse_date;
pub use parser::{
    EnclosureOrigin, MalformedItem, ParseError, ParseStats, ParsedChannel, ParsedEnclosure,
    ParsedFeed, ParsedItem, ParsedValue, parse,
};
pub use probe::{FeedKind, FeedProbe, ProbeError, probe};
pub use text::{DecodedText, decode_bytes};
