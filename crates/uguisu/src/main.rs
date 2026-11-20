//! Uguisu – a self-hosted podcast archiver that re-publishes its archive as feeds.

mod auth;
mod ui;
mod web;

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
}

#[tokio::main]
async fn main() -> ExitCode {
    // Logs go to stderr so that command output on stdout stays clean.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
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
    cfg.verify_interval =
        (cli.verify_days > 0).then(|| Duration::from_secs(cli.verify_days * 24 * 3600));
    let app = Uguisu::open(cfg).await?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            app.spawn_workers();
            let base_url = cli
                .base_url
                .map(|u| u.trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty());
            let router = web::router(web::AppState { app, base_url });
            let listener = tokio::net::TcpListener::bind(cli.listen)
                .await
                .with_context(|| format!("cannot listen on {}", cli.listen))?;
            tracing::info!("Uguisu is listening on http://{}", cli.listen);
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
            Ok(ExitCode::SUCCESS)
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

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}
