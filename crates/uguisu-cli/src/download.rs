//! Handlers for the `download` command group.
//!
//! `download <episode-id>` queues one episode; `download podcast <id>` a
//! whole podcast. Embedded commands never start a worker unless asked:
//! `--wait` runs the workers until that job stops and `download run` runs
//! them until the queue is idle (or the process is told to stop).
//! `--server` sends the same operations to a running server, whose own
//! workers do the downloading; `--wait` then polls the job.

use std::time::Duration;

use uguisu_core::Event;
use uguisu_core::download::{DownloadJob, DownloadState, PauseAllReason, Priority};
use uguisu_core::events::EventKind;
use uguisu_core::ids::{EpisodeId, JobId};
use uguisu_download::{DownloadStats, EnqueueOutcome, JobDetail, JobFilter};
use uguisu_engine::Engine;

use crate::library::{client_exit, engine_exit, parse_podcast_id, remote};
use crate::output::{self, Exit};
use crate::{Cli, open_engine};

/// Which job a download event is about and whether the job stopped with it.
fn event_job(kind: &EventKind) -> Option<(JobId, bool)> {
    match kind {
        EventKind::DownloadQueued { job_id, .. }
        | EventKind::DownloadStarted { job_id, .. }
        | EventKind::DownloadProgress { job_id, .. }
        | EventKind::DownloadResumed { job_id, .. }
        | EventKind::DownloadRetryScheduled { job_id, .. } => Some((*job_id, false)),
        EventKind::DownloadPaused { job_id, .. }
        | EventKind::DownloadCompleted { job_id, .. }
        | EventKind::DownloadFailed { job_id, .. }
        | EventKind::DownloadCancelled { job_id, .. } => Some((*job_id, true)),
        _ => None,
    }
}

fn parse_priority(raw: &str) -> Priority {
    Priority::parse(raw).unwrap_or(Priority::Normal)
}

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

fn parse_job_id(cli: &Cli, command: &str, raw: &str) -> Result<JobId, Exit> {
    raw.trim().parse().map_err(|_| {
        output::error(
            cli.json,
            command,
            &format!("`{raw}` is not a download job id (see `uguisu download list`)"),
        );
        Exit::Usage
    })
}

/// Resolves when the process is asked to stop (Ctrl-C everywhere, SIGTERM
/// on Unix, Ctrl-Break on Windows).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    // Windows has no SIGTERM; a supervisor stops a console process with
    // Ctrl-Break, the one console event it can send to a single process group.
    #[cfg(windows)]
    let terminate = async {
        match tokio::signal::windows::ctrl_break() {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(any(unix, windows)))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[allow(clippy::print_stdout)]
fn print_enqueue(cli: &Cli, outcome: &EnqueueOutcome) {
    if cli.json {
        output::json_with_schema(outcome);
    } else {
        print!("{}", output::render_enqueue(outcome));
    }
}

#[allow(clippy::print_stdout)]
fn print_detail(cli: &Cli, detail: &JobDetail) -> Exit {
    if cli.json {
        output::json_with_schema(detail);
    } else {
        print!("{}", output::render_job(detail));
    }
    Exit::for_download_job(&detail.job)
}

#[allow(clippy::print_stderr)]
fn print_progress(cli: &Cli, done: u64, total: Option<u64>, speed: u64, eta: Option<u64>) {
    if !cli.json {
        eprint!(
            "\r\x1b[2K  {}",
            output::progress_line(done, total, speed, eta)
        );
    }
}

#[allow(clippy::print_stderr)]
fn end_progress(cli: &Cli) {
    if !cli.json {
        eprintln!();
    }
}

/// Why a job that already exists cannot be waited for as it stands.
fn cannot_wait(cli: &Cli, command: &str, job: &DownloadJob, stats: &DownloadStats) -> Option<Exit> {
    if let Some(reason) = stats.paused_all {
        output::error(
            cli.json,
            command,
            &format!(
                "downloads are paused ({}); run `uguisu download resume-all` first",
                reason.as_str()
            ),
        );
        return Some(match reason {
            PauseAllReason::DiskFull => Exit::DiskFull,
            PauseAllReason::User => Exit::Usage,
        });
    }
    if job.state == DownloadState::Paused {
        output::error(
            cli.json,
            command,
            &format!(
                "job {} is paused ({}); run `uguisu download resume {}` first",
                job.id,
                job.state_reason.as_deref().unwrap_or("user"),
                job.id
            ),
        );
        return Some(if job.state_reason.as_deref() == Some("disk_full") {
            Exit::DiskFull
        } else {
            Exit::Usage
        });
    }
    None
}

/// `download <episode-id> [--priority p] [--wait]` and its alias
/// `episode download`.
#[allow(clippy::print_stderr)]
pub async fn run_enqueue(cli: &Cli, episode_id: &str, priority: &str, wait: bool) -> Exit {
    const CMD: &str = "download";
    let priority = parse_priority(priority);
    match remote(cli, CMD) {
        Err(exit) => exit,
        Ok(Some(client)) => {
            let outcome = match client.enqueue(episode_id.trim(), priority).await {
                Ok(o) => o,
                Err(e) => return client_exit(cli, CMD, &e),
            };
            if !wait || matches!(outcome, EnqueueOutcome::AlreadyCompleted(_)) {
                print_enqueue(cli, &outcome);
                return Exit::Ok;
            }
            let job_id = outcome.job().id.to_string();
            let stats = match client.download_stats().await {
                Ok(s) => s,
                Err(e) => return client_exit(cli, CMD, &e),
            };
            if let Some(exit) = cannot_wait(cli, CMD, outcome.job(), &stats) {
                return exit;
            }
            // The server's workers download; poll until the job stops.
            let detail = loop {
                let detail = match client.download(&job_id).await {
                    Ok(d) => d,
                    Err(e) => return client_exit(cli, CMD, &e),
                };
                match detail.job.state {
                    DownloadState::Completed
                    | DownloadState::Failed
                    | DownloadState::Cancelled
                    | DownloadState::Paused => break detail,
                    _ => {}
                }
                if let Some(p) = &detail.progress {
                    print_progress(
                        cli,
                        p.bytes_downloaded,
                        p.total_bytes,
                        p.speed_bps,
                        p.eta_secs,
                    );
                }
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    () = shutdown_signal() => {
                        end_progress(cli);
                        output::error(cli.json, CMD, "interrupted; the server keeps downloading");
                        return Exit::Error;
                    }
                }
            };
            end_progress(cli);
            print_detail(cli, &detail)
        }
        Ok(None) => {
            let id = match parse_episode_id(cli, CMD, episode_id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            // Subscribe before anything can happen so no event is missed.
            let mut sub = engine.subscribe();
            let outcome = match engine.downloads().enqueue_episode(id, priority).await {
                Ok(o) => o,
                Err(e) => {
                    let exit = engine_exit(cli, CMD, &e);
                    engine.close().await;
                    return exit;
                }
            };
            if !wait || matches!(outcome, EnqueueOutcome::AlreadyCompleted(_)) {
                print_enqueue(cli, &outcome);
                engine.close().await;
                return Exit::Ok;
            }
            let stats = match engine.downloads().stats().await {
                Ok(s) => s,
                Err(e) => {
                    let exit = engine_exit(cli, CMD, &e);
                    engine.close().await;
                    return exit;
                }
            };
            if let Some(exit) = cannot_wait(cli, CMD, outcome.job(), &stats) {
                engine.close().await;
                return exit;
            }
            let job_id = outcome.job().id;
            engine.start_downloads();
            let exit = wait_for_job(cli, &engine, &mut sub, job_id, CMD).await;
            engine.close().await;
            exit
        }
    }
}

/// Follows one job's events until it stops; Ctrl-C parks it and exits 1.
async fn wait_for_job(
    cli: &Cli,
    engine: &Engine,
    sub: &mut uguisu_engine::Subscription,
    job_id: JobId,
    command: &str,
) -> Exit {
    let signal = shutdown_signal();
    tokio::pin!(signal);
    loop {
        tokio::select! {
            () = &mut signal => {
                end_progress(cli);
                output::error(cli.json, command, "interrupted; the job stays queued");
                return Exit::Error;
            }
            event = sub.recv() => {
                let Some(event) = event else { break };
                let Some((id, terminal)) = event_job(&event.kind) else { continue };
                if id != job_id {
                    continue;
                }
                if let EventKind::DownloadProgress {
                    bytes_downloaded,
                    total_bytes,
                    speed_bps,
                    eta_secs,
                    ..
                } = &event.kind
                {
                    print_progress(cli, *bytes_downloaded, *total_bytes, *speed_bps, *eta_secs);
                }
                if terminal {
                    break;
                }
            }
        }
    }
    end_progress(cli);
    match engine.downloads().job(job_id).await {
        Ok(detail) => print_detail(cli, &detail),
        Err(e) => engine_exit(cli, command, &e),
    }
}

/// `download podcast <id> [--priority p]`.
#[allow(clippy::print_stdout)]
pub async fn run_enqueue_podcast(cli: &Cli, podcast_id: &str, priority: &str) -> Exit {
    const CMD: &str = "download podcast";
    let priority = parse_priority(priority);
    let summary = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.enqueue_podcast(podcast_id.trim(), priority).await {
            Ok(s) => s,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let id = match parse_podcast_id(cli, CMD, podcast_id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.downloads().enqueue_podcast(id, priority).await;
            engine.close().await;
            match result {
                Ok(s) => s,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&summary);
    } else {
        print!("{}", output::render_summary(&summary));
    }
    Exit::Ok
}

/// `download list [--state s] [--podcast id] [--after job] [--limit n]`.
#[allow(clippy::print_stdout)]
pub async fn run_list(
    cli: &Cli,
    state: Option<&str>,
    podcast: Option<&str>,
    after: Option<&str>,
    limit: u32,
) -> Exit {
    const CMD: &str = "download list";
    let page = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.downloads(state, podcast, after, limit).await {
            Ok(p) => p,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let state = match state {
                None => None,
                Some(raw) => {
                    let Some(parsed) = DownloadState::parse(raw.trim()) else {
                        output::error(
                            cli.json,
                            CMD,
                            &format!(
                                "`{raw}` is not a download state (one of {})",
                                DownloadState::ALL
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        );
                        return Exit::Usage;
                    };
                    Some(parsed)
                }
            };
            let podcast_id = match podcast {
                None => None,
                Some(raw) => match parse_podcast_id(cli, CMD, raw) {
                    Ok(id) => Some(id),
                    Err(exit) => return exit,
                },
            };
            let after = match after {
                None => None,
                Some(raw) => match parse_job_id(cli, CMD, raw) {
                    Ok(id) => Some(id),
                    Err(exit) => return exit,
                },
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine
                .downloads()
                .list(&JobFilter {
                    state,
                    podcast_id,
                    after,
                    limit,
                })
                .await;
            engine.close().await;
            match result {
                Ok(p) => p,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&page);
    } else {
        print!(
            "{}",
            output::render_jobs(
                // The CLI's columns are the job's; a summary is a superset,
                // and widening the table is not this phase's business.
                &page.jobs.iter().map(|j| j.job.clone()).collect::<Vec<_>>(),
                page.next_after.map(|a| a.to_string()).as_deref()
            )
        );
    }
    Exit::Ok
}

/// `download show <job-id>`.
pub async fn run_show(cli: &Cli, id: &str) -> Exit {
    const CMD: &str = "download show";
    let detail = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.download(id.trim()).await {
            Ok(d) => d,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let id = match parse_job_id(cli, CMD, id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.downloads().job(id).await;
            engine.close().await;
            match result {
                Ok(d) => d,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    print_detail(cli, &detail);
    Exit::Ok
}

/// `download cancel|pause|resume|retry <job-id>`.
#[allow(clippy::print_stdout)]
pub async fn run_job_command(cli: &Cli, command: &str, id: &str) -> Exit {
    let cmd = format!("download {command}");
    let job = match remote(cli, &cmd) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.download_command(id.trim(), command).await {
            Ok(j) => j,
            Err(e) => return client_exit(cli, &cmd, &e),
        },
        Ok(None) => {
            let id = match parse_job_id(cli, &cmd, id) {
                Ok(id) => id,
                Err(exit) => return exit,
            };
            let engine = match open_engine(cli, &cmd).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let svc = engine.downloads();
            let result = match command {
                "cancel" => svc.cancel(id).await,
                "pause" => svc.pause(id).await,
                "resume" => svc.resume(id).await,
                _ => svc.retry(id).await,
            };
            engine.close().await;
            match result {
                Ok(j) => j,
                Err(e) => return engine_exit(cli, &cmd, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "job": job }));
    } else {
        println!(
            "Job {} is now {}{}.",
            job.id,
            job.state.as_str(),
            job.state_reason
                .as_deref()
                .map_or_else(String::new, |r| format!(" ({r})"))
        );
    }
    Exit::Ok
}

/// `download retry-failed`.
#[allow(clippy::print_stdout)]
pub async fn run_retry_failed(cli: &Cli) -> Exit {
    const CMD: &str = "download retry-failed";
    let requeued = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.retry_failed().await {
            Ok(n) => n,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.downloads().retry_failed().await;
            engine.close().await;
            match result {
                Ok(n) => n,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "requeued": requeued }));
    } else {
        println!("Re-queued {requeued} failed job(s).");
    }
    Exit::Ok
}

/// `download pause-all` / `download resume-all`.
#[allow(clippy::print_stdout)]
pub async fn run_control(cli: &Cli, pause: bool) -> Exit {
    let cmd = if pause {
        "download pause-all"
    } else {
        "download resume-all"
    };
    let control = match remote(cli, cmd) {
        Err(exit) => return exit,
        Ok(Some(client)) => {
            match client
                .download_control(if pause { "pause" } else { "resume" })
                .await
            {
                Ok(c) => c,
                Err(e) => return client_exit(cli, cmd, &e),
            }
        }
        Ok(None) => {
            let engine = match open_engine(cli, cmd).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = if pause {
                engine.downloads().pause_all(PauseAllReason::User).await
            } else {
                engine.downloads().resume_all().await
            };
            engine.close().await;
            match result {
                Ok(c) => c,
                Err(e) => return engine_exit(cli, cmd, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({ "control": control }));
    } else {
        print!("{}", output::render_control(&control));
    }
    Exit::Ok
}

/// `download stats`.
#[allow(clippy::print_stdout)]
pub async fn run_stats(cli: &Cli) -> Exit {
    const CMD: &str = "download stats";
    let stats = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.download_stats().await {
            Ok(s) => s,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.downloads().stats().await;
            engine.close().await;
            match result {
                Ok(s) => s,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&stats);
    } else {
        print!("{}", output::render_stats(&stats));
    }
    Exit::Ok
}

/// `download reconcile [--deep]`.
#[allow(clippy::print_stdout)]
pub async fn run_reconcile(cli: &Cli, deep: bool) -> Exit {
    const CMD: &str = "download reconcile";
    let report = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.reconcile(deep).await {
            Ok(r) => r,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.downloads().reconcile(deep).await;
            engine.close().await;
            match result {
                Ok(r) => r,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&report);
    } else {
        print!("{}", output::render_reconcile(&report));
    }
    Exit::Ok
}

/// `download run [--until-idle]`: runs the workers in this process until
/// the queue is idle (`--until-idle`) or the process is told to stop.
/// Embedded only: a server runs its own workers.
#[allow(clippy::print_stdout, clippy::print_stderr, clippy::too_many_lines)]
pub async fn run_workers(cli: &Cli, until_idle: bool) -> Exit {
    const CMD: &str = "download run";
    if cli.server.is_some() {
        output::error(
            cli.json,
            CMD,
            "`download run` is embedded only; a running server already downloads its queue",
        );
        return Exit::Usage;
    }
    let engine = match open_engine(cli, CMD).await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let mut sub = engine.subscribe();
    engine.start_downloads();
    let signal = shutdown_signal();
    tokio::pin!(signal);
    let idle = async {
        if until_idle {
            engine.downloads().wait_idle().await
        } else {
            std::future::pending::<Result<(), uguisu_core::UguisuError>>().await
        }
    };
    tokio::pin!(idle);
    let mut exits = Vec::new();
    let mut completed = 0u32;
    let mut failed = 0u32;
    let mut interrupted = false;
    loop {
        tokio::select! {
            () = &mut signal => {
                interrupted = true;
                if !cli.json {
                    eprintln!("stopping; running jobs are parked as queued(shutdown)");
                }
                break;
            }
            result = &mut idle => {
                if let Err(e) = result {
                    exits.push(engine_exit(cli, CMD, &e));
                }
                break;
            }
            event = sub.recv() => {
                let Some(event) = event else { break };
                report_event(cli, &engine, &event, &mut exits, &mut completed, &mut failed).await;
            }
        }
    }
    let stats = engine.downloads().stats().await;
    engine.close().await;
    let stats = match stats {
        Ok(s) => s,
        Err(e) => return engine_exit(cli, CMD, &e),
    };
    if cli.json {
        output::json_with_schema(&serde_json::json!({
            "completed": completed,
            "failed": failed,
            "interrupted": interrupted,
            "stats": stats,
        }));
    } else {
        print!("{}", output::render_stats(&stats));
        println!("This run: {completed} completed, {failed} failed.");
    }
    Exit::worst(exits)
}

#[allow(clippy::print_stdout)]
async fn report_event(
    cli: &Cli,
    engine: &Engine,
    event: &Event,
    exits: &mut Vec<Exit>,
    completed: &mut u32,
    failed: &mut u32,
) {
    match &event.kind {
        EventKind::DownloadCompleted { job_id, path, .. } => {
            *completed += 1;
            if !cli.json {
                println!("completed  {job_id}  {path}");
            }
        }
        EventKind::DownloadFailed {
            job_id,
            reason,
            error_kind,
            detail,
            ..
        } => {
            *failed += 1;
            if !cli.json {
                println!("failed     {job_id}  {reason} ({error_kind}): {detail}");
            }
            if let Ok(detail) = engine.downloads().job(*job_id).await {
                exits.push(Exit::for_download_job(&detail.job));
            } else {
                exits.push(Exit::DownloadFailed);
            }
        }
        EventKind::DownloadPausedAll { reason } => {
            if !cli.json {
                println!("paused all downloads ({})", reason.as_str());
            }
            if *reason == PauseAllReason::DiskFull {
                exits.push(Exit::DiskFull);
            }
        }
        EventKind::DownloadRetryScheduled {
            job_id,
            attempt,
            error_kind,
            next_attempt_at,
            ..
        } if !cli.json => {
            println!(
                "retry      {job_id}  attempt {attempt} failed ({error_kind}); next at {next_attempt_at}"
            );
        }
        _ => {}
    }
}
