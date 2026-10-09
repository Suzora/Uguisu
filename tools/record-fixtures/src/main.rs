//! Records live provider, website and feed responses as sanitized fixtures
//! for deterministic tests (`docs/DISCOVERY.md`).
//!
//! ```text
//! record-fixtures provider apple --case search_darknet --query "Darknet Diaries"
//! record-fixtures provider podcastindex --case search_darknet --query "Darknet Diaries"   # needs UGUISU_PODCASTINDEX_KEY/SECRET
//! record-fixtures lookup apple --case lookup_darknet --reference 1296350485
//! record-fixtures lookup podcastindex --case byitunesid --reference itunes:1296350485
//! record-fixtures url https://darknetdiaries.com/ --case darknetdiaries_home
//! record-fixtures diff tests/fixtures/discovery/spec/apple/search_darknet.json tests/fixtures/discovery/live/apple/search_darknet.json
//! ```
//!
//! Recordings go to `tests/fixtures/discovery/live/` (override with `--out`);
//! the spec-example tree under `spec/` is never written by this tool, so the
//! two provenances stay separate.
//!
//! Credentials never reach a fixture: authentication headers are not
//! captured, credential-like query parameters are stripped, and the
//! loader refuses fixtures that still carry them.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use uguisu_core::config::DiscoveryConfig;
use uguisu_discovery::providers::podcastindex::PodcastIndexProvider;
use uguisu_discovery::testing::{Fixture, FixtureOrigin, FixtureRequest, FixtureResponse};
use uguisu_http::{ClientConfig, GetOptions, HeaderName, HeaderValue, HttpClient, Profile, Url};

#[derive(Debug, Parser)]
#[command(name = "record-fixtures", about)]
struct Cli {
    /// Output root (default: tests/fixtures/discovery/live).
    #[arg(long, global = true)]
    out: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Provider {
    Apple,
    Podcastindex,
    Gpoddernet,
}

impl Provider {
    const fn id(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Podcastindex => "podcastindex",
            Self::Gpoddernet => "gpoddernet",
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Record a provider search.
    Provider {
        /// Provider.
        provider: Provider,
        /// Case name (file stem).
        #[arg(long)]
        case: String,
        /// Search query.
        #[arg(long)]
        query: String,
        /// Result limit.
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Apple storefront country.
        #[arg(long, default_value = "US")]
        country: String,
    },
    /// Record a provider lookup.
    Lookup {
        /// Provider.
        provider: Provider,
        /// Case name.
        #[arg(long)]
        case: String,
        /// Reference: an id, `itunes:<id>`, `guid:<guid>` or a feed URL.
        #[arg(long)]
        reference: String,
    },
    /// Record a website or feed body.
    Url {
        /// The URL.
        url: String,
        /// Case name.
        #[arg(long)]
        case: String,
        /// Maximum bytes kept.
        #[arg(long, default_value_t = 2_000_000)]
        max_bytes: usize,
    },
    /// Compare the response keys of two fixtures.
    Diff {
        /// First fixture.
        a: PathBuf,
        /// Second fixture.
        b: PathBuf,
    },
}

/// Builds the request a provider would send (mirrors the provider modules).
fn provider_request(
    provider: Provider,
    cfg: &DiscoveryConfig,
    query: &str,
    limit: usize,
    country: &str,
) -> anyhow::Result<(Url, BTreeMap<String, String>)> {
    let (base, path, params): (&str, &str, Vec<(&str, String)>) = match provider {
        Provider::Apple => (
            &cfg.apple.base_url,
            "search",
            vec![
                ("term", query.to_owned()),
                ("country", country.to_owned()),
                ("media", "podcast".into()),
                ("entity", "podcast".into()),
                ("limit", limit.clamp(1, 200).to_string()),
            ],
        ),
        Provider::Podcastindex => (
            &cfg.podcastindex.base_url,
            "search/byterm",
            vec![
                ("q", query.to_owned()),
                ("max", limit.clamp(1, 100).to_string()),
                ("similar", "true".into()),
            ],
        ),
        Provider::Gpoddernet => (
            &cfg.gpoddernet.base_url,
            "search.json",
            vec![("q", query.to_owned()), ("scale_logo", "256".into())],
        ),
    };
    build(base, path, &params)
}

fn lookup_request(
    provider: Provider,
    cfg: &DiscoveryConfig,
    reference: &str,
) -> anyhow::Result<(Url, BTreeMap<String, String>)> {
    let itunes = reference.strip_prefix("itunes:");
    let guid = reference.strip_prefix("guid:");
    let is_url = reference.starts_with("http");
    let (base, path, params): (&str, &str, Vec<(&str, String)>) = match provider {
        Provider::Apple => (
            &cfg.apple.base_url,
            "lookup",
            vec![
                ("id", itunes.unwrap_or(reference).to_owned()),
                ("entity", "podcast".into()),
            ],
        ),
        Provider::Podcastindex => {
            let (path, key, value) = if let Some(id) = itunes {
                ("podcasts/byitunesid", "id", id.to_owned())
            } else if let Some(g) = guid {
                ("podcasts/byguid", "guid", g.to_owned())
            } else if is_url {
                ("podcasts/byfeedurl", "url", reference.to_owned())
            } else {
                ("podcasts/byfeedid", "id", reference.to_owned())
            };
            (&cfg.podcastindex.base_url, path, vec![(key, value)])
        }
        Provider::Gpoddernet => (
            &cfg.gpoddernet.base_url,
            "api/2/data/podcast.json",
            vec![("url", reference.to_owned())],
        ),
    };
    build(base, path, &params)
}

fn build(
    base: &str,
    path: &str,
    params: &[(&str, String)],
) -> anyhow::Result<(Url, BTreeMap<String, String>)> {
    let mut url =
        Url::parse(&format!("{}/{path}", base.trim_end_matches('/'))).context("base url")?;
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    let query: BTreeMap<String, String> = params
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    Ok((url, query))
}

fn auth_headers(cfg: &DiscoveryConfig) -> anyhow::Result<Vec<(HeaderName, HeaderValue)>> {
    let (Some(key), Some(secret)) = (&cfg.podcastindex.key, &cfg.podcastindex.secret) else {
        bail!("Podcast Index needs UGUISU_PODCASTINDEX_KEY and UGUISU_PODCASTINDEX_SECRET");
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let hash = PodcastIndexProvider::auth_hash(key.expose(), secret.expose(), now);
    Ok(vec![
        (
            HeaderName::from_static("x-auth-key"),
            HeaderValue::from_str(key.expose())?,
        ),
        (
            HeaderName::from_static("x-auth-date"),
            HeaderValue::from_str(&now.to_string())?,
        ),
        (
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&hash)?,
        ),
    ])
}

/// Keeps only headers worth replaying.
fn keep_headers(headers: &uguisu_http::HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for name in [
        "content-type",
        "cache-control",
        "retry-after",
        "etag",
        "last-modified",
    ] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            out.insert(name.to_owned(), v.to_owned());
        }
    }
    out
}

/// Converts a body to a JSON value (parsed JSON or a string), truncating
/// very long arrays and noting it.
fn body_value(body: &[u8], notes: &mut Vec<String>, max_bytes: usize) -> serde_json::Value {
    let bytes = if body.len() > max_bytes {
        notes.push(format!(
            "body truncated from {} to {max_bytes} bytes",
            body.len()
        ));
        &body[..max_bytes]
    } else {
        body
    };
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(mut v) => {
            truncate_arrays(&mut v, 50, notes);
            v
        }
        Err(_) => serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned()),
    }
}

fn truncate_arrays(v: &mut serde_json::Value, max: usize, notes: &mut Vec<String>) {
    match v {
        serde_json::Value::Array(a) => {
            if a.len() > max {
                notes.push(format!("array truncated from {} to {max} entries", a.len()));
                a.truncate(max);
            }
            for x in a.iter_mut() {
                truncate_arrays(x, max, notes);
            }
        }
        serde_json::Value::Object(o) => {
            for x in o.values_mut() {
                truncate_arrays(x, max, notes);
            }
        }
        _ => {}
    }
}

fn write_fixture(root: &Path, fixture: &Fixture) -> anyhow::Result<PathBuf> {
    fixture
        .validate()
        .context("fixture failed sanitization check")?;
    let dir = root.join(&fixture.provider);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", fixture.case));
    std::fs::write(&path, fixture.to_json() + "\n")?;
    Ok(path)
}

fn json_keys(v: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(o) => {
            for (k, x) in o {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                out.push(p.clone());
                json_keys(x, &p, out);
            }
        }
        serde_json::Value::Array(a) => {
            if let Some(first) = a.first() {
                json_keys(first, &format!("{prefix}[]"), out);
            }
        }
        _ => {}
    }
}

/// Keys present in one fixture body but not the other.
fn diff_keys(a: &serde_json::Value, b: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let mut ka = Vec::new();
    let mut kb = Vec::new();
    json_keys(a, "", &mut ka);
    json_keys(b, "", &mut kb);
    let only_a = ka.iter().filter(|k| !kb.contains(k)).cloned().collect();
    let only_b = kb.iter().filter(|k| !ka.contains(k)).cloned().collect();
    (only_a, only_b)
}

async fn record(
    client: &HttpClient,
    url: &Url,
    headers: Vec<(HeaderName, HeaderValue)>,
    max_bytes: usize,
) -> anyhow::Result<(FixtureResponse, Vec<String>)> {
    let resp = client
        .get_with(
            url,
            &GetOptions {
                headers,
                retry: Some(false),
                ..GetOptions::default()
            },
        )
        .await
        .with_context(|| format!("GET {url}"))?;
    let mut notes = Vec::new();
    if !resp.redirects.is_empty() {
        notes.push(format!(
            "followed {} redirect(s) to {}",
            resp.redirects.len(),
            resp.url
        ));
    }
    let body = body_value(&resp.body, &mut notes, max_bytes);
    Ok((
        FixtureResponse {
            status: resp.status.as_u16(),
            headers: keep_headers(&resp.headers),
            body,
        },
        notes,
    ))
}

fn fixture(
    provider: &str,
    case: &str,
    source: &str,
    request: FixtureRequest,
    response: FixtureResponse,
    notes: Vec<String>,
) -> Fixture {
    let mut f = Fixture {
        schema: 1,
        origin: FixtureOrigin::Recorded,
        recorded_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        source: source.to_owned(),
        provider: provider.to_owned(),
        case: case.to_owned(),
        request,
        response,
        notes,
    };
    f.sanitize();
    f
}

#[tokio::main]
#[allow(clippy::too_many_lines)] // one match arm per subcommand
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = cli.out.clone().unwrap_or_else(Fixture::live_root);
    let cfg = DiscoveryConfig::from_env()?;
    let client = HttpClient::new(Profile::Discovery, ClientConfig::from_network(&cfg.network))?;
    let feed_client = HttpClient::new(Profile::Feed, ClientConfig::from_network(&cfg.network))?;

    match cli.command {
        Command::Provider {
            provider,
            case,
            query,
            limit,
            country,
        } => {
            let (url, q) = provider_request(provider, &cfg, &query, limit, &country)?;
            let headers = if matches!(provider, Provider::Podcastindex) {
                auth_headers(&cfg)?
            } else {
                Vec::new()
            };
            let (response, notes) = record(&client, &url, headers, 2_000_000).await?;
            let request = FixtureRequest {
                method: "GET".into(),
                path: url.path().to_owned(),
                query: q,
            };
            let f = fixture(provider.id(), &case, url.as_str(), request, response, notes);
            let path = write_fixture(&root, &f)?;
            println!("wrote {} (status {})", path.display(), f.response.status);
        }
        Command::Lookup {
            provider,
            case,
            reference,
        } => {
            let (url, q) = lookup_request(provider, &cfg, &reference)?;
            let headers = if matches!(provider, Provider::Podcastindex) {
                auth_headers(&cfg)?
            } else {
                Vec::new()
            };
            let (response, notes) = record(&client, &url, headers, 2_000_000).await?;
            let request = FixtureRequest {
                method: "GET".into(),
                path: url.path().to_owned(),
                query: q,
            };
            let f = fixture(provider.id(), &case, url.as_str(), request, response, notes);
            let path = write_fixture(&root, &f)?;
            println!("wrote {} (status {})", path.display(), f.response.status);
        }
        Command::Url {
            url,
            case,
            max_bytes,
        } => {
            let url = Url::parse(&url)?;
            let (response, notes) = record(&feed_client, &url, Vec::new(), max_bytes).await?;
            let host = url.host_str().unwrap_or("unknown").replace('.', "_");
            let request = FixtureRequest {
                method: "GET".into(),
                path: url.path().to_owned(),
                query: url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect(),
            };
            let f = fixture(
                &format!("web/{host}"),
                &case,
                url.as_str(),
                request,
                response,
                notes,
            );
            let path = write_fixture(&root, &f)?;
            println!("wrote {} (status {})", path.display(), f.response.status);
        }
        Command::Diff { a, b } => {
            let fa = Fixture::load_path(&a)?;
            let fb = Fixture::load_path(&b)?;
            let (only_a, only_b) = diff_keys(&fa.response.body, &fb.response.body);
            println!("status: {} vs {}", fa.response.status, fb.response.status);
            println!("origin: {:?} vs {:?}", fa.origin, fb.origin);
            println!(
                "only in {}: {}",
                a.display(),
                if only_a.is_empty() {
                    "-".into()
                } else {
                    only_a.join(", ")
                }
            );
            println!(
                "only in {}: {}",
                b.display(),
                if only_b.is_empty() {
                    "-".into()
                } else {
                    only_b.join(", ")
                }
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn provider_requests_match_the_providers() {
        let cfg = DiscoveryConfig::default();
        let (url, q) =
            provider_request(Provider::Apple, &cfg, "darknet diaries", 10, "DE").unwrap();
        assert!(url.as_str().starts_with("https://itunes.apple.com/search?"));
        assert_eq!(q.get("term").map(String::as_str), Some("darknet diaries"));
        assert_eq!(q.get("country").map(String::as_str), Some("DE"));
        let (url, q) = provider_request(Provider::Podcastindex, &cfg, "x", 500, "US").unwrap();
        assert!(url.path().ends_with("/search/byterm"));
        assert_eq!(q.get("max").map(String::as_str), Some("100"));
        let (url, _) = lookup_request(Provider::Podcastindex, &cfg, "itunes:123").unwrap();
        assert!(url.path().ends_with("/podcasts/byitunesid"));
        let (url, _) = lookup_request(Provider::Podcastindex, &cfg, "https://x.test/feed").unwrap();
        assert!(url.path().ends_with("/podcasts/byfeedurl"));
        let (url, _) = lookup_request(Provider::Gpoddernet, &cfg, "https://x.test/feed").unwrap();
        assert!(url.path().ends_with("/api/2/data/podcast.json"));
    }

    #[test]
    fn bodies_are_parsed_or_kept_raw() {
        let mut notes = Vec::new();
        let v = body_value(br#"{"a":[1,2,3]}"#, &mut notes, 1000);
        assert_eq!(v["a"][2], 3);
        let big = serde_json::json!({ "feeds": (0..80).collect::<Vec<_>>() }).to_string();
        let v = body_value(big.as_bytes(), &mut notes, 1_000_000);
        assert_eq!(v["feeds"].as_array().unwrap().len(), 50);
        assert!(notes.iter().any(|n| n.contains("array truncated")));
        let v = body_value(b"<rss/>", &mut notes, 1000);
        assert_eq!(v, serde_json::Value::String("<rss/>".into()));
    }

    #[test]
    fn recorded_fixtures_are_sanitized_and_diffable() {
        let mut query = BTreeMap::new();
        query.insert("q".to_owned(), "x".to_owned());
        query.insert("key".to_owned(), "SECRET".to_owned());
        let request = FixtureRequest {
            method: "GET".into(),
            path: "/search".into(),
            query,
        };
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_owned(), "leak".to_owned());
        headers.insert("content-type".to_owned(), "application/json".to_owned());
        let response = FixtureResponse {
            status: 200,
            headers,
            body: serde_json::json!({"a": 1, "b": {"c": 2}}),
        };
        let f = fixture(
            "apple",
            "case",
            "https://example.test/search",
            request,
            response,
            vec![],
        );
        assert!(f.validate().is_ok());
        assert!(!f.request.query.contains_key("key"));
        assert!(!f.response.headers.contains_key("authorization"));
        assert_eq!(f.origin, FixtureOrigin::Recorded);
        let (only_a, only_b) = diff_keys(&f.response.body, &serde_json::json!({"a": 1, "d": 3}));
        assert_eq!(only_a, vec!["b", "b.c"]);
        assert_eq!(only_b, vec!["d"]);
    }
}
