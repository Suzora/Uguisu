//! Archive engine of Uguisu.
//!
//! The flow is: feed URL → raw snapshot on disk → parsed episodes in the index →
//! media files on disk (hashed) → re-published RSS feed.

pub mod archive;
pub mod db;
pub mod feed;
pub mod http;
pub mod model;
pub mod opml;
pub mod reindex;
pub mod rss;
pub mod search;
pub mod service;
pub mod util;

pub use service::{Config, Uguisu};
