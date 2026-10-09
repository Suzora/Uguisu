//! The embedded Uguisu server: open the engine, bind loopback, close both.
//!
//! This is `run_serve` from the CLI with a different shutdown source and a
//! different error surface. The composition is repeated rather than lifted into
//! a shared crate because the two agree on none of the interesting steps — the
//! tracing sink, the shutdown source, the auth options, the web directory and
//! what a failure must look like to the person in front of it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use crate::launch::Launch;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uguisu_core::config::Config;
use uguisu_core::error::UguisuError;
use uguisu_engine::Engine;
use uguisu_server::auth::AuthOptions;
use uguisu_server::{AppState, Bound};

/// Why the desktop could not start, in words a person can act on.
///
/// Every variant carries a path or an operating-system message and never a
/// credential: these strings reach a dialog and the log.
#[derive(Debug, thiserror::Error)]
pub enum Startup {
    /// Configuration could not be read.
    #[error("Uguisu could not read its configuration: {0}")]
    Config(String),
    /// Another process holds the data directory's lock.
    #[error("Another Uguisu is already using this data directory ({0}). Close it and try again.")]
    Locked(String),
    /// The data directory could not be opened, created or migrated.
    #[error("Uguisu could not open its data directory: {0}")]
    Data(String),
    /// The loopback listener could not be created.
    #[error("Uguisu could not start its local server: {0}")]
    Listen(String),
    /// The remembered archive folder is gone, not a folder, or not writable.
    #[error(
        "The archive folder {path} cannot be used: {reason}. If it is on a drive that is not \
         connected, connect it and start Uguisu again, or choose another folder."
    )]
    Folder {
        /// The folder `desktop.json` names.
        path: String,
        /// What the check found.
        reason: String,
    },
}

/// A running embedded server, and everything that closes with it.
#[derive(Debug)]
pub struct Embedded {
    engine: Engine,
    addr: SocketAddr,
    media_root: PathBuf,
    media_pinned: bool,
    launch: std::sync::Arc<Launch>,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl Embedded {
    /// Where the web UI and the API are, with the port the OS assigned.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Where the archive is for this launch, as the engine resolved it.
    pub fn media_root(&self) -> PathBuf {
        self.media_root.clone()
    }

    /// Whether the environment fixed the archive directory, so the picker
    /// must say so rather than pretend it can change it.
    pub fn media_pinned(&self) -> bool {
        self.media_pinned
    }

    /// The credential this launch bootstraps its session with.
    pub fn launch(&self) -> std::sync::Arc<Launch> {
        std::sync::Arc::clone(&self.launch)
    }

    /// Stops serving, then runs the engine's own shutdown to completion.
    ///
    /// Ordering is the point: new requests stop first, in-flight ones drain,
    /// and only then do the download queue, the scheduler, the index and the
    /// database close. Dropping `engine` last is what releases the lock file.
    pub async fn shutdown(self) {
        // Before anything else: the credential stops working even if the rest
        // of this shutdown goes wrong.
        self.launch.revoke(&self.engine).await;
        // The receiver is gone only if the server task already ended.
        let _ = self.stop.send(());
        // Bounded by the server itself: the window's event stream ends only
        // with the engine, so serving gives up on it rather than wait.
        if let Err(error) = self.serving.await {
            tracing::warn!(%error, "the server task did not join cleanly");
        }
        self.engine.close().await;
    }
}

/// Opens the engine and serves it on a loopback port the OS chooses.
///
/// On any failure past the engine opening, the engine is closed again, so a
/// shell that cannot finish starting leaves nothing running behind it.
pub async fn start(web: PathBuf, chosen: Option<PathBuf>) -> Result<Embedded, Startup> {
    let mut config = Config::from_env().map_err(|e| Startup::Config(e.to_string()))?;
    // `UGUISU_MEDIA_DIR` wins. The shell's remembered folder fills the gap
    // when the environment left one, and never overrides it (ADR 0044).
    let media_pinned = config.data.media_dir.is_some();
    if !media_pinned {
        // Checked before anything opens: the first download would otherwise
        // create the folder again wherever its path now leads (ADR 0044).
        config.data.media_dir = chosen
            .map(|folder| {
                crate::config::usable_folder(&folder).map_err(|reason| Startup::Folder {
                    path: folder.display().to_string(),
                    reason,
                })
            })
            .transpose()?;
    }
    let engine = Engine::open(config).await.map_err(|e| match e {
        UguisuError::Locked(detail) => Startup::Locked(detail),
        other => Startup::Data(other.to_string()),
    })?;
    let media_root = match engine.config().data.media_dir() {
        Ok(root) => root,
        Err(error) => {
            engine.close().await;
            return Err(Startup::Config(error.to_string()));
        }
    };
    match bring_up(&engine, web).await {
        Ok((addr, launch, stop, serving)) => Ok(Embedded {
            engine,
            addr,
            media_root,
            media_pinned,
            launch,
            stop,
            serving,
        }),
        Err(failure) => {
            engine.close().await;
            Err(failure)
        }
    }
}

type BroughtUp = (
    SocketAddr,
    std::sync::Arc<Launch>,
    oneshot::Sender<()>,
    JoinHandle<()>,
);

async fn bring_up(engine: &Engine, web: PathBuf) -> Result<BroughtUp, Startup> {
    let launch = std::sync::Arc::new(
        Launch::mint(engine)
            .await
            .map_err(|e| Startup::Data(e.to_string()))?,
    );
    let state = AppState::new(engine.discovery().clone(), Some(engine.clone()))
        .with_web_dir(Some(web))
        .with_auth(AuthOptions {
            // On the desktop authentication is always on, password or not:
            // the per-launch token is the credential, and the WebView trades
            // it for an ordinary session. A fresh install is therefore not an
            // anonymous API, which is what `uguisu serve` on loopback with no
            // password still is (ADR 0042).
            required: true,
            // Loopback over plain HTTP: a `Secure` cookie would never be sent.
            cookie_secure: false,
            // Port 0 on 127.0.0.1 satisfies the exposure gate on its own merits
            // (ADR 0037). The desktop never sets the override.
            allow_insecure_exposure: false,
            // Nothing stands between the WebView and the server.
            trusted_proxies: Vec::new(),
        });
    let bound = bind(&state).await?;
    let addr = bound.addr();
    engine.start_downloads();
    engine.start_refresh_scheduler();
    engine.start_search_index();
    let (stop, halt) = oneshot::channel();
    let serving = tokio::spawn(async move {
        let shutdown = async {
            let _ = halt.await;
        };
        if let Err(error) = bound.serve(state, shutdown).await {
            tracing::error!(%error, "the embedded server stopped");
        }
    });
    Ok((addr, launch, stop, serving))
}

async fn bind(state: &AppState) -> Result<Bound, Startup> {
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    uguisu_server::bind(loopback, state)
        .await
        .map_err(|e| Startup::Listen(e.to_string()))
}
