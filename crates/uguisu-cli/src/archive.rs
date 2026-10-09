//! Handlers for the `archive` command group.
//!
//! Everything here is either a report or an explicit change. Nothing in
//! this group deletes a file or a record: `verify` says what it found and
//! leaves the archive exactly as it was, `path-preview` creates nothing,
//! and `relocate` moves files only when it is asked to, with `--dry-run`
//! to see the moves first.
//!
//! Like `download`, each command runs either against the embedded engine
//! or, with `--server`, against a running one.

use uguisu_core::archive::{ArchiveFile, ArchivePolicy, PolicyMode, VerifyDepth};
use uguisu_core::download::Priority;
use uguisu_core::ids::{EpisodeId, PodcastId};
use uguisu_engine::Engine;
use uguisu_engine::archive::{ArchiveFilter, PathPreview, Relocation, VerifiedFile, VerifySummary};

use crate::library::{client_exit, engine_exit, parse_podcast_id, remote};
use crate::output::{self, Exit};
use crate::{Cli, open_engine};

/// Largest page the CLI asks for in one go.
const PAGE: u32 = 200;

fn parse_episode_id(cli: &Cli, command: &str, raw: &str) -> Result<EpisodeId, Exit> {
    raw.trim().parse().map_err(|_| {
        output::error(
            cli.json,
            command,
            &format!("`{raw}` is not an episode id (see `uguisu episode list`)"),
        );
        Exit::Usage
    })
}

fn depth_of(full: bool) -> VerifyDepth {
    if full {
        VerifyDepth::Full
    } else {
        VerifyDepth::Light
    }
}

#[allow(clippy::print_stdout)]
fn print_files(cli: &Cli, files: &[ArchiveFile], what: &str) -> Exit {
    if cli.json {
        #[derive(serde::Serialize)]
        struct Body<'a> {
            files: &'a [ArchiveFile],
        }
        output::json_with_schema(&Body { files });
    } else {
        print!("{}", output::render_archive_files(files, what));
    }
    Exit::Ok
}

/// `archive list [--podcast] [--state] [--source-changed] [--limit]`.
pub async fn run_list(
    cli: &Cli,
    podcast: Option<&str>,
    state: Option<&str>,
    source_changed: bool,
    limit: u32,
) -> Exit {
    run_listing(
        cli,
        "archive list",
        podcast,
        state,
        source_changed,
        limit,
        "",
    )
    .await
}

/// `archive missing` / `archive invalid`.
pub async fn run_problem_list(cli: &Cli, state: &str) -> Exit {
    run_listing(
        cli,
        &format!("archive {state}"),
        None,
        Some(state),
        false,
        PAGE,
        state,
    )
    .await
}

async fn run_listing(
    cli: &Cli,
    command: &str,
    podcast: Option<&str>,
    state: Option<&str>,
    source_changed: bool,
    limit: u32,
    what: &str,
) -> Exit {
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client
            .archive_list(state, podcast, source_changed, limit)
            .await
        {
            Ok(files) => print_files(cli, &files, what),
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let filter = match archive_filter(cli, command, podcast, state) {
        Ok(f) => ArchiveFilter {
            source_changed,
            ..f
        },
        Err(e) => return e,
    };
    let exit = match engine.archive_list(&filter, None, limit).await {
        Ok(page) => print_files(cli, &page.files, what),
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

fn archive_filter(
    cli: &Cli,
    command: &str,
    podcast: Option<&str>,
    state: Option<&str>,
) -> Result<ArchiveFilter, Exit> {
    let podcast_id: Option<PodcastId> = match podcast {
        Some(p) => Some(parse_podcast_id(cli, command, p)?),
        None => None,
    };
    let state = match state {
        None => None,
        Some(raw) => Some(
            uguisu_core::archive::VerificationState::parse(raw).ok_or_else(|| {
                output::error(
                    cli.json,
                    command,
                    &format!(
                        "`{raw}` is not a verification state (one of {})",
                        uguisu_core::archive::VerificationState::ALL
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                Exit::Usage
            })?,
        ),
    };
    Ok(ArchiveFilter {
        state,
        podcast_id,
        source_changed: false,
    })
}

/// `archive show <episode-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_show(cli: &Cli, episode_id: &str) -> Exit {
    let command = "archive show";
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client.archive_file(episode_id).await {
            Ok(file) => {
                if cli.json {
                    output::json_with_schema(&file);
                } else {
                    print!("{}", output::render_archive_file(&file));
                }
                Exit::Ok
            }
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match engine.archive_file(id).await {
        Ok(Some(file)) => {
            if cli.json {
                output::json_with_schema(&file);
            } else {
                print!("{}", output::render_archive_file(&file));
            }
            Exit::Ok
        }
        Ok(None) => {
            output::error(
                cli.json,
                command,
                &format!("episode {id} has no archived file"),
            );
            Exit::Usage
        }
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

/// `archive verify [<episode-id>] [--all] [--podcast] [--full]`.
#[allow(clippy::print_stdout)]
pub async fn run_verify(
    cli: &Cli,
    episode_id: Option<&str>,
    all: bool,
    podcast: Option<&str>,
    full: bool,
) -> Exit {
    let command = "archive verify";
    let depth = depth_of(full);
    if episode_id.is_none() && !all && podcast.is_none() {
        output::error(
            cli.json,
            command,
            "name an episode, or pass --all or --podcast <id>",
        );
        return Exit::Usage;
    }

    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match episode_id {
            Some(id) => match client.archive_verify_one(id, depth).await {
                Ok(v) => print_verified(cli, &v),
                Err(e) => client_exit(cli, command, &e),
            },
            None => match client.archive_verify_all(depth, podcast, None).await {
                Ok(s) => print_summary(cli, &s),
                Err(e) => client_exit(cli, command, &e),
            },
        };
    }

    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match episode_id {
        Some(raw) => match parse_episode_id(cli, command, raw) {
            Ok(id) => match engine.verify_episode(id, depth).await {
                Ok(v) => print_verified(cli, &v),
                Err(e) => engine_exit(cli, command, &e),
            },
            Err(e) => e,
        },
        None => match archive_filter(cli, command, podcast, None) {
            Ok(filter) => match engine.verify_all(&filter, depth).await {
                Ok(s) => print_summary(cli, &s),
                Err(e) => engine_exit(cli, command, &e),
            },
            Err(e) => e,
        },
    };
    engine.close().await;
    exit
}

/// A verification's exit code: a problem found is a finding, not a crash,
/// so it is reported as a failure the caller can act on.
const fn verify_exit(summary: &VerifySummary) -> Exit {
    if summary.has_problems() {
        Exit::Error
    } else {
        Exit::Ok
    }
}

#[allow(clippy::print_stdout)]
fn print_verified(cli: &Cli, v: &VerifiedFile) -> Exit {
    if cli.json {
        output::json_with_schema(v);
    } else {
        print!("{}", output::render_verified(v));
    }
    if v.state.is_problem() {
        Exit::Error
    } else {
        Exit::Ok
    }
}

#[allow(clippy::print_stdout)]
fn print_summary(cli: &Cli, s: &VerifySummary) -> Exit {
    if cli.json {
        output::json_with_schema(s);
    } else {
        print!("{}", output::render_verify_summary(s));
    }
    verify_exit(s)
}

/// `archive path-preview <episode-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_path_preview(cli: &Cli, episode_id: &str) -> Exit {
    let command = "archive path-preview";
    let print = |p: &PathPreview| {
        if cli.json {
            output::json_with_schema(p);
        } else {
            print!("{}", output::render_path_preview(p));
        }
        Exit::Ok
    };
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client.archive_path_preview(episode_id).await {
            Ok(p) => print(&p),
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match engine.path_preview(id).await {
        Ok(p) => print(&p),
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

/// `archive relocate [<episode-id>] [--all] [--podcast] [--dry-run]`.
#[allow(clippy::print_stdout)]
pub async fn run_relocate(
    cli: &Cli,
    episode_id: Option<&str>,
    all: bool,
    podcast: Option<&str>,
    dry_run: bool,
) -> Exit {
    let command = "archive relocate";
    if episode_id.is_none() && !all && podcast.is_none() {
        output::error(
            cli.json,
            command,
            "name an episode, or pass --all or --podcast <id>",
        );
        return Exit::Usage;
    }
    let print = |moves: &[Relocation]| {
        if cli.json {
            #[derive(serde::Serialize)]
            struct Body<'a> {
                moves: &'a [Relocation],
                dry_run: bool,
            }
            output::json_with_schema(&Body { moves, dry_run });
        } else {
            print!("{}", output::render_relocations(moves, dry_run));
        }
        Exit::Ok
    };

    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        let targets = match episode_id {
            Some(id) => vec![id.to_owned()],
            None => match client.archive_list(None, podcast, false, PAGE).await {
                Ok(files) => files.iter().map(|f| f.episode_id.to_string()).collect(),
                Err(e) => return client_exit(cli, command, &e),
            },
        };
        let mut moves = Vec::with_capacity(targets.len());
        for target in &targets {
            match client.archive_relocate(target, dry_run).await {
                Ok(m) => moves.push(m),
                Err(e) => return client_exit(cli, command, &e),
            }
        }
        return print(&moves);
    }

    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = relocate_embedded(cli, command, &engine, episode_id, podcast, dry_run, &print).await;
    engine.close().await;
    exit
}

async fn relocate_embedded(
    cli: &Cli,
    command: &str,
    engine: &Engine,
    episode_id: Option<&str>,
    podcast: Option<&str>,
    dry_run: bool,
    print: &impl Fn(&[Relocation]) -> Exit,
) -> Exit {
    let targets: Vec<EpisodeId> = if let Some(raw) = episode_id {
        match parse_episode_id(cli, command, raw) {
            Ok(id) => vec![id],
            Err(e) => return e,
        }
    } else {
        {
            let filter = match archive_filter(cli, command, podcast, None) {
                Ok(f) => f,
                Err(e) => return e,
            };
            match engine.archive_list(&filter, None, PAGE).await {
                Ok(page) => page.files.iter().map(|f| f.episode_id).collect(),
                Err(e) => return engine_exit(cli, command, &e),
            }
        }
    };
    let mut moves = Vec::with_capacity(targets.len());
    for target in targets {
        match engine.relocate(target, dry_run).await {
            Ok(m) => moves.push(m),
            // One file that cannot move must not abandon the rest: the
            // failure is reported and the run continues.
            Err(e) => output::error(cli.json, command, &format!("{target}: {e}")),
        }
    }
    print(&moves)
}

/// `archive reconcile [--deep]`.
#[allow(clippy::print_stdout)]
pub async fn run_reconcile(cli: &Cli, deep: bool) -> Exit {
    let command = "archive reconcile";
    let print = |report: &uguisu_engine::archive::ArchiveReconcileReport| {
        if cli.json {
            output::json_with_schema(report);
        } else {
            print!("{}", output::render_archive_reconcile(report));
        }
        Exit::Ok
    };
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client.archive_reconcile(deep).await {
            Ok(report) => print(&report),
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match engine.reconcile_archive(deep).await {
        Ok(report) => print(&report),
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

/// `archive policy show <podcast-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_policy_show(cli: &Cli, podcast_id: &str) -> Exit {
    let command = "archive policy show";
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client.policy_show(podcast_id).await {
            Ok(v) => print_policy(cli, &v),
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let id = match parse_podcast_id(cli, command, podcast_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match policy_json(&engine, id).await {
        Ok(v) => print_policy(cli, &v),
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

/// The same shape the API answers with, so `--json` matches either way.
async fn policy_json(
    engine: &Engine,
    podcast_id: PodcastId,
) -> Result<serde_json::Value, uguisu_core::UguisuError> {
    let stored = engine
        .policies()
        .await?
        .into_iter()
        .find(|p| p.podcast_id == podcast_id);
    let effective = engine.effective_policy(podcast_id).await?;
    Ok(serde_json::json!({
        "podcast_id": podcast_id.to_string(),
        "stored": stored,
        "effective": {
            "mode": match effective.mode {
                PolicyMode::Auto => "auto",
                PolicyMode::Manual => "manual",
            },
            "max_backlog": effective.max_backlog,
            "max_age_days": effective.max_age_days,
            "priority": effective.priority,
        },
    }))
}

#[allow(clippy::print_stdout)]
fn print_policy(cli: &Cli, value: &serde_json::Value) -> Exit {
    if cli.json {
        output::json_with_schema(value);
    } else {
        print!("{}", output::render_policy(value));
    }
    Exit::Ok
}

/// `archive policy set <podcast-id> --mode …`.
pub async fn run_policy_set(
    cli: &Cli,
    podcast_id: &str,
    mode: &str,
    max_backlog: Option<u32>,
    max_age_days: Option<u32>,
    priority: Option<&str>,
) -> Exit {
    let command = "archive policy set";
    let parsed_mode = match mode {
        "auto" => PolicyMode::Auto,
        "manual" => PolicyMode::Manual,
        other => {
            output::error(
                cli.json,
                command,
                &format!("`{other}` is not a policy mode (manual, auto)"),
            );
            return Exit::Usage;
        }
    };
    let parsed_priority = if let Some(raw) = priority {
        let Some(p) = Priority::parse(raw) else {
            output::error(
                cli.json,
                command,
                &format!("`{raw}` is not a priority (low, normal, high)"),
            );
            return Exit::Usage;
        };
        Some(p)
    } else {
        None
    };

    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        let update = serde_json::json!({
            "mode": mode,
            "max_backlog": max_backlog,
            "max_age_days": max_age_days,
            "priority": parsed_priority,
        });
        return match client.policy_set(podcast_id, &update).await {
            Ok(v) => print_policy(cli, &v),
            Err(e) => client_exit(cli, command, &e),
        };
    }

    let id = match parse_podcast_id(cli, command, podcast_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let policy = ArchivePolicy {
        podcast_id: id,
        mode: parsed_mode,
        max_backlog,
        max_age_days,
        priority: parsed_priority,
        updated_at: time::OffsetDateTime::now_utc(),
    };
    let exit = match engine.set_policy(&policy).await {
        Ok(()) => match policy_json(&engine, id).await {
            Ok(v) => print_policy(cli, &v),
            Err(e) => engine_exit(cli, command, &e),
        },
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

/// `archive policy clear <podcast-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_policy_clear(cli: &Cli, podcast_id: &str) -> Exit {
    let command = "archive policy clear";
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client.policy_clear(podcast_id).await {
            Ok(v) => {
                if cli.json {
                    output::json_with_schema(&v);
                } else {
                    println!("Policy cleared; the global defaults apply again.");
                }
                Exit::Ok
            }
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let id = match parse_podcast_id(cli, command, podcast_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match engine.clear_policy(id).await {
        Ok(cleared) => {
            if cli.json {
                output::json_with_schema(&serde_json::json!({
                    "podcast_id": id.to_string(),
                    "cleared": cleared,
                }));
            } else if cleared {
                println!("Policy cleared; the global defaults apply again.");
            } else {
                println!("This podcast had no policy of its own.");
            }
            Exit::Ok
        }
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

/// `archive policy list`.
#[allow(clippy::print_stdout)]
pub async fn run_policy_list(cli: &Cli) -> Exit {
    let command = "archive policy list";
    let print = |policies: &[ArchivePolicy]| {
        if cli.json {
            #[derive(serde::Serialize)]
            struct Body<'a> {
                policies: &'a [ArchivePolicy],
            }
            output::json_with_schema(&Body { policies });
        } else {
            print!("{}", output::render_policies(policies));
        }
        Exit::Ok
    };
    if let Some(client) = match remote(cli, command) {
        Ok(c) => c,
        Err(e) => return e,
    } {
        return match client.archive_policies().await {
            Ok(policies) => print(&policies),
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(e) => return e,
    };
    let exit = match engine.policies().await {
        Ok(policies) => print(&policies),
        Err(e) => engine_exit(cli, command, &e),
    };
    engine.close().await;
    exit
}

// Sidecars, manifests, rebuild, import, artwork and tags.
//
// Every one of these is explicit, and the two that read a tree Uguisu did
// not necessarily write - `reconcile --rebuild` and `import` - default to
// a dry run. `--apply` is how a user says they have read the plan.
//
// The remote path returns the server's JSON body as it arrived. These are
// reports the CLI renders rather than computes with, and a second set of
// mirror structs would be one more place for the wire format to drift.

/// Prints a JSON value, or a short human summary of it.
#[allow(clippy::print_stdout)]
fn print_report(cli: &Cli, value: &serde_json::Value, lines: &[(&str, &str)]) -> Exit {
    if cli.json {
        output::json_with_schema(value);
        return Exit::Ok;
    }
    for (label, key) in lines {
        if let Some(found) = value.get(*key) {
            println!("{label}: {}", render_scalar(found));
        }
    }
    Exit::Ok
}

fn render_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => match (map.get("count"), map.get("sample")) {
            (Some(count), Some(serde_json::Value::Array(sample))) if !sample.is_empty() => {
                let names: Vec<String> = sample
                    .iter()
                    .take(3)
                    .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                    .collect();
                format!("{count} ({})", names.join(", "))
            }
            (Some(count), _) => count.to_string(),
            _ => value.to_string(),
        },
        other => other.to_string(),
    }
}

/// Runs an archive-asset command against the server or the embedded engine.
async fn remote_or_engine<F, Fut>(
    cli: &Cli,
    command: &str,
    remote_call: impl AsyncFnOnce(
        &crate::client::ApiClient,
    ) -> Result<serde_json::Value, crate::client::ClientError>,
    local: F,
) -> Result<serde_json::Value, Exit>
where
    F: FnOnce(Engine) -> Fut,
    Fut:
        std::future::Future<Output = (Result<serde_json::Value, uguisu_core::UguisuError>, Engine)>,
{
    if let Some(client) = remote(cli, command)? {
        return remote_call(&client)
            .await
            .map_err(|e| client_exit(cli, command, &e));
    }
    let engine = open_engine(cli, command).await?;
    let (result, engine) = local(engine).await;
    let out = result.map_err(|e| engine_exit(cli, command, &e));
    engine.close().await;
    out
}

/// `archive sidecar show <episode-id>`.
pub async fn run_sidecar_show(cli: &Cli, episode_id: &str) -> Exit {
    let command = "archive sidecar show";
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = remote_or_engine(
        cli,
        command,
        async |client| {
            client
                .archive_get_json(&format!("api/v1/archive/{id}/sidecar"))
                .await
        },
        |engine| async move {
            let out = engine.read_sidecar(id).await.and_then(|s| {
                s.ok_or_else(|| uguisu_core::UguisuError::NotFound {
                    entity: "sidecar".to_owned(),
                    id: id.to_string(),
                })
            });
            (
                out.and_then(|s| {
                    serde_json::to_value(s)
                        .map_err(|e| uguisu_core::UguisuError::Internal(format!("sidecar: {e}")))
                }),
                engine,
            )
        },
    )
    .await;
    match result {
        Ok(value) => print_report(
            cli,
            &value,
            &[("schema", "schema"), ("generator", "generator")],
        ),
        Err(exit) => exit,
    }
}

/// `archive sidecar write <episode-id>`.
pub async fn run_sidecar_write(cli: &Cli, episode_id: &str) -> Exit {
    let command = "archive sidecar write";
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = remote_or_engine(
        cli,
        command,
        async |client| {
            client
                .archive_post_json(&format!("api/v1/archive/{id}/sidecar/write"), None)
                .await
        },
        |engine| async move {
            let out = engine.write_sidecar(id).await.map(|written| {
                serde_json::json!({
                    "episode_id": id.to_string(),
                    "path": written.map(|w| w.path),
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_report(cli, &value, &[("sidecar", "path")]),
        Err(exit) => exit,
    }
}

/// `archive manifest status`.
pub async fn run_manifest_status(cli: &Cli) -> Exit {
    let command = "archive manifest status";
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.archive_get_json("api/v1/archive/manifests").await,
        |engine| async move {
            let out = engine.manifest_status().await.and_then(|m| {
                serde_json::to_value(serde_json::json!({ "manifests": m }))
                    .map_err(|e| uguisu_core::UguisuError::Internal(format!("manifests: {e}")))
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_manifest_status(cli, &value),
        Err(exit) => exit,
    }
}

#[allow(clippy::print_stdout)]
fn print_manifest_status(cli: &Cli, value: &serde_json::Value) -> Exit {
    if cli.json {
        output::json_with_schema(value);
        return Exit::Ok;
    }
    let Some(rows) = value["manifests"].as_array() else {
        println!("no manifests");
        return Exit::Ok;
    };
    if rows.is_empty() {
        println!("no manifests yet");
        return Exit::Ok;
    }
    for row in rows {
        println!(
            "{}  {:>6} entries  {}",
            row["podcast_id"].as_str().unwrap_or("?"),
            row["entries"].as_u64().unwrap_or(0),
            if row["stale"].as_bool().unwrap_or(false) {
                "stale"
            } else {
                "current"
            }
        );
    }
    Exit::Ok
}

/// `archive manifest write [--podcast <id>]`.
pub async fn run_manifest_write(cli: &Cli, podcast: Option<&str>) -> Exit {
    let command = "archive manifest write";
    let podcast_id = match podcast {
        Some(raw) => match parse_podcast_id(cli, command, raw) {
            Ok(id) => Some(id),
            Err(e) => return e,
        },
        None => None,
    };
    let path = podcast_id.map_or_else(
        || "api/v1/archive/manifests/write".to_owned(),
        |id| format!("api/v1/archive/manifests/{id}/write"),
    );
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.archive_post_json(&path, None).await,
        |engine| async move {
            let out = match podcast_id {
                Some(id) => engine.write_manifest(id).await.map(|w| vec![w]),
                None => engine.write_stale_manifests().await,
            };
            let out = out.map(|written| {
                let rows: Vec<serde_json::Value> = written
                    .into_iter()
                    .map(|w| {
                        serde_json::json!({
                            "podcast_id": w.podcast_id.to_string(),
                            "path": w.path,
                            "entries": w.entries,
                            "cleared": w.cleared,
                        })
                    })
                    .collect();
                serde_json::json!({ "written": rows })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_manifest_written(cli, &value),
        Err(exit) => exit,
    }
}

#[allow(clippy::print_stdout)]
fn print_manifest_written(cli: &Cli, value: &serde_json::Value) -> Exit {
    if cli.json {
        output::json_with_schema(value);
        return Exit::Ok;
    }
    let rows = value["written"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("every manifest was already current");
        return Exit::Ok;
    }
    for row in rows {
        println!(
            "{}  {} entries",
            row["path"].as_str().unwrap_or("?"),
            row["entries"].as_u64().unwrap_or(0)
        );
    }
    Exit::Ok
}

/// `archive manifest verify --podcast <id> [--full]`.
pub async fn run_manifest_verify(cli: &Cli, podcast: &str, full: bool) -> Exit {
    let command = "archive manifest verify";
    let id = match parse_podcast_id(cli, command, podcast) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let path = format!("api/v1/archive/manifests/{id}/verify?rehash={full}");
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.archive_get_json(&path).await,
        |engine| async move {
            let out = engine.verify_manifest(id, full).await.map(|check| {
                serde_json::json!({
                    "podcast_id": check.podcast_id.to_string(),
                    "path": check.path,
                    "stale": check.stale,
                    "rehashed": check.rehashed,
                    "unchanged": check.diff.unchanged,
                    "clean": check.diff.is_clean(),
                    "changed": { "count": check.diff.changed.count, "sample": check.diff.changed.sample },
                    "missing": { "count": check.diff.missing.count, "sample": check.diff.missing.sample },
                    "added": { "count": check.diff.added.count, "sample": check.diff.added.sample },
                    "unreadable": { "count": check.diff.unreadable.count, "sample": check.diff.unreadable.sample },
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => {
            let clean = value["clean"].as_bool().unwrap_or(false);
            let exit = print_report(
                cli,
                &value,
                &[
                    ("manifest", "path"),
                    ("unchanged", "unchanged"),
                    ("changed", "changed"),
                    ("missing", "missing"),
                    ("added", "added"),
                    ("unreadable", "unreadable"),
                ],
            );
            // Exit 1 on a finding, like `archive verify`: a check that
            // found something must be visible to a script.
            if clean { exit } else { Exit::Error }
        }
        Err(exit) => exit,
    }
}

/// `archive orphans`: what nothing owns under the media directory (ADR 0051).
pub async fn run_orphans(cli: &Cli) -> Exit {
    let command = "archive orphans";
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.archive_orphans().await,
        |engine| async move {
            let out = engine.orphans().await.map(|r| {
                serde_json::json!({
                    "scanned": r.scanned,
                    "clean": !r.has_findings(),
                    "leftovers": { "count": r.leftovers.count, "sample": r.leftovers.sample },
                    "orphan_parts": { "count": r.orphan_parts.count, "sample": r.orphan_parts.sample },
                    "unknown_media": { "count": r.unknown_media.count, "sample": r.unknown_media.sample },
                    "stray_sidecars": { "count": r.stray_sidecars.count, "sample": r.stray_sidecars.sample },
                    "unreadable": { "count": r.unreadable.count, "sample": r.unreadable.sample },
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => {
            let exit = print_report(
                cli,
                &value,
                &[
                    ("scanned", "scanned"),
                    ("leftovers", "leftovers"),
                    ("orphan parts", "orphan_parts"),
                    ("unknown media", "unknown_media"),
                    ("stray sidecars", "stray_sidecars"),
                    ("unreadable", "unreadable"),
                ],
            );
            // Exit 1 on a finding, like `archive verify`. Nothing is removed
            // either way: what to do with a leftover is the person's call.
            if value["clean"] == true {
                exit
            } else {
                Exit::Error
            }
        }
        Err(exit) => exit,
    }
}

/// `archive reconcile --rebuild`.
pub async fn run_rebuild(cli: &Cli, apply: bool, podcast: Option<&str>) -> Exit {
    let command = "archive reconcile --rebuild";
    let podcast_id = match podcast {
        Some(raw) => match parse_podcast_id(cli, command, raw) {
            Ok(id) => Some(id),
            Err(e) => return e,
        },
        None => None,
    };
    let request = serde_json::json!({
        "apply": apply,
        "podcast": podcast_id.map(|id| id.to_string()),
    });
    let result = remote_or_engine(
        cli,
        command,
        async |client| {
            client
                .archive_post_json("api/v1/archive/rebuild", Some(request.clone()))
                .await
        },
        |engine| async move {
            let out = engine
                .rebuild_archive(&uguisu_engine::rebuild::RebuildOptions {
                    apply,
                    podcast: podcast_id,
                })
                .await
                .map(|r| {
                    serde_json::json!({
                        "applied": r.applied,
                        "scanned": r.scanned,
                        "rebuilt": r.rebuilt,
                        "unchanged": r.unchanged,
                        "conflicts": { "count": r.conflicts.count, "sample": r.conflicts.sample },
                        "unknown_episode": { "count": r.unknown_episode.count, "sample": r.unknown_episode.sample },
                        "malformed": { "count": r.malformed.count, "sample": r.malformed.sample },
                        "missing_media": { "count": r.missing_media.count, "sample": r.missing_media.sample },
                        "unreadable": { "count": r.unreadable.count, "sample": r.unreadable.sample },
                    })
                });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_report(
            cli,
            &value,
            &[
                ("applied", "applied"),
                ("scanned", "scanned"),
                ("rebuilt", "rebuilt"),
                ("unchanged", "unchanged"),
                ("conflicts", "conflicts"),
                ("unknown episode", "unknown_episode"),
                ("malformed", "malformed"),
                ("missing media", "missing_media"),
            ],
        ),
        Err(exit) => exit,
    }
}

/// `archive import <path>`.
pub async fn run_import(
    cli: &Cli,
    path: &std::path::Path,
    format: Option<&str>,
    apply: bool,
    podcast: Option<&str>,
    podgrab_db: Option<&std::path::Path>,
) -> Exit {
    let command = "archive import";
    let podcast_id = match podcast {
        Some(raw) => match parse_podcast_id(cli, command, raw) {
            Ok(id) => Some(id),
            Err(e) => return e,
        },
        None => None,
    };
    let mut chosen = None;
    if let Some(raw) = format {
        let Some(parsed) = uguisu_archive::import::ImportFormat::parse(raw) else {
            output::error(
                cli.json,
                command,
                &format!(
                    "`{raw}` is not an import format (one of {})",
                    uguisu_archive::import::ImportFormat::ALL
                        .iter()
                        .map(|f: &uguisu_archive::ImportFormat| f.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            return Exit::Usage;
        };
        chosen = Some(parsed);
    }
    let format = chosen;
    if podgrab_db.is_some() && format == Some(uguisu_archive::ImportFormat::Generic) {
        output::error(
            cli.json,
            command,
            "--podgrab-db reads Podgrab's layout; it cannot be combined with --format generic",
        );
        return Exit::Usage;
    }
    let request = serde_json::json!({
        "path": path.to_string_lossy(),
        "format": format.map(uguisu_archive::ImportFormat::as_str),
        "apply": apply,
        "podcast": podcast_id.map(|id| id.to_string()),
        "podgrab_db": podgrab_db.map(|db| db.to_string_lossy()),
    });
    let owned = path.to_path_buf();
    let owned_db = podgrab_db.map(std::path::Path::to_path_buf);
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.import_archive(&request).await,
        |engine| async move {
            let options = uguisu_engine::import::ImportOptions {
                apply,
                format,
                podcast: podcast_id,
                threshold: None,
                podgrab_db: owned_db,
            };
            let plan = if apply {
                engine.import_apply(&owned, &options).await
            } else {
                engine.import_plan(&owned, &options).await
            };
            let out = plan.map(|p| {
                serde_json::json!({
                    "applied": p.applied,
                    "source_root": p.source_root,
                    "format": p.format.as_str(),
                    "threshold": p.threshold,
                    "scanned": p.counts.scanned,
                    "imported": p.counts.imported,
                    "already_present": p.counts.already_present,
                    "conflicts": p.counts.conflicts,
                    "ambiguous": p.counts.ambiguous,
                    "unmatched": p.counts.unmatched,
                    "invalid": p.counts.invalid,
                    "unreadable": p.counts.unreadable,
                    "items": p.items.iter().map(|i| serde_json::json!({
                        "source_path": i.source_path,
                        "size_bytes": i.size_bytes,
                        "action": i.action.as_str(),
                        "podcast_id": i.podcast_id.map(|p| p.to_string()),
                        "episode_id": i.episode_id.map(|e| e.to_string()),
                        "confidence": i.confidence,
                        "matched_by": i.matched_by,
                        "target_path": i.target_path,
                        "detail": i.detail,
                    })).collect::<Vec<_>>(),
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_import(cli, &value),
        Err(exit) => exit,
    }
}

/// `archive restore <path> [--apply] [--podcast <id>]`.
pub async fn run_restore(
    cli: &Cli,
    path: &std::path::Path,
    apply: bool,
    podcast: Option<&str>,
) -> Exit {
    let command = "archive restore";
    let podcast_id = match podcast {
        Some(raw) => match parse_podcast_id(cli, command, raw) {
            Ok(id) => Some(id),
            Err(e) => return e,
        },
        None => None,
    };
    let request = serde_json::json!({
        "path": path.to_string_lossy(),
        "apply": apply,
        "podcast": podcast_id.map(|id| id.to_string()),
    });
    let owned = path.to_path_buf();
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.restore_archive(&request).await,
        |engine| async move {
            let options = uguisu_engine::restore::RestoreOptions {
                apply,
                podcast: podcast_id,
            };
            let out = engine.restore(&owned, &options).await.map(|r| {
                serde_json::json!({
                    "applied": r.applied,
                    "source_root": r.source_root,
                    "scanned": r.scanned,
                    "items": r.items.iter().map(|i| serde_json::json!({
                        "episode_id": i.episode_id.to_string(),
                        "podcast_id": i.podcast_id.to_string(),
                        "target_path": i.target_path,
                        "action": i.action.as_str(),
                        "source_path": i.source_path,
                        "detail": i.detail,
                    })).collect::<Vec<_>>(),
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_restore(cli, &value),
        Err(exit) => exit,
    }
}

#[allow(clippy::print_stdout)]
fn print_restore(cli: &Cli, value: &serde_json::Value) -> Exit {
    if cli.json {
        output::json_with_schema(value);
        return Exit::Ok;
    }
    println!(
        "{} {}",
        if value["applied"].as_bool().unwrap_or(false) {
            "restored from"
        } else {
            "would restore from"
        },
        value["source_root"].as_str().unwrap_or("?")
    );
    println!("  scanned: {}", value["scanned"].as_u64().unwrap_or(0));
    for item in value["items"].as_array().cloned().unwrap_or_default() {
        let source = item["source_path"]
            .as_str()
            .map(|s| format!(" <- {s}"))
            .unwrap_or_default();
        let detail = item["detail"]
            .as_str()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        println!(
            "  {}: {}{source}{detail}",
            item["action"].as_str().unwrap_or("?"),
            item["target_path"].as_str().unwrap_or("?")
        );
    }
    Exit::Ok
}

/// `archive redownload <episode-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_redownload(cli: &Cli, episode_id: &str) -> Exit {
    let command = "archive redownload";
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = remote_or_engine(
        cli,
        command,
        async |client| client.redownload(&id.to_string()).await,
        |engine| async move {
            let out = engine
                .downloads()
                .redownload(id, uguisu_core::download::Priority::Normal)
                .await
                .map(|o| serde_json::to_value(&o).unwrap_or_default());
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) if cli.json => {
            output::json_with_schema(&value);
            Exit::Ok
        }
        Ok(value) => match serde_json::from_value::<uguisu_download::EnqueueOutcome>(value) {
            Ok(outcome) => {
                print!("{}", output::render_enqueue(&outcome));
                Exit::Ok
            }
            Err(e) => {
                output::error(false, command, &format!("unexpected answer: {e}"));
                Exit::Error
            }
        },
        Err(exit) => exit,
    }
}

#[allow(clippy::print_stdout)]
fn print_import(cli: &Cli, value: &serde_json::Value) -> Exit {
    if cli.json {
        output::json_with_schema(value);
        return Exit::Ok;
    }
    println!(
        "{} {} as {}",
        if value["applied"].as_bool().unwrap_or(false) {
            "imported from"
        } else {
            "would import from"
        },
        value["source_root"].as_str().unwrap_or("?"),
        value["format"].as_str().unwrap_or("?")
    );
    for key in [
        "scanned",
        "imported",
        "already_present",
        "conflicts",
        "ambiguous",
        "unmatched",
        "invalid",
        "unreadable",
    ] {
        println!("  {key}: {}", value[key].as_u64().unwrap_or(0));
    }
    for item in value["items"].as_array().cloned().unwrap_or_default() {
        let action = item["action"].as_str().unwrap_or("?");
        // A plain import is the expected line; one with a note needs reading.
        if action == "already_present" || (action == "import" && item["detail"].is_null()) {
            continue;
        }
        println!(
            "  {action}: {} ({})",
            item["source_path"].as_str().unwrap_or("?"),
            item["detail"].as_str().unwrap_or("")
        );
    }
    Exit::Ok
}

/// `archive artwork show <podcast-id>` and `... fetch <podcast-id>`.
pub async fn run_artwork(cli: &Cli, podcast: &str, fetch: bool, force: bool) -> Exit {
    let command = if fetch {
        "archive artwork fetch"
    } else {
        "archive artwork show"
    };
    let id = match parse_podcast_id(cli, command, podcast) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = remote_or_engine(
        cli,
        command,
        async |client| {
            if fetch {
                client
                    .archive_post_json(
                        &format!("api/v1/podcasts/{id}/artwork/fetch"),
                        Some(serde_json::json!({ "force": force })),
                    )
                    .await
            } else {
                client
                    .archive_get_json(&format!("api/v1/podcasts/{id}/artwork"))
                    .await
            }
        },
        |engine| async move {
            let out = if fetch {
                engine.fetch_artwork(id, force).await.map(|outcome| {
                    let state = outcome.state();
                    let artwork = match outcome {
                        uguisu_engine::artwork::ArtworkOutcome::Fetched(a) => {
                            serde_json::to_value(*a).unwrap_or(serde_json::Value::Null)
                        }
                        _ => serde_json::Value::Null,
                    };
                    serde_json::json!({ "state": state, "artwork": artwork })
                })
            } else {
                engine.current_artwork(id).await.map(|current| {
                    serde_json::json!({
                        "state": if current.is_some() { "stored" } else { "none" },
                        "current": current,
                    })
                })
            };
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_report(
            cli,
            &value,
            &[
                ("state", "state"),
                ("path", "artwork"),
                ("current", "current"),
            ],
        ),
        Err(exit) => exit,
    }
}

/// `archive tags show <episode-id>`.
pub async fn run_tags_show(cli: &Cli, episode_id: &str) -> Exit {
    let command = "archive tags show";
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let result = remote_or_engine(
        cli,
        command,
        async |client| {
            client
                .archive_get_json(&format!("api/v1/archive/{id}/tags"))
                .await
        },
        |engine| async move {
            let out = engine.read_episode_tags(id).await.map(|tags| {
                let values: std::collections::BTreeMap<String, String> = tags
                    .values
                    .into_iter()
                    .map(|(k, v)| (k.as_str().to_owned(), v))
                    .collect();
                serde_json::json!({
                    "values": values,
                    "has_cover": tags.cover.is_some(),
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_tags(cli, &value),
        Err(exit) => exit,
    }
}

#[allow(clippy::print_stdout)]
fn print_tags(cli: &Cli, value: &serde_json::Value) -> Exit {
    if cli.json {
        output::json_with_schema(value);
        return Exit::Ok;
    }
    match value["values"].as_object() {
        Some(map) if !map.is_empty() => {
            for (key, v) in map {
                println!("{key}: {}", v.as_str().unwrap_or_default());
            }
        }
        _ => println!("the file carries none of the fields Uguisu manages"),
    }
    println!(
        "cover: {}",
        if value["has_cover"].as_bool().unwrap_or(false) {
            "yes"
        } else {
            "no"
        }
    );
    Exit::Ok
}

/// `archive tags write <episode-id> [--mode ...]`.
pub async fn run_tags_write(cli: &Cli, episode_id: &str, mode: Option<&str>) -> Exit {
    let command = "archive tags write";
    let id = match parse_episode_id(cli, command, episode_id) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let mut parsed = None;
    if let Some(raw) = mode {
        let Some(chosen) = uguisu_core::archive::TagMode::parse(&raw.to_ascii_lowercase()) else {
            output::error(
                cli.json,
                command,
                &format!(
                    "`{raw}` is not a tag mode (one of {})",
                    uguisu_core::archive::TagMode::ALL
                        .iter()
                        .map(|m: &uguisu_core::archive::TagMode| m.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            return Exit::Usage;
        };
        parsed = Some(chosen);
    }
    let request = serde_json::json!({ "mode": parsed.map(uguisu_core::archive::TagMode::as_str) });
    let result = remote_or_engine(
        cli,
        command,
        async |client| {
            client
                .archive_post_json(
                    &format!("api/v1/archive/{id}/tags/write"),
                    Some(request.clone()),
                )
                .await
        },
        |engine| async move {
            let mode = parsed.unwrap_or(engine.config().archive.tag_mode);
            let out = engine.write_episode_tags(id, mode).await.map(|r| {
                serde_json::json!({
                    "episode_id": r.episode_id.to_string(),
                    "path": r.path,
                    "state": r.state,
                    "mode": r.mode.as_str(),
                    "fields": r.fields,
                    "not_embeddable": r.not_embeddable,
                    "hash_value": r.hash_value,
                    "cover_written": r.cover_written,
                    "detail": r.detail,
                })
            });
            (out, engine)
        },
    )
    .await;
    match result {
        Ok(value) => print_report(
            cli,
            &value,
            &[
                ("file", "path"),
                ("state", "state"),
                ("mode", "mode"),
                ("hash", "hash_value"),
            ],
        ),
        Err(exit) => exit,
    }
}
