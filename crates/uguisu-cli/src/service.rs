//! The scheduler, a podcast's place in it, persisted
//! settings and local search.
//!
//! All of them run against the local data directory, like the rest of the
//! CLI. When `uguisu serve` holds the lock, they say so and point at
//! `--server`, which is the same answer every other command gives.

#![allow(clippy::print_stdout)]

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uguisu_core::UguisuError;
use uguisu_core::ids::PodcastId;
use uguisu_engine::search::{SearchOutcome, SearchRequest};

use crate::output::{self, Exit};
use crate::{Cli, open_engine};

const CMD: &str = "scheduler";

/// `health`: asks `--server`, or the server `serve` would bind, whether it is
/// up. It needs no credential and no data directory, which is what a
/// container's health check has.
pub async fn run_health(cli: &Cli, bind: std::net::SocketAddr) -> Exit {
    let command = "health";
    let server = cli.server.clone().unwrap_or_else(|| {
        let ip = match bind.ip() {
            std::net::IpAddr::V4(ip) if ip.is_unspecified() => std::net::Ipv4Addr::LOCALHOST.into(),
            std::net::IpAddr::V6(ip) if ip.is_unspecified() => std::net::Ipv6Addr::LOCALHOST.into(),
            ip => ip,
        };
        format!("http://{}", std::net::SocketAddr::new(ip, bind.port()))
    });
    let answer = match crate::client::ApiClient::new(&server, None) {
        Ok(client) => client.health().await,
        Err(e) => Err(e),
    };
    match answer {
        Ok(body) if body["status"] == "ok" => {
            let version = body["version"].as_str().unwrap_or("?");
            if cli.json {
                output::json_with_schema(
                    &serde_json::json!({ "status": "ok", "version": version }),
                );
            } else {
                println!("Healthy: Uguisu {version} at {server}");
            }
            Exit::Ok
        }
        Ok(body) => {
            output::error(cli.json, command, &format!("{server} answered {body}"));
            Exit::Error
        }
        Err(e) => {
            output::error(cli.json, command, &format!("{server}: {e}"));
            Exit::Error
        }
    }
}

/// `scheduler status`.
pub async fn run_status(cli: &Cli) -> Exit {
    let Ok(engine) = open_engine(cli, CMD).await else {
        return Exit::Error;
    };
    let result = engine.scheduler_status().await;
    engine.close().await;
    match result {
        Ok(status) => {
            if cli.json {
                output::json_with_schema(&status);
            } else {
                print!("{}", output::render_scheduler(&status));
            }
            Exit::Ok
        }
        Err(e) => fail(cli, CMD, &e),
    }
}

/// `scheduler pause [--reason]` / `scheduler resume`.
pub async fn run_pause(cli: &Cli, pause: bool, reason: Option<&str>) -> Exit {
    let Ok(engine) = open_engine(cli, CMD).await else {
        return Exit::Error;
    };
    let result = if pause {
        engine.pause_scheduler(reason).await
    } else {
        engine.resume_scheduler().await
    };
    engine.close().await;
    match result {
        Ok(control) => {
            if cli.json {
                output::json_with_schema(&control);
            } else if control.paused {
                println!(
                    "scheduler paused{}",
                    control
                        .paused_reason
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                );
            } else {
                println!("scheduler running");
            }
            Exit::Ok
        }
        Err(e) => fail(cli, CMD, &e),
    }
}

/// `scheduler run`: one pass now, without waiting for the loop.
pub async fn run_pass(cli: &Cli) -> Exit {
    let Ok(engine) = open_engine(cli, CMD).await else {
        return Exit::Error;
    };
    let config = engine.config();
    let result = engine
        .run_scheduler_pass(config.feed.refresh_concurrency)
        .await;
    // The pass returns as soon as the refreshes are *started*. Closing the
    // engine cancels them, so they are waited for first -- otherwise this
    // command would report what it began and undo it on the way out.
    engine
        .wait_for_refreshes(config.download.shutdown_grace)
        .await;
    engine.close().await;
    match result {
        Ok(pass) => {
            if cli.json {
                output::json_with_schema(&serde_json::json!({
                    "due": pass.due,
                    "started": pass.started,
                    "paused": pass.paused,
                }));
            } else if pass.paused {
                println!("scheduler is paused; nothing started");
            } else {
                println!("{} due, {} started", pass.due, pass.started);
            }
            Exit::Ok
        }
        Err(e) => fail(cli, CMD, &e),
    }
}

/// `scheduler maintenance`: the housekeeping pass, now.
pub async fn run_maintenance(cli: &Cli) -> Exit {
    let Ok(engine) = open_engine(cli, CMD).await else {
        return Exit::Error;
    };
    let result = engine.run_maintenance().await;
    engine.close().await;
    match result {
        Ok(report) => {
            if cli.json {
                output::json_with_schema(&report);
            } else {
                println!(
                    "pruned {} events, expired {} cache rows, closed {} sessions",
                    report.events_pruned, report.cache_expired, report.sessions_pruned
                );
            }
            Exit::Ok
        }
        Err(e) => fail(cli, CMD, &e),
    }
}

/// `podcast pause|resume <id>`.
pub async fn run_podcast_pause(cli: &Cli, id: &str, pause: bool) -> Exit {
    let command = if pause {
        "podcast pause"
    } else {
        "podcast resume"
    };
    let Some(id) = parse_id(cli, command, id) else {
        return Exit::Usage;
    };
    let Ok(engine) = open_engine(cli, command).await else {
        return Exit::Error;
    };
    let result = if pause {
        engine.pause_podcast(id).await
    } else {
        engine.resume_podcast(id).await
    };
    engine.close().await;
    match result {
        Ok(status) => {
            if cli.json {
                output::json_with_schema(&serde_json::json!({
                    "podcast_id": id.to_string(),
                    "status": status.as_str(),
                }));
            } else {
                println!("{id} is {}", status.as_str());
            }
            Exit::Ok
        }
        Err(e) => fail(cli, command, &e),
    }
}

/// `podcast schedule <id> [--at RFC3339|--now]`.
pub async fn run_podcast_schedule(cli: &Cli, id: &str, at: Option<&str>) -> Exit {
    const COMMAND: &str = "podcast schedule";
    let Some(id) = parse_id(cli, COMMAND, id) else {
        return Exit::Usage;
    };
    let at = match at.map(|raw| (raw, OffsetDateTime::parse(raw, &Rfc3339))) {
        Some((_, Ok(at))) => Some(at),
        Some((raw, Err(_))) => {
            output::error(
                cli.json,
                COMMAND,
                &format!("`{raw}` is not an RFC 3339 timestamp"),
            );
            return Exit::Usage;
        }
        None => None,
    };
    let Ok(engine) = open_engine(cli, COMMAND).await else {
        return Exit::Error;
    };
    let result = engine.reschedule_podcast(id, at).await;
    engine.close().await;
    match result {
        Ok(()) => {
            if cli.json {
                output::json_with_schema(&serde_json::json!({
                    "podcast_id": id.to_string(),
                    "next_refresh_at": at.map(|a| a.format(&Rfc3339).unwrap_or_default()),
                }));
            } else {
                match at {
                    Some(at) => println!("{id} is next due {at}"),
                    None => println!("{id} is due as soon as the scheduler looks"),
                }
            }
            Exit::Ok
        }
        Err(e) => fail(cli, COMMAND, &e),
    }
}

/// `config list|get|set|unset|validate`.
pub async fn run_config(cli: &Cli, action: ConfigAction<'_>) -> Exit {
    const COMMAND: &str = "config";
    let Ok(engine) = open_engine(cli, COMMAND).await else {
        return Exit::Error;
    };
    let exit = match action {
        ConfigAction::List | ConfigAction::Validate => {
            let report = engine.settings_report();
            if cli.json {
                output::json_with_schema(&report);
            } else {
                print!("{}", output::render_settings(&report));
            }
            // `validate` is the one that answers with its exit code: a
            // stored value Uguisu had to ignore, whether it does not parse
            // or something wins over it, is a configuration the operator
            // wanted and is not getting.
            if matches!(action, ConfigAction::Validate) && report.has_problems() {
                Exit::Usage
            } else {
                Exit::Ok
            }
        }
        ConfigAction::Get { key } => {
            let report = engine.settings_report();
            if let Some(described) = report.keys.iter().find(|d| d.key == key) {
                if cli.json {
                    output::json_with_schema(described);
                } else {
                    println!(
                        "{} = {}  ({})",
                        described.key, described.value, described.origin
                    );
                }
                Exit::Ok
            } else {
                output::error(cli.json, COMMAND, &format!("unknown setting `{key}`"));
                Exit::Usage
            }
        }
        ConfigAction::Set { key, value } => match engine.set_setting(key, value, Some("cli")).await
        {
            Ok(described) => {
                if cli.json {
                    output::json_with_schema(&described);
                } else {
                    println!(
                        "{} = {}  ({})",
                        described.key, described.value, described.origin
                    );
                    if !described.live {
                        println!("restart uguisu for this to take effect");
                    }
                }
                Exit::Ok
            }
            Err(e) => fail(cli, COMMAND, &e),
        },
        ConfigAction::Unset { key } => match engine.unset_setting(key, Some("cli")).await {
            Ok(cleared) => {
                if cli.json {
                    output::json_with_schema(&serde_json::json!({
                        "key": key,
                        "cleared": cleared,
                    }));
                } else if cleared {
                    println!("{key} cleared");
                } else {
                    println!("{key} was not stored");
                }
                Exit::Ok
            }
            Err(e) => fail(cli, COMMAND, &e),
        },
    };
    engine.close().await;
    exit
}

/// What `config` was asked to do.
#[derive(Debug, Clone, Copy)]
pub enum ConfigAction<'a> {
    /// Show every key.
    List,
    /// Show every key and fail when a stored value is being ignored.
    Validate,
    /// Show one key.
    Get {
        /// The `UGUISU_*` name.
        key: &'a str,
    },
    /// Store a value.
    Set {
        /// The `UGUISU_*` name.
        key: &'a str,
        /// The value, in environment-variable syntax.
        value: &'a str,
    },
    /// Clear a stored value.
    Unset {
        /// The `UGUISU_*` name.
        key: &'a str,
    },
}

/// `search library <text>`.
pub async fn run_search(cli: &Cli, text: &str, limit: u32, prefix: bool, explain: bool) -> Exit {
    const COMMAND: &str = "search library";
    let Ok(engine) = open_engine(cli, COMMAND).await else {
        return Exit::Error;
    };
    let request = SearchRequest {
        text: text.to_owned(),
        limit,
        prefix,
        ..SearchRequest::default()
    };
    let result = engine.search_library(&request).await;
    engine.close().await;
    match result {
        Ok(results) => {
            let exit = match results.outcome {
                SearchOutcome::Ok => Exit::Ok,
                // The discovery convention: nothing found is 3, whatever
                // the reason, and the reason is printed.
                SearchOutcome::NoResults
                | SearchOutcome::EmptyQuery
                | SearchOutcome::IndexBuilding
                | SearchOutcome::IndexStale => Exit::NoResults,
            };
            if cli.json {
                output::json_with_schema(&results);
            } else {
                print!("{}", output::render_library_search(&results, explain));
            }
            exit
        }
        Err(e) => fail(cli, COMMAND, &e),
    }
}

/// `search reindex`.
pub async fn run_reindex(cli: &Cli) -> Exit {
    const COMMAND: &str = "search reindex";
    let Ok(engine) = open_engine(cli, COMMAND).await else {
        return Exit::Error;
    };
    let result = engine.reindex_search().await;
    engine.close().await;
    match result {
        Ok(report) => {
            if cli.json {
                output::json_with_schema(&report);
            } else {
                println!(
                    "indexed {} podcasts and {} episodes in {} ms",
                    report.podcasts, report.episodes, report.duration_ms
                );
            }
            Exit::Ok
        }
        Err(e) => fail(cli, COMMAND, &e),
    }
}

fn parse_id(cli: &Cli, command: &str, raw: &str) -> Option<PodcastId> {
    let Ok(id) = raw.parse() else {
        output::error(cli.json, command, &format!("`{raw}` is not a podcast id"));
        return None;
    };
    Some(id)
}

fn fail(cli: &Cli, command: &str, e: &UguisuError) -> Exit {
    output::error(cli.json, command, &e.to_string());
    Exit::for_engine_error(e)
}
