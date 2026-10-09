//! Handlers for `podcast add|list|show|refresh|import|export|move-feed|archive|remove`,
//! `episode list|show|duplicates|resolve` and `feed inspect|refresh|status`.
//!
//! Embedded mode opens the engine (and the data directory lock); `--server`
//! sends the same operations to a running server. `feed inspect` never
//! touches the database: embedded, it only builds an HTTP client.

use std::io::Read as _;
use std::path::Path;

use tokio_util::sync::CancellationToken;
use uguisu_core::UguisuError;
use uguisu_core::archive::PolicyMode;
use uguisu_core::download::Priority;
use uguisu_core::feed::RefreshReport;
use uguisu_core::ids::{EpisodeId, PodcastId, SourceId};
use uguisu_core::model::DuplicateResolution;
use uguisu_discovery::ResolvedFeed;
use uguisu_engine::library::AddOutcome;
use uguisu_engine::migration::MoveOptions;
use uguisu_engine::opml::{MAX_BYTES, OpmlOptions, PolicyDefaults};
use uguisu_engine::{RefreshAllEntry, RefreshOptions};
use url::Url;

use crate::client::{ApiClient, ClientError};
use crate::output::{self, Exit};
use crate::{Cli, ImportArgs, MoveFeedArgs, open_engine};

pub(crate) fn client_exit(cli: &Cli, command: &str, e: &ClientError) -> Exit {
    output::error(cli.json, command, &e.to_string());
    match e {
        ClientError::Api { kind, .. } => match kind.as_str() {
            "not_found" | "invalid" => Exit::Usage,
            "blocked_by_policy" => Exit::Blocked,
            "unresolvable" => Exit::Unresolvable,
            "feed" => Exit::FeedError,
            "network" => Exit::NetworkError,
            "disk_full" => Exit::DiskFull,
            _ => Exit::Error,
        },
        _ => Exit::Error,
    }
}

pub(crate) fn engine_exit(cli: &Cli, command: &str, e: &UguisuError) -> Exit {
    output::error(cli.json, command, &e.to_string());
    Exit::for_engine_error(e)
}

pub(crate) fn remote(cli: &Cli, command: &str) -> Result<Option<ApiClient>, Exit> {
    match &cli.server {
        None => Ok(None),
        Some(server) => ApiClient::new(server, cli.token.as_ref())
            .map(Some)
            .map_err(|e| {
                output::error(cli.json, command, &e.to_string());
                Exit::Error
            }),
    }
}

pub(crate) fn parse_podcast_id(cli: &Cli, command: &str, raw: &str) -> Result<PodcastId, Exit> {
    raw.trim().parse().map_err(|_| {
        output::error(
            cli.json,
            command,
            &format!("`{raw}` is not a podcast id (see `uguisu podcast list`)"),
        );
        Exit::Usage
    })
}

fn parse_source_id(cli: &Cli, command: &str, raw: &str) -> Result<SourceId, Exit> {
    raw.trim().parse().map_err(|_| {
        output::error(
            cli.json,
            command,
            &format!("`{raw}` is not a source id (see `uguisu podcast show`)"),
        );
        Exit::Usage
    })
}

/// Whether the input already was the feed the resolver verified (no
/// confirmation needed).
fn is_direct(input: &str, feed: &ResolvedFeed) -> bool {
    let Ok(url) = Url::parse(input.trim()) else {
        return false;
    };
    let same = |u: &Url| u.as_str().trim_end_matches('/') == url.as_str().trim_end_matches('/');
    same(&feed.feed_url)
        || feed.canonical_url.as_ref().is_some_and(same)
        || feed.moved_to.as_ref().is_some_and(same)
}

#[allow(clippy::print_stdout, clippy::print_stderr)]
fn print_add(cli: &Cli, outcome: &AddOutcome) -> Exit {
    if cli.json {
        output::json_with_schema(outcome);
    } else {
        print!("{}", output::render_add(outcome));
    }
    outcome.report.as_ref().map_or(Exit::Ok, Exit::for_report)
}

/// `podcast add <input> [--yes]`: a feed URL is added directly; a website
/// or directory page is resolved first and, without `--yes` (or `--json`),
/// the resolution is shown and the command stops with exit 2.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub async fn run_podcast_add(cli: &Cli, input: &str, yes: bool) -> Exit {
    const CMD: &str = "podcast add";
    let confirmed = yes || cli.json;
    match remote(cli, CMD) {
        Err(exit) => exit,
        Ok(Some(client)) => {
            if !confirmed {
                let resolved = match client.resolve(input).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(failure)) => {
                        eprint!("{}", output::render_failure(&failure));
                        return Exit::for_resolve_error(&failure.error);
                    }
                    Err(e) => return client_exit(cli, CMD, &e),
                };
                if !is_direct(input, &resolved) {
                    print!("{}", output::render_resolved(&resolved));
                    output::error(
                        false,
                        CMD,
                        "input resolved to the feed above; re-run with --yes to add it",
                    );
                    return Exit::Usage;
                }
            }
            match client.add_podcast(input).await {
                Ok(outcome) => print_add(cli, &outcome),
                Err(e) => client_exit(cli, CMD, &e),
            }
        }
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let cancel = CancellationToken::new();
            let resolved = match engine.resolve_input(input, cancel.clone()).await {
                Ok(r) => r,
                Err(e) => return engine_exit(cli, CMD, &e),
            };
            if !confirmed && !is_direct(input, &resolved) {
                print!("{}", output::render_resolved(&resolved));
                output::error(
                    false,
                    CMD,
                    "input resolved to the feed above; re-run with --yes to add it",
                );
                return Exit::Usage;
            }
            let mut outcome = match engine.add_resolved(resolved).await {
                Ok(o) => o,
                Err(e) => return engine_exit(cli, CMD, &e),
            };
            if outcome.created {
                match engine
                    .refresh_podcast(
                        outcome.podcast.id,
                        RefreshOptions {
                            force: false,
                            cancel: Some(cancel),
                        },
                    )
                    .await
                {
                    Ok(report) => {
                        if let Ok(detail) = engine.podcast(outcome.podcast.id).await {
                            outcome.podcast = detail.podcast;
                        }
                        outcome.report = Some(report);
                    }
                    Err(e) => return engine_exit(cli, CMD, &e),
                }
            }
            let exit = print_add(cli, &outcome);
            engine.close().await;
            exit
        }
    }
}

/// `podcast list`.
#[allow(clippy::print_stdout)]
pub async fn run_podcast_list(cli: &Cli) -> Exit {
    const CMD: &str = "podcast list";
    let list = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.podcasts().await {
            Ok(l) => l,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.list_podcasts().await;
            engine.close().await;
            match result {
                Ok(l) => l,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "podcasts": list }));
    } else {
        print!("{}", output::render_podcasts(&list));
    }
    Exit::Ok
}

/// `podcast show <id>`.
#[allow(clippy::print_stdout)]
pub async fn run_podcast_show(cli: &Cli, id: &str) -> Exit {
    const CMD: &str = "podcast show";
    let detail = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.podcast(id.trim()).await {
            Ok(d) => d,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let id = match parse_podcast_id(cli, CMD, id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.podcast(id).await;
            engine.close().await;
            match result {
                Ok(d) => d,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&detail);
    } else {
        print!("{}", output::render_podcast(&detail));
    }
    Exit::Ok
}

#[allow(clippy::print_stdout)]
fn print_report(cli: &Cli, title: &str, report: &RefreshReport) -> Exit {
    if cli.json {
        output::json(report);
    } else {
        print!("{}", output::render_report(title, report));
    }
    Exit::for_report(report)
}

#[allow(clippy::print_stdout)]
fn print_refresh_all(cli: &Cli, entries: &[RefreshAllEntry]) -> Exit {
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "entries": entries }));
    } else {
        print!("{}", output::render_refresh_all(entries));
    }
    Exit::worst(entries.iter().map(|e| match (&e.report, &e.error) {
        (Some(r), _) => Exit::for_report(r),
        (None, Some(err)) => Exit::for_engine_error(err),
        (None, None) => Exit::Error,
    }))
}

/// `podcast refresh <id> | --all [--force]`.
pub async fn run_podcast_refresh(cli: &Cli, id: Option<&str>, all: bool, force: bool) -> Exit {
    const CMD: &str = "podcast refresh";
    match remote(cli, CMD) {
        Err(exit) => exit,
        Ok(Some(client)) => {
            if all {
                match client.refresh_all(force).await {
                    Ok(entries) => print_refresh_all(cli, &entries),
                    Err(e) => client_exit(cli, CMD, &e),
                }
            } else {
                let id = id.unwrap_or_default().trim();
                match client.refresh(id, force).await {
                    Ok(report) => print_report(cli, id, &report),
                    Err(e) => client_exit(cli, CMD, &e),
                }
            }
        }
        Ok(None) => {
            let target = if all {
                None
            } else {
                match parse_podcast_id(cli, CMD, id.unwrap_or_default()) {
                    Ok(id) => Some(id),
                    Err(exit) => return exit,
                }
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let exit = match target {
                None => {
                    let concurrency = engine.config().feed.refresh_concurrency;
                    match engine
                        .refresh_all(force, concurrency, CancellationToken::new())
                        .await
                    {
                        Ok(entries) => print_refresh_all(cli, &entries),
                        Err(e) => engine_exit(cli, CMD, &e),
                    }
                }
                Some(id) => refresh_one(cli, &engine, id, force, CMD).await,
            };
            engine.close().await;
            exit
        }
    }
}

async fn refresh_one(
    cli: &Cli,
    engine: &uguisu_engine::Engine,
    id: PodcastId,
    force: bool,
    command: &str,
) -> Exit {
    let title = engine
        .podcast(id)
        .await
        .map_or_else(|_| id.to_string(), |d| d.podcast.title);
    match engine
        .refresh_podcast(
            id,
            RefreshOptions {
                force,
                cancel: None,
            },
        )
        .await
    {
        Ok(report) => print_report(cli, &title, &report),
        Err(e) => engine_exit(cli, command, &e),
    }
}

/// `feed refresh <source-id> [--force]`: refreshes the podcast behind a
/// current source.
pub async fn run_feed_refresh(cli: &Cli, source_id: &str, force: bool) -> Exit {
    const CMD: &str = "feed refresh";
    match remote(cli, CMD) {
        Err(exit) => exit,
        Ok(Some(client)) => {
            let status = match client.feed_status(source_id.trim()).await {
                Ok(s) => s,
                Err(e) => return client_exit(cli, CMD, &e),
            };
            if !status.source.is_current {
                output::error(
                    cli.json,
                    CMD,
                    "source is not current; refresh the podcast instead",
                );
                return Exit::Usage;
            }
            let pid = status.source.podcast_id.to_string();
            match client.refresh(&pid, force).await {
                Ok(report) => print_report(cli, &pid, &report),
                Err(e) => client_exit(cli, CMD, &e),
            }
        }
        Ok(None) => {
            let sid = match parse_source_id(cli, CMD, source_id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let exit = match engine.source(sid).await {
                Ok((source, _)) if source.is_current => {
                    refresh_one(cli, &engine, source.podcast_id, force, CMD).await
                }
                Ok(_) => {
                    output::error(
                        cli.json,
                        CMD,
                        "source is not current; refresh the podcast instead",
                    );
                    Exit::Usage
                }
                Err(e) => engine_exit(cli, CMD, &e),
            };
            engine.close().await;
            exit
        }
    }
}

/// `feed status <source-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_feed_status(cli: &Cli, source_id: &str) -> Exit {
    const CMD: &str = "feed status";
    let (source, last) = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.feed_status(source_id.trim()).await {
            Ok(s) => (s.source, s.last_fetch),
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let sid = match parse_source_id(cli, CMD, source_id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.source(sid).await;
            engine.close().await;
            match result {
                Ok(s) => s,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&uguisu_server::FeedStatus {
            source: source.clone(),
            last_fetch: last.clone(),
        });
    } else {
        print!("{}", output::render_feed_status(&source, last.as_ref()));
    }
    Exit::Ok
}

/// `feed inspect <url>`: fetch and parse only; no data directory, no lock.
#[allow(clippy::print_stdout)]
pub async fn run_feed_inspect(cli: &Cli, url: &str) -> Exit {
    const CMD: &str = "feed inspect";
    let inspection = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.inspect(url.trim()).await {
            Ok(i) => i,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let parsed = match Url::parse(url.trim()) {
                Ok(u) => u,
                Err(e) => {
                    output::error(cli.json, CMD, &format!("`{url}` is not a URL: {e}"));
                    return Exit::Usage;
                }
            };
            let config = match crate::engine_config(cli) {
                Ok(c) => c,
                Err(e) => {
                    output::error(cli.json, CMD, &e);
                    return Exit::Error;
                }
            };
            let client = match uguisu_engine::inspect::standalone_client(&config.discovery.network)
            {
                Ok(c) => c,
                Err(e) => return engine_exit(cli, CMD, &e),
            };
            match uguisu_engine::inspect(
                &client,
                &config.feed.limits,
                &parsed,
                CancellationToken::new(),
            )
            .await
            {
                Ok(i) => i,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json(&inspection);
    } else {
        print!("{}", output::render_inspection(&inspection));
    }
    if inspection.looks_like_podcast && !inspection.truncated && inspection.malformed_items == 0 {
        Exit::Ok
    } else if inspection.looks_like_podcast {
        Exit::Partial
    } else {
        Exit::FeedError
    }
}

/// `podcast import <file> [--apply] [--mode …]`: plans an OPML import, or
/// adds its new feeds (ADR 0049). The file is read here, so `--server`
/// sends its text rather than a path on the server.
#[allow(clippy::print_stdout)]
pub async fn run_podcast_import(cli: &Cli, args: &ImportArgs) -> Exit {
    const CMD: &str = "podcast import";
    let policy = match args
        .mode
        .as_deref()
        .map(|mode| policy_defaults(mode, args))
        .transpose()
    {
        Ok(policy) => policy,
        Err(message) => {
            output::error(cli.json, CMD, &message);
            return Exit::Usage;
        }
    };
    let text = match read_document(&args.file) {
        Ok(text) => text,
        Err((exit, message)) => {
            output::error(cli.json, CMD, &message);
            return exit;
        }
    };
    let report = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => {
            let mut body = serde_json::json!({ "opml": text, "apply": args.apply });
            if args.mode.is_some() {
                body["policy"] = serde_json::json!({
                    "mode": args.mode,
                    "max_backlog": args.max_backlog,
                    "max_age_days": args.max_age_days,
                    "priority": args.priority,
                });
            }
            match client.import_opml(&body).await {
                Ok(report) => report,
                Err(e) => return client_exit(cli, CMD, &e),
            }
        }
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let report = engine
                .import_opml(
                    &text,
                    OpmlOptions {
                        apply: args.apply,
                        policy,
                    },
                    CancellationToken::new(),
                )
                .await;
            engine.close().await;
            match report {
                Ok(report) => report,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&report);
    } else {
        print!("{}", output::render_opml_import(&report));
    }
    Exit::Ok
}

fn policy_defaults(mode: &str, args: &ImportArgs) -> Result<PolicyDefaults, String> {
    let mode = PolicyMode::parse(mode)
        .ok_or_else(|| format!("`{mode}` is not a policy mode (manual, auto)"))?;
    let priority = args
        .priority
        .as_deref()
        .map(|raw| {
            Priority::parse(raw)
                .ok_or_else(|| format!("`{raw}` is not a priority (low, normal, high)"))
        })
        .transpose()?;
    Ok(PolicyDefaults {
        mode,
        max_backlog: args.max_backlog,
        max_age_days: args.max_age_days,
        priority,
    })
}

/// The document as text, read no further than one byte past the cap. A
/// file in another encoding loses accented title characters, never a URL.
fn read_document(file: &Path) -> Result<String, (Exit, String)> {
    let cap = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX) + 1;
    let mut bytes = Vec::new();
    let read = if file == Path::new("-") {
        std::io::stdin().lock().take(cap).read_to_end(&mut bytes)
    } else {
        std::fs::File::open(file).and_then(|f| f.take(cap).read_to_end(&mut bytes))
    };
    read.map_err(|e| (Exit::Error, format!("cannot read {}: {e}", file.display())))?;
    if bytes.len() > MAX_BYTES {
        return Err((
            Exit::Usage,
            format!(
                "{} is larger than {MAX_BYTES} bytes, the most an import reads",
                file.display()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `podcast export`: every podcast as an OPML file on stdout.
#[allow(clippy::print_stdout)]
pub async fn run_podcast_export(cli: &Cli) -> Exit {
    const CMD: &str = "podcast export";
    let document = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.export_opml().await {
            Ok(document) => document,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let document = engine.export_opml().await;
            engine.close().await;
            match document {
                Ok(document) => document,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "opml": document }));
    } else {
        print!("{document}");
    }
    Exit::Ok
}

/// `episode duplicates [--podcast <id>]`: every candidate duplicate with the
/// episode it probably duplicates.
#[allow(clippy::print_stdout)]
pub async fn run_duplicates(cli: &Cli, podcast: Option<&str>) -> Exit {
    const CMD: &str = "episode duplicates";
    let duplicates = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.duplicates(podcast.map(str::trim)).await {
            Ok(d) => d,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let podcast = match podcast.map(|p| parse_podcast_id(cli, CMD, p)).transpose() {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let mut duplicates = Vec::new();
            let mut after = None;
            let result = loop {
                match engine
                    .duplicates(podcast, after, uguisu_core::page::MAX)
                    .await
                {
                    Ok(page) => {
                        duplicates.extend(page.duplicates);
                        after = page.next_after;
                        if after.is_none() {
                            break Ok(duplicates);
                        }
                    }
                    Err(e) => break Err(e),
                }
            };
            engine.close().await;
            match result {
                Ok(d) => d,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "duplicates": duplicates }));
    } else {
        print!("{}", output::render_duplicates(&duplicates));
    }
    Exit::Ok
}

/// `podcast move-feed <id> <url> [--dry-run] [--force]` (ADR 0052). A
/// feed that fails the same-show check without `--force` exits 2, as a
/// confirmation the command needs.
#[allow(clippy::print_stdout)]
pub async fn run_move_feed(cli: &Cli, args: &MoveFeedArgs) -> Exit {
    const CMD: &str = "podcast move-feed";
    let (id, url) = (args.id.as_str(), args.url.as_str());
    let options = MoveOptions {
        dry_run: args.dry_run,
        force: args.force,
    };
    let moved = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.move_feed(id.trim(), url, options).await {
            Ok(m) => m,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let Ok(podcast) = id.trim().parse::<PodcastId>() else {
                output::error(
                    cli.json,
                    CMD,
                    &format!("`{id}` is not a podcast id (see `uguisu podcast list`)"),
                );
                return Exit::Usage;
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine
                .move_feed(podcast, url, options, CancellationToken::new())
                .await;
            engine.close().await;
            match result {
                Ok(m) => m,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&moved);
    } else {
        print!("{}", output::render_move(&moved, options));
    }
    if !(moved.moved || moved.verified || options.dry_run) {
        if !cli.json {
            output::error(
                false,
                CMD,
                "the feed failed the same-show check; re-run with --force to move anyway",
            );
        }
        return Exit::Usage;
    }
    moved.report.as_ref().map_or(Exit::Ok, Exit::for_report)
}

/// `episode list <podcast-id>`: every episode, newest first.
#[allow(clippy::print_stdout)]
pub async fn run_episode_list(cli: &Cli, podcast_id: &str) -> Exit {
    const CMD: &str = "episode list";
    let episodes = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.episodes(podcast_id.trim()).await {
            Ok(e) => e,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let podcast = match parse_podcast_id(cli, CMD, podcast_id) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let mut episodes = Vec::new();
            let mut after = None;
            let result = loop {
                match engine
                    .episodes(podcast, after, uguisu_engine::library::MAX_PAGE)
                    .await
                {
                    Ok(page) => {
                        episodes.extend(page.episodes);
                        match page.next_after {
                            Some(cursor) => after = Some(cursor),
                            None => break Ok(()),
                        }
                    }
                    Err(e) => break Err(e),
                }
            };
            engine.close().await;
            if let Err(e) = result {
                return engine_exit(cli, CMD, &e);
            }
            episodes
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "episodes": episodes }));
    } else {
        print!("{}", output::render_episodes(&episodes));
    }
    Exit::Ok
}

/// `episode show <id>`: the episode, its download job and its archive record.
#[allow(clippy::print_stdout)]
pub async fn run_episode_show(cli: &Cli, id: &str) -> Exit {
    const CMD: &str = "episode show";
    let detail = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.episode(id.trim()).await {
            Ok(d) => d,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let Ok(episode) = id.trim().parse::<EpisodeId>() else {
                output::error(
                    cli.json,
                    CMD,
                    &format!("`{id}` is not an episode id (see `uguisu episode list`)"),
                );
                return Exit::Usage;
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.episode(episode).await;
            engine.close().await;
            match result {
                Ok(d) => d,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&detail);
    } else {
        print!("{}", output::render_episode(&detail));
    }
    Exit::Ok
}

/// `podcast archive <id>` (ADR 0055): never refreshed again until resumed.
#[allow(clippy::print_stdout)]
pub async fn run_podcast_archive(cli: &Cli, id: &str) -> Exit {
    const CMD: &str = "podcast archive";
    let status = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.archive_podcast(id.trim()).await {
            Ok(s) => s,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let podcast = match parse_podcast_id(cli, CMD, id) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.archive_podcast(podcast).await;
            engine.close().await;
            match result {
                Ok(s) => s,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({
            "podcast_id": id.trim(),
            "status": status.as_str(),
        }));
    } else {
        println!("{} is {}", id.trim(), status.as_str());
    }
    Exit::Ok
}

/// `podcast remove <id> --yes` (ADR 0055): the podcast's records go, its
/// files stay. Without `--yes` nothing is touched and the exit is 2.
#[allow(clippy::print_stdout)]
pub async fn run_podcast_remove(cli: &Cli, id: &str, yes: bool) -> Exit {
    const CMD: &str = "podcast remove";
    if !yes {
        output::error(
            cli.json,
            CMD,
            &format!(
                "removing podcast {} deletes its records and keeps its files; re-run with --yes",
                id.trim()
            ),
        );
        return Exit::Usage;
    }
    let removed = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.remove_podcast(id.trim()).await {
            Ok(r) => r,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let podcast = match parse_podcast_id(cli, CMD, id) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.remove_podcast(podcast).await;
            engine.close().await;
            match result {
                Ok(r) => r,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&removed);
    } else {
        println!(
            "Removed podcast {} ({}): {} episodes; its {} archived files stay on disk",
            removed.podcast_id, removed.title, removed.episodes, removed.files
        );
    }
    Exit::Ok
}

/// `episode resolve <id> --same|--separate` (ADR 0051).
#[allow(clippy::print_stdout)]
pub async fn run_resolve(cli: &Cli, id: &str, resolution: DuplicateResolution) -> Exit {
    const CMD: &str = "episode resolve";
    let resolved = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.resolve_duplicate(id.trim(), resolution).await {
            Ok(r) => r,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let Ok(episode) = id.trim().parse::<EpisodeId>() else {
                output::error(
                    cli.json,
                    CMD,
                    &format!("`{id}` is not an episode id (see `uguisu episode duplicates`)"),
                );
                return Exit::Usage;
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.resolve_duplicate(episode, resolution).await;
            engine.close().await;
            match result {
                Ok(r) => r,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&resolved);
    } else {
        print!("{}", output::render_resolution(&resolved));
    }
    Exit::Ok
}
