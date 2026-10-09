//! The OpenAPI document (ADR 0039).
//!
//! The document is a function of the code: `ToSchema` beside `Serialize`,
//! `utoipa::path` beside every handler, and `tools/openapi` writing
//! `docs/api/openapi.json`. `python3 scripts/check.py openapi` fails when the
//! committed file and the code disagree, which is what keeps this a contract
//! and not documentation.

use utoipa::Modify;
use utoipa::OpenApi;
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::schema::{AllOf, Object, Schema, Type};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, HttpMethod, Ref, RefOr, Server};

/// The `/api/v1` contract.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Uguisu",
        version = "1",
        description = "The API of a self-hosted podcast archiver. Version 1; \
                       every success body carries `schema: 1` and every failure \
                       the same `error` envelope."
    ),
    modifiers(&Contract),
    security(("session_cookie" = []), ("bearer_token" = [])),
    tags(
        (name = "auth", description = "Sessions, the password and API tokens"),
        (name = "library", description = "Podcasts and episodes"),
        (name = "feeds", description = "Fetch state, and parsing a feed without storing it"),
        (name = "events", description = "The domain event log, stored and live"),
        (name = "downloads", description = "The download queue"),
        (name = "archive", description = "Archived files, verification, import"),
        (name = "policies", description = "What gets archived automatically"),
        (name = "sidecars", description = "Per-episode sidecar documents"),
        (name = "manifests", description = "Per-podcast checksum manifests"),
        (name = "tags", description = "Tags embedded in the archived file"),
        (name = "artwork", description = "Podcast artwork records"),
        (name = "bytes", description = "Media and image bytes, with `Range`"),
        (name = "scheduler", description = "The refresh scheduler and maintenance"),
        (name = "settings", description = "Stored settings"),
        (name = "search", description = "Full-text search over the library"),
        (name = "discovery", description = "Providers, search and resolution"),
        (name = "service", description = "Liveness and what the daemon is doing")
    ),
    paths(
        crate::health,
        crate::search,
        crate::providers,
        crate::resolve_get,
        crate::resolve_post,
        crate::library::add_podcast,
        crate::library::list_podcasts,
        crate::library::show_podcast,
        crate::library::remove_podcast,
        crate::library::list_episodes,
        crate::library::show_episode,
        crate::library::list_duplicates,
        crate::library::resolve_duplicate,
        crate::library::refresh_podcast,
        crate::library::move_feed,
        crate::library::refresh_all,
        crate::library::export_opml,
        crate::library::import_opml,
        crate::library::feed_status,
        crate::library::inspect,
        crate::library::events,
        crate::downloads::enqueue,
        crate::downloads::enqueue_podcast,
        crate::downloads::list,
        crate::downloads::stats,
        crate::downloads::show,
        crate::downloads::cancel,
        crate::downloads::pause,
        crate::downloads::resume,
        crate::downloads::retry,
        crate::downloads::retry_failed,
        crate::downloads::pause_all,
        crate::downloads::resume_all,
        crate::downloads::reconcile,
        crate::archive::list,
        crate::archive::missing,
        crate::archive::invalid,
        crate::archive::stats,
        crate::archive::show,
        crate::archive::verify_one,
        crate::archive::verify_all,
        crate::archive::path_preview,
        crate::archive::relocate,
        crate::archive::reconcile,
        crate::archive::policies,
        crate::archive::policy_show,
        crate::archive::policy_set,
        crate::archive::policy_clear,
        crate::archive_assets::manifest_status,
        crate::archive_assets::manifests_write_all,
        crate::archive_assets::manifest_write,
        crate::archive_assets::manifest_verify,
        crate::archive_assets::rebuild,
        crate::archive_assets::orphans,
        crate::archive_assets::import,
        crate::archive_assets::restore,
        crate::archive_assets::redownload,
        crate::archive_assets::sidecar_show,
        crate::archive_assets::sidecar_write,
        crate::archive_assets::tags_show,
        crate::archive_assets::tags_write,
        crate::archive_assets::media,
        crate::archive_assets::artwork_show,
        crate::archive_assets::artwork_image,
        crate::archive_assets::artwork_fetch,
        crate::service::status,
        crate::service::scheduler,
        crate::service::pause,
        crate::service::resume,
        crate::service::run_pass,
        crate::service::run_maintenance,
        crate::service::pause_podcast,
        crate::service::resume_podcast,
        crate::service::archive_podcast,
        crate::service::backup_database,
        crate::service::check_database,
        crate::service::vacuum_database,
        crate::service::schedule_podcast,
        crate::service::list_settings,
        crate::service::set_setting,
        crate::service::clear_setting,
        crate::service::search,
        crate::service::reindex,
        crate::service::list_records,
        crate::service::show_record,
        crate::auth::session,
        crate::auth::login,
        crate::auth::logout,
        crate::auth::exchange,
        crate::auth::password,
        crate::auth::list_tokens,
        crate::auth::create_token,
        crate::auth::revoke_token,
    )
)]
pub struct ApiDoc;

/// The operations whose success body is not the `library::body` envelope, so
/// the one convention the derived document cannot see is stated once here
/// instead of on twenty handlers: liveness, the three discovery routes that
/// answer a provider payload directly, the two that answer bytes and the
/// OPML export, which answers a file.
const UNENVELOPED: [&str; 8] = [
    "health",
    "discovery_search",
    "providers",
    "resolve_get",
    "resolve_post",
    "media",
    "artwork_image",
    "export_opml",
];

/// Everything about the contract that is true of the whole API rather than of
/// one handler: how a client authenticates, where the API lives, the `schema`
/// key on every enveloped success, and the one route that exists twice.
struct Contract;

impl Modify for Contract {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi.servers = Some(vec![Server::new("/")]);
        let components = openapi.components.get_or_insert_with(Components::new);
        components.add_security_scheme(
            "session_cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                crate::auth::COOKIE,
                "Set by `POST /api/v1/auth/login`. A mutating request must also \
                 send the session's CSRF token in `X-Uguisu-CSRF`.",
            ))),
        );
        components.add_security_scheme(
            "bearer_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "An API token from `POST /api/v1/auth/tokens`. A `read` \
                         token is refused on a mutation; no CSRF header applies.",
                    ))
                    .build(),
            ),
        );
        components
            .schemas
            .insert(ENVELOPE.to_owned(), RefOr::T(envelope()));
        add_aliases(openapi);
        for item in openapi.paths.paths.values_mut() {
            for operation in operations(item) {
                add_envelope(operation);
            }
        }
    }
}

/// Name of the component that carries the `schema` key.
const ENVELOPE: &str = "Envelope";

fn envelope() -> Schema {
    Schema::Object(
        Object::builder()
            .description(Some(
                "Every enveloped success body carries the API's schema version.",
            ))
            .schema_type(Type::Object)
            .property(
                "schema",
                Object::builder()
                    .schema_type(Type::Integer)
                    .enum_values(Some([crate::library::SCHEMA])),
            )
            .required("schema")
            .build(),
    )
}

fn operations(item: &mut PathItem) -> impl Iterator<Item = &mut Operation> {
    [
        item.get.as_mut(),
        item.put.as_mut(),
        item.post.as_mut(),
        item.delete.as_mut(),
        item.patch.as_mut(),
        item.head.as_mut(),
        item.options.as_mut(),
        item.trace.as_mut(),
    ]
    .into_iter()
    .flatten()
}

/// Wraps every JSON success body of `operation` so the document says what
/// `library::body` actually writes.
fn add_envelope(operation: &mut Operation) {
    let unenveloped = operation
        .operation_id
        .as_deref()
        .is_some_and(|id| UNENVELOPED.contains(&id));
    if unenveloped {
        return;
    }
    for (status, response) in &mut operation.responses.responses {
        if !status.starts_with('2') {
            continue;
        }
        let RefOr::T(response) = response else {
            continue;
        };
        for content in response.content.values_mut() {
            if let Some(schema) = content.schema.take() {
                content.schema = Some(RefOr::T(Schema::AllOf(
                    AllOf::builder()
                        .item(schema)
                        .item(Ref::from_schema_name(ENVELOPE))
                        .build(),
                )));
            }
        }
    }
}

/// A route reachable under a second method and path.
struct Alias {
    /// Where the documented operation is.
    from: (HttpMethod, &'static str),
    /// Where it is also reachable.
    to: (HttpMethod, &'static str),
    /// The copy's own operation id.
    operation_id: &'static str,
}

/// Five handlers answer under a second spelling, because the CLI's HTTP client
/// speaks GET and POST only and widening it for five routes is the worse trade.
/// A handler cannot carry two `utoipa::path` attributes, and an `operationId` is
/// unique across the document, so each second spelling is copied here with its
/// own id rather than wrapped in a function that only forwards.
const ALIASES: [Alias; 5] = [
    Alias {
        from: (HttpMethod::Delete, "/api/v1/podcasts/{id}"),
        to: (HttpMethod::Post, "/api/v1/podcasts/{id}/remove"),
        operation_id: "remove_podcast_post",
    },
    Alias {
        from: (HttpMethod::Delete, "/api/v1/auth/tokens/{id}"),
        to: (HttpMethod::Post, "/api/v1/auth/tokens/{id}/revoke"),
        operation_id: "revoke_token_post",
    },
    Alias {
        from: (HttpMethod::Post, "/api/v1/podcasts/{id}/policy/clear"),
        to: (HttpMethod::Delete, "/api/v1/podcasts/{id}/policy"),
        operation_id: "policy_clear_delete",
    },
    Alias {
        from: (HttpMethod::Put, "/api/v1/podcasts/{id}/policy"),
        to: (HttpMethod::Post, "/api/v1/podcasts/{id}/policy"),
        operation_id: "policy_set_post",
    },
    Alias {
        from: (
            HttpMethod::Get,
            "/api/v1/archive/manifests/{podcast_id}/verify",
        ),
        to: (
            HttpMethod::Post,
            "/api/v1/archive/manifests/{podcast_id}/verify",
        ),
        operation_id: "manifest_verify_post",
    },
];

fn add_aliases(openapi: &mut utoipa::openapi::OpenApi) {
    for alias in ALIASES {
        let (from_method, from_path) = alias.from;
        let (to_method, to_path) = alias.to;
        let Some(source) = openapi
            .paths
            .paths
            .get(from_path)
            .and_then(|item| operation_of(item, &from_method))
        else {
            continue;
        };
        let mut operation = source.clone();
        operation.operation_id = Some(alias.operation_id.to_owned());
        let copy = PathItem::new(to_method, operation);
        openapi
            .paths
            .paths
            .entry(to_path.to_owned())
            .or_default()
            .merge_operations(copy);
    }
}

fn operation_of<'a>(item: &'a PathItem, method: &HttpMethod) -> Option<&'a Operation> {
    match method {
        HttpMethod::Get => item.get.as_ref(),
        HttpMethod::Put => item.put.as_ref(),
        HttpMethod::Post => item.post.as_ref(),
        HttpMethod::Delete => item.delete.as_ref(),
        HttpMethod::Options => item.options.as_ref(),
        HttpMethod::Head => item.head.as_ref(),
        HttpMethod::Patch => item.patch.as_ref(),
        HttpMethod::Trace => item.trace.as_ref(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use axum::http::Method;
    use utoipa::OpenApi;

    use super::{ApiDoc, UNENVELOPED};
    use crate::auth::{Access, access_of};

    /// Every module that registers routes. A module added without a line here
    /// is not checked, which is why `routes()` lives in exactly these seven.
    const ROUTED: [&str; 7] = [
        include_str!("lib.rs"),
        include_str!("auth.rs"),
        include_str!("library.rs"),
        include_str!("downloads.rs"),
        include_str!("archive.rs"),
        include_str!("archive_assets.rs"),
        include_str!("service.rs"),
    ];

    /// The `(method, path)` pairs the router actually registers, read out of
    /// the source: axum's `Router` cannot be asked what it contains, and a
    /// route the document does not mention is exactly the drift to catch.
    fn registered() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for source in ROUTED {
            for span in spans(source) {
                let Some(path) = literal(&span) else { continue };
                if !path.starts_with("/api/") {
                    continue;
                }
                for method in ["get", "put", "post", "delete", "patch"] {
                    if span.contains(&format!("{method}(")) {
                        out.push((method.to_owned(), path.clone()));
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The text inside each `.route(…)` call, comments stripped.
    fn spans(source: &str) -> Vec<String> {
        let mut spans = Vec::new();
        let bytes = source.as_bytes();
        let mut at = 0;
        while let Some(found) = source[at..].find(".route(") {
            let start = at + found + ".route(".len();
            let mut depth = 1usize;
            let mut end = start;
            while end < bytes.len() && depth > 0 {
                match bytes[end] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                end += 1;
            }
            let span: String = source[start..end]
                .lines()
                .map(|line| line.split("//").next().unwrap_or(line))
                .collect::<Vec<_>>()
                .join(" ");
            spans.push(span);
            at = end;
        }
        spans
    }

    fn literal(span: &str) -> Option<String> {
        let rest = span.split_once('"')?.1;
        rest.split_once('"').map(|(value, _)| value.to_owned())
    }

    fn document() -> serde_json::Value {
        serde_json::to_value(ApiDoc::openapi()).unwrap()
    }

    #[test]
    fn every_route_has_an_operation() {
        let document = document();
        let paths = document["paths"].as_object().unwrap();
        let missing: Vec<_> = registered()
            .into_iter()
            .filter(|(method, path)| {
                paths
                    .get(path)
                    .is_none_or(|item| item.get(method).is_none())
            })
            .collect();
        assert!(
            missing.is_empty(),
            "routes the document does not describe: {missing:?}"
        );
    }

    #[test]
    fn resolve_lists_every_status() {
        // `failure_status` in lib.rs is the table these come from.
        let document = document();
        for method in ["get", "post"] {
            let responses: Vec<&str> =
                document["paths"]["/api/v1/discovery/resolve"][method]["responses"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
            assert_eq!(
                responses,
                ["200", "400", "403", "422", "500", "502", "504"],
                "{method}"
            );
        }
    }

    #[test]
    fn every_operation_has_a_route() {
        let registered = registered();
        let document = document();
        let extra: Vec<_> = document["paths"]
            .as_object()
            .unwrap()
            .iter()
            .flat_map(|(path, item)| {
                item.as_object()
                    .unwrap()
                    .keys()
                    .map(move |method| (method.clone(), path.clone()))
            })
            .filter(|pair| !registered.contains(pair))
            .collect();
        assert!(extra.is_empty(), "documented but not routed: {extra:?}");
    }

    /// An `operationId` is unique across the whole document, and utoipa derives
    /// it from the handler's function name — which repeats across modules, so
    /// `list`, `show` and `search` need one spelled out.
    #[test]
    fn every_operation_id_is_unique() {
        let document = document();
        let mut seen = std::collections::BTreeSet::new();
        for (path, item) in document["paths"].as_object().unwrap() {
            for (method, operation) in item.as_object().unwrap() {
                let id = operation["operationId"].as_str().unwrap();
                assert!(
                    seen.insert(id.to_owned()),
                    "{method} {path}: `{id}` is already another operation's id"
                );
            }
        }
    }

    /// Without this, a scan that stopped finding routes would make
    /// `every_route_has_an_operation` pass by describing nothing.
    #[test]
    fn the_scan_sees_every_route() {
        assert_eq!(registered().len(), 98);
    }

    /// The document's security and the classifier must agree: an operation the
    /// document shows as open (`security: []`) is one `access_of` calls public,
    /// and every other operation authenticates.
    #[test]
    fn declared_security_matches_the_classifier() {
        let document = document();
        for (path, item) in document["paths"].as_object().unwrap() {
            let concrete = path
                .split('/')
                .map(|segment| {
                    if segment.starts_with('{') {
                        "01ARZ3NDEKTSV4RRFFQ69G5FAV"
                    } else {
                        segment
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            for (name, operation) in item.as_object().unwrap() {
                let method = Method::from_bytes(name.to_uppercase().as_bytes()).unwrap();
                let open = operation["security"]
                    .as_array()
                    .is_some_and(std::vec::Vec::is_empty);
                let access = access_of(&method, &concrete, false);
                assert_eq!(
                    open,
                    access == Access::Public,
                    "{name} {path}: document says open={open}, classifier says {access:?}"
                );
            }
        }
    }

    #[test]
    fn no_operation_takes_a_credential_in_a_parameter() {
        let document = document().to_string();
        for forbidden in ["\"token\"", "\"password\"", "\"api_key\"", "\"secret\""] {
            for parameter in [
                format!("\"in\":\"query\",\"name\":{forbidden}"),
                format!("\"name\":{forbidden},\"in\":\"query\""),
            ] {
                assert!(
                    !document.contains(&parameter),
                    "a credential must never be a query parameter: {parameter}"
                );
            }
        }
    }

    #[test]
    fn an_enveloped_success_declares_the_schema_key() {
        let document = document();
        let wrapped = &document["paths"]["/api/v1/podcasts"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["allOf"];
        assert_eq!(
            wrapped[1]["$ref"], "#/components/schemas/Envelope",
            "an enveloped body must say so: {wrapped}"
        );
    }

    #[test]
    fn a_body_outside_the_envelope_is_not_wrapped() {
        let document = document();
        let plain = &document["paths"]["/api/v1/health"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"];
        assert_eq!(plain["$ref"], "#/components/schemas/Health");
    }

    #[test]
    fn the_unenveloped_list_names_live_operations() {
        let document = document();
        let ids: Vec<String> = document["paths"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|item| item.as_object().unwrap().values())
            .filter_map(|operation| operation["operationId"].as_str().map(str::to_owned))
            .collect();
        for id in UNENVELOPED {
            assert!(ids.iter().any(|seen| seen == id), "no operation named {id}");
        }
    }

    #[test]
    fn no_named_type_is_unknown() {
        let document = document();
        let schemas = document["components"]["schemas"].as_object().unwrap();
        let empty: Vec<_> = schemas
            .iter()
            .filter(|(_, schema)| schema.as_object().is_some_and(serde_json::Map::is_empty))
            .map(|(name, _)| name.clone())
            .collect();
        assert!(empty.is_empty(), "schemas with no shape: {empty:?}");
    }
}
