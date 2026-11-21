//! Uguisu – a self-hosted podcast archiver that re-publishes its archive as feeds.

mod auth;
mod ui;
mod web;

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use uguisu_core::{Config, Uguisu};

#[derive(Parser)]
#[command(name = "uguisu", version, about)]
struct Cli {
    /// Directory for the database and the archive.
    #[arg(long, env = "UGUISU_DATA_DIR", default_value = "./data", global = true)]
    data_dir: PathBuf,

    /// Address the web interface listens on.
    #[arg(
        long,
        env = "UGUISU_LISTEN",
        default_value = "0.0.0.0:8484",
        global = true
    )]
    listen: SocketAddr,

    /// Public URL of this instance, e.g. https://podcasts.example.org. Used in published feeds.
    /// When unset it is derived from each request.
    #[arg(long, env = "UGUISU_BASE_URL", global = true)]
    base_url: Option<String>,

    /// Minutes between checks of each source feed.
    #[arg(
        long,
        env = "UGUISU_REFRESH_MINUTES",
        default_value_t = 60,
        global = true
    )]
    refresh_minutes: u64,

    /// Number of simultaneous media downloads.
    #[arg(
        long,
        env = "UGUISU_DOWNLOAD_CONCURRENCY",
        default_value_t = 2,
        global = true
    )]
    download_concurrency: usize,

    /// Every archived file is re-hashed at least once in this many days. 0 turns the automatic
    /// integrity check off; `uguisu verify` still checks everything on demand.
    #[arg(long, env = "UGUISU_VERIFY_DAYS", default_value_t = 30, global = true)]
    verify_days: u64,

    /// Downloads stop while less than this many gigabytes are free on the archive's disk, so
    /// that the database and feed snapshots always have room. 0 turns the check off.
    #[arg(long, env = "UGUISU_MIN_FREE_GB", default_value_t = 1, global = true)]
    min_free_gb: u64,

    /// Number of daily copies of the database kept in `backups/` inside the data directory.
    /// 0 turns the copies off.
    #[arg(long, env = "UGUISU_BACKUPS", default_value_t = 7, global = true)]
    backups: usize,

    /// Allow fetching feeds and media from loopback and private addresses.
    #[arg(long, env = "UGUISU_ALLOW_PRIVATE_NETWORKS", global = true)]
    allow_private_networks: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the archiver and its web interface (default).
    Serve,
    /// Re-hash every archived file and report damaged or missing ones.
    Verify,
    /// Rebuild the database from the archive on disk. Needs an empty database: stop Uguisu and
    /// move `uguisu.db` away first.
    Reindex,
    /// Take media files that are already on disk into the archive of a podcast. Files are
    /// matched to episodes by file name or title and copied; the directory is left as it is.
    Import {
        /// Directory with the media files; subdirectories are included.
        directory: PathBuf,
        /// Feed address of the podcast the files belong to. It is added if it is not archived yet.
        #[arg(long)]
        feed: String,
        /// Keep files that fit no episode of the feed as episodes of their own.
        #[arg(long)]
        keep_unmatched: bool,
    },
    /// Remove the password of the web interface and sign out every device. The next visit asks
    /// for a new password. For when the password is lost.
    ResetPassword,
    /// Ask the running instance whether it answers; exits with 0 if it does. Used by the
    /// container health check, where no other HTTP client is available.
    Healthcheck,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Logs go to stderr so that command output on stdout stays clean.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // No colour codes in `docker logs` and log files.
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let mut cfg = Config::new(&cli.data_dir);
    cfg.allow_private_networks = cli.allow_private_networks;
    cfg.refresh_interval = Duration::from_secs(cli.refresh_minutes.max(1) * 60);
    cfg.download_concurrency = cli.download_concurrency.max(1);
    cfg.min_free_space = cli.min_free_gb.saturating_mul(1 << 30);
    cfg.backups = cli.backups;
    cfg.verify_interval =
        (cli.verify_days > 0).then(|| Duration::from_secs(cli.verify_days * 24 * 3600));
    if matches!(cli.command, Some(Command::Healthcheck)) {
        return Ok(if healthcheck(cli.listen).await {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    let app = Uguisu::open(cfg).await.map_err(|error| {
        let denied = error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
        });
        if denied {
            error.context(format!(
                "the data directory {} is not writable for this process; in a container that \
                 runs as uid 65532, make a mounted directory writable with \
                 `chown 65532:65532 <directory>`",
                cli.data_dir.display()
            ))
        } else {
            error
        }
    })?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            app.spawn_workers();
            let base_url = cli
                .base_url
                .map(|u| u.trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty());
            let router = web::router(web::AppState::new(app, base_url));
            let listener = tokio::net::TcpListener::bind(cli.listen)
                .await
                .with_context(|| format!("cannot listen on {}", cli.listen))?;
            tracing::info!("Uguisu is listening on http://{}", cli.listen);
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Healthcheck => unreachable!("handled before the archive is opened"),
        Command::Import {
            directory,
            feed,
            keep_unmatched,
        } => {
            let podcast_id = app.add_podcast(&feed).await?;
            let report = app
                .import_directory(podcast_id, &directory, keep_unmatched)
                .await?;
            for (file, episode) in &report.matched {
                println!("MATCHED  {file}  ->  {episode}");
            }
            for file in &report.added {
                println!("ADDED    {file}");
            }
            for file in &report.known {
                println!("KNOWN    {file}");
            }
            for (file, reason) in &report.skipped {
                println!("SKIPPED  {file}  ({reason})");
            }
            println!(
                "{} files matched to episodes, {} kept as episodes of their own, \
                 {} already archived, {} skipped",
                report.matched.len(),
                report.added.len(),
                report.known.len(),
                report.skipped.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::ResetPassword => {
            auth::reset_password(app.pool()).await?;
            println!("The password is removed. Open the web interface to choose a new one.");
            Ok(ExitCode::SUCCESS)
        }
        Command::Reindex => {
            let report = app.reindex().await?;
            for problem in &report.problems {
                println!("SKIPPED  {problem}");
            }
            println!(
                "{} podcasts, {} episodes and {} files indexed, {} entries skipped",
                report.podcasts,
                report.episodes,
                report.files,
                report.problems.len()
            );
            Ok(if report.problems.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::Verify => {
            let report = app.verify().await?;
            for path in &report.corrupt {
                println!("CORRUPT  {path}");
            }
            for path in &report.missing {
                println!("MISSING  {path}");
            }
            println!(
                "{} files checked: {} intact, {} corrupt, {} missing",
                report.checked,
                report.ok,
                report.corrupt.len(),
                report.missing.len()
            );
            Ok(if report.ok == report.checked {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

/// Requests `/healthz` from the instance listening on `listen`.
async fn healthcheck(listen: SocketAddr) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut target = listen;
    if target.ip().is_unspecified() {
        target.set_ip(match target {
            SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        });
    }
    let request = async {
        let mut stream = tokio::net::TcpStream::connect(target).await?;
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Ok::<_, std::io::Error>(response.starts_with(b"HTTP/1.1 200"))
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(5), request).await,
        Ok(Ok(true))
    )
}

/// Resolves on Ctrl-C or on SIGTERM, which is what `docker stop` sends.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}
