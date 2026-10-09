//! Test support: the scenario media server and failure injection.
//! Available under the `testing` feature and in this crate's own tests.

pub mod media_server;

pub use media_server::{MediaServer, RecordedRequest, content_bytes, content_sha256};
