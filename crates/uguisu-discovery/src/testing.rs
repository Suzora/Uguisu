//! Fixture format and wiremock helpers shared by provider tests and the
//! fixture recorder (`tools/record-fixtures`).
//!
//! A fixture is one recorded or spec-derived HTTP exchange. The shape is the
//! on-disk format, so it lives here rather than only in a document:
//!
//! ```json
//! {
//!   "schema": 1,
//!   "origin": "recorded" | "spec-example",
//!   "recorded_at": "2026-09-17T10:00:00Z",
//!   "source": "https://… or a spec reference",
//!   "provider": "apple",
//!   "case": "search_darknet",
//!   "request": { "method": "GET", "path": "/search", "query": { "term": "darknet diaries" } },
//!   "response": { "status": 200, "headers": { "content-type": "…" }, "body": <json or string> }
//! }
//! ```
//!
//! A recorded fixture never contains `Authorization`, `X-Auth-*` or credential
//! query parameters: the recorder strips them and the loader refuses to mount
//! one that still has them.
//!
//! [`Fixture::load`] reads `tests/fixtures/discovery/spec/`, which documents
//! the published shape; [`Fixture::load_live`] reads
//! `tests/fixtures/discovery/live/`, which proves live behaviour on the
//! recording date. Keeping them apart is what makes schema drift a diff.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where a fixture's data came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FixtureOrigin {
    /// Captured from the live provider by the recorder.
    Recorded,
    /// Assembled from the provider's published documentation or OpenAPI examples.
    SpecExample,
}

/// The request half of a fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FixtureRequest {
    /// HTTP method.
    pub method: String,
    /// Path (no host, no query).
    pub path: String,
    /// Query parameters that must match (others are ignored).
    #[serde(default)]
    pub query: BTreeMap<String, String>,
}

/// The response half of a fixture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FixtureResponse {
    /// Status code.
    pub status: u16,
    /// Headers to replay (lowercase names).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Body: JSON value or raw string.
    pub body: serde_json::Value,
}

/// One recorded exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Fixture {
    /// Format version.
    pub schema: u32,
    /// Provenance.
    pub origin: FixtureOrigin,
    /// When it was recorded / written.
    pub recorded_at: String,
    /// URL or document the data came from.
    pub source: String,
    /// Provider id.
    pub provider: String,
    /// Case name (file stem).
    pub case: String,
    /// Request.
    pub request: FixtureRequest,
    /// Response.
    pub response: FixtureResponse,
    /// Free-form notes (e.g. which fields were truncated).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Errors when loading or mounting fixtures.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// I/O.
    #[error("cannot read fixture {path}: {source}")]
    Io {
        /// File.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// JSON.
    #[error("cannot parse fixture {path}: {source}")]
    Json {
        /// File.
        path: PathBuf,
        /// Cause.
        source: serde_json::Error,
    },
    /// The fixture still carries credentials.
    #[error("fixture {0} contains credentials ({1})")]
    Unsanitized(String, String),
    /// Unsupported schema version.
    #[error("fixture {0} has unsupported schema {1}")]
    Schema(String, u32),
}

/// Names that must never appear in a fixture.
pub const SENSITIVE_HEADERS: [&str; 5] = [
    "authorization",
    "x-auth-key",
    "x-auth-date",
    "cookie",
    "set-cookie",
];
/// Query parameters that must never appear in a fixture.
pub const SENSITIVE_QUERY_KEYS: [&str; 6] = ["key", "secret", "token", "api_key", "apikey", "auth"];

impl Fixture {
    /// Root of the spec-example fixture tree (`tests/fixtures/discovery/spec`).
    pub fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery/spec")
    }

    /// Root of the recorded fixture tree (`tests/fixtures/discovery/live`).
    pub fn live_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery/live")
    }

    /// Loads the spec-example fixture `spec/<provider>/<case>.json`.
    pub fn load(provider: &str, case: &str) -> Result<Self, FixtureError> {
        Self::load_path(&Self::root().join(provider).join(format!("{case}.json")))
    }

    /// Loads the recorded fixture `live/<provider>/<case>.json`; `provider`
    /// is `web/<host>` for website and feed captures.
    pub fn load_live(provider: &str, case: &str) -> Result<Self, FixtureError> {
        Self::load_path(
            &Self::live_root()
                .join(provider)
                .join(format!("{case}.json")),
        )
    }

    /// Loads a fixture file.
    pub fn load_path(path: &Path) -> Result<Self, FixtureError> {
        let text = std::fs::read_to_string(path).map_err(|source| FixtureError::Io {
            path: path.to_owned(),
            source,
        })?;
        let fixture: Self = serde_json::from_str(&text).map_err(|source| FixtureError::Json {
            path: path.to_owned(),
            source,
        })?;
        fixture.validate()?;
        Ok(fixture)
    }

    /// Checks schema and sanitization.
    pub fn validate(&self) -> Result<(), FixtureError> {
        if self.schema != 1 {
            return Err(FixtureError::Schema(self.case.clone(), self.schema));
        }
        for h in self.response.headers.keys() {
            if SENSITIVE_HEADERS.contains(&h.to_ascii_lowercase().as_str()) {
                return Err(FixtureError::Unsanitized(
                    self.case.clone(),
                    format!("header {h}"),
                ));
            }
        }
        for k in self.request.query.keys() {
            if SENSITIVE_QUERY_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                return Err(FixtureError::Unsanitized(
                    self.case.clone(),
                    format!("query {k}"),
                ));
            }
        }
        Ok(())
    }

    /// Removes credentials from headers and query parameters.
    pub fn sanitize(&mut self) {
        self.response
            .headers
            .retain(|k, _| !SENSITIVE_HEADERS.contains(&k.to_ascii_lowercase().as_str()));
        self.request
            .query
            .retain(|k, _| !SENSITIVE_QUERY_KEYS.contains(&k.to_ascii_lowercase().as_str()));
    }

    /// Serializes with stable formatting.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Body as bytes (JSON re-serialized, strings raw).
    pub fn body_bytes(&self) -> Vec<u8> {
        match &self.response.body {
            serde_json::Value::String(s) => s.clone().into_bytes(),
            other => serde_json::to_vec(other).unwrap_or_default(),
        }
    }

    /// Mounts the fixture on a wiremock server and returns the base URL.
    #[cfg(any(test, feature = "testing"))]
    pub async fn mount(&self, server: &wiremock::MockServer) {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, ResponseTemplate};
        let mut mock =
            Mock::given(method(self.request.method.as_str())).and(path(self.request.path.as_str()));
        for (k, v) in &self.request.query {
            mock = mock.and(query_param(k.as_str(), v.as_str()));
        }
        let mut template = ResponseTemplate::new(self.response.status);
        for (k, v) in &self.response.headers {
            template = template.insert_header(k.as_str(), v.as_str());
        }
        if !self.response.headers.contains_key("content-type") {
            let ct = if self.response.body.is_string() {
                "text/plain"
            } else {
                "application/json"
            };
            template = template.insert_header("content-type", ct);
        }
        template = template.set_body_bytes(self.body_bytes());
        mock.respond_with(template).mount(server).await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sample() -> Fixture {
        Fixture {
            schema: 1,
            origin: FixtureOrigin::SpecExample,
            recorded_at: "2026-09-17T00:00:00Z".into(),
            source: "unit test".into(),
            provider: "apple".into(),
            case: "sample".into(),
            request: FixtureRequest {
                method: "GET".into(),
                path: "/search".into(),
                query: BTreeMap::from([("term".to_owned(), "x".to_owned())]),
            },
            response: FixtureResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: serde_json::json!({"ok": true}),
            },
            notes: vec![],
        }
    }

    #[test]
    fn rejects_unsanitized_and_sanitizes() {
        let mut f = sample();
        f.response
            .headers
            .insert("Authorization".into(), "x".into());
        assert!(matches!(f.validate(), Err(FixtureError::Unsanitized(..))));
        f.sanitize();
        assert!(f.validate().is_ok());
        f.request.query.insert("api_key".into(), "k".into());
        assert!(f.validate().is_err());
        f.sanitize();
        assert!(f.validate().is_ok());
    }

    #[test]
    fn json_round_trip_and_body_bytes() {
        let f = sample();
        let back: Fixture = serde_json::from_str(&f.to_json()).unwrap();
        assert_eq!(back, f);
        assert_eq!(f.body_bytes(), br#"{"ok":true}"#);
        let mut s = sample();
        s.response.body = serde_json::Value::String("<rss/>".into());
        assert_eq!(s.body_bytes(), b"<rss/>");
    }

    #[tokio::test]
    async fn mounts_on_wiremock() {
        let server = wiremock::MockServer::start().await;
        sample().mount(&server).await;
        let client = uguisu_http::HttpClient::with_policy(
            uguisu_http::Profile::Discovery,
            uguisu_http::NetworkPolicy::trusted(),
        )
        .unwrap();
        let url =
            uguisu_http::Url::parse(&format!("{}/search?term=x&limit=5", server.uri())).unwrap();
        let resp = client.get(&url).await.unwrap();
        assert_eq!(resp.status.as_u16(), 200);
        assert_eq!(resp.content_type(), Some("application/json"));
        let miss = uguisu_http::Url::parse(&format!("{}/search?term=y", server.uri())).unwrap();
        assert_eq!(client.get(&miss).await.unwrap().status.as_u16(), 404);
    }
}
