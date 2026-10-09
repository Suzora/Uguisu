//! Uguisu as a desktop application.
//!
//! One process holds the engine, the HTTP server and the WebView. The window
//! shows the ordinary web UI served by the ordinary server over loopback, so
//! there is one UI, one API and one data model (ADR 0004, ADR 0041).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod launch;
mod logfile;
mod ops;
mod reveal;
mod server;
mod tray;

use std::cell::Cell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use tauri::Manager;

/// What the window is called and how big it opens.
const TITLE: &str = "Uguisu";
const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 860.0;

// What a refused start says, beyond what the engine or server reports itself.
const NO_WEB_UI: &str =
    "Uguisu's web interface is missing from this installation. Reinstall Uguisu.";
const NO_WINDOW: &str = "Uguisu could not open its window. Its log file has the details.";

fn main() -> ExitCode {
    init_tracing();
    // Before anything else touches the data directory: a shell that cannot
    // open a window must fail having started nothing.
    if let Err(reason) = graphical_session() {
        tracing::error!(error = %reason, "uguisu cannot open a window");
        return ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "uguisu could not start its runtime");
            return ExitCode::FAILURE;
        }
    };
    // The event loop is built first for the same reason: it is the step most
    // likely to fail on a machine without a usable desktop, and failing here
    // costs nothing because no engine is open yet.
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            launch_credential,
            ops::desktop_report,
            ops::choose_media_root,
            ops::set_notifications,
            ops::set_autostart,
            ops::notify,
            ops::reveal,
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(%error, "uguisu could not create its window system");
            return ExitCode::FAILURE;
        }
    };

    // Read before the engine opens: the remembered archive folder is an input
    // to the engine's configuration, not something to apply afterwards.
    let settings_path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(config::FILE);
    let chosen = config::Desktop::load(&settings_path).media_root;

    // The window's whole content is this directory, so its absence is a
    // broken installation rather than a degraded one, and it is refused
    // before the engine opens anything.
    let Some((web, source)) = web_dir(&app) else {
        tracing::error!("the web interface is not installed");
        refuse(app, NO_WEB_UI);
        return ExitCode::FAILURE;
    };
    tracing::info!(web_dir = %web.display(), source, "web interface found");

    // Before the engine opens: a termination request that arrives while the
    // server or the window is still starting must end in the same clean close
    // as one that arrives later, not in the signal's default action. The
    // request is queued until the event loop runs, or dropped with the app if
    // it never does, and the shutdown below runs either way.
    watch_for_termination(&runtime, app.handle().clone());

    tracing::info!("server starting");
    let embedded = match runtime.block_on(server::start(web, chosen)) {
        Ok(embedded) => embedded,
        Err(failure) => {
            // The message is the user-facing one; the log already carries
            // whatever the engine reported underneath it.
            tracing::error!(error = %failure, "uguisu could not start");
            if matches!(failure, server::Startup::Folder { .. }) {
                ask_for_folder(app, settings_path, failure.to_string());
            } else {
                refuse(app, &failure.to_string());
            }
            return ExitCode::FAILURE;
        }
    };
    let addr = embedded.addr();
    tracing::info!(%addr, "server listening");
    if std::env::args().any(|a| a == "--print-port") {
        announce(addr.port());
    }

    app.manage(embedded.launch());
    app.manage(ops::native(
        &app,
        embedded.media_root(),
        embedded.media_pinned(),
    ));
    let mut outcome = grant_desktop(&app, addr).and_then(|()| open_window(&app, addr));
    // Closed exactly once: by the event loop's `Exit`, or below when the loop
    // never ran.
    let embedded = Rc::new(Cell::new(Some(embedded)));
    let unopened = if outcome.is_ok() {
        tray::install(app.handle());
        tracing::info!("desktop ready");
        let code = run(app, runtime.handle().clone(), Rc::clone(&embedded));
        if code != 0 {
            tracing::warn!(code, "the window system exited with a failure");
            outcome = Err(());
        }
        None
    } else {
        Some(app)
    };
    shut_down(runtime.handle(), embedded.take());
    // After the close, so the data directory is free while the dialog waits.
    if let Some(app) = unopened {
        refuse(app, NO_WINDOW);
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

/// Runs the event loop, and closes the engine when it exits.
///
/// `run_return` rather than `run`: `run` ends the process from inside the
/// event loop, before the engine could close. The close runs on `Exit` rather
/// than after the loop returns because a Windows logoff reports `WM_ENDSESSION`
/// as `Exit` without leaving the loop, and Windows ends the process as soon as
/// that message is answered.
fn run(
    app: tauri::App,
    runtime: tokio::runtime::Handle,
    embedded: Rc<Cell<Option<server::Embedded>>>,
) -> i32 {
    app.run_return(move |app, event| {
        if let tauri::RunEvent::Exit = event {
            // A quit from the tray leaves the window up until the loop is gone.
            for window in app.webview_windows().values() {
                let _ = window.hide();
            }
            shut_down(&runtime, embedded.take());
        }
    })
}

/// Revokes the launch credential, stops serving and closes the engine.
fn shut_down(runtime: &tokio::runtime::Handle, embedded: Option<server::Embedded>) {
    if let Some(embedded) = embedded {
        tracing::info!("desktop shutting down");
        runtime.block_on(embedded.shutdown());
        tracing::info!("server stopped");
    }
}

/// Hands the per-launch credential to the web UI, once.
///
/// Reachable only from the window and origin named by the capability below, so
/// the secret never has to travel in a URL, a page or a log to get there.
// The state is taken by value because that is the shape a Tauri command must
// have; the lint has no way to know that.
#[allow(clippy::needless_pass_by_value, reason = "a Tauri command's signature")]
#[tauri::command]
fn launch_credential(
    launch: tauri::State<'_, Arc<launch::Launch>>,
) -> Result<String, &'static str> {
    launch
        .take()
        .map(uguisu_core::secret::Secret::into_inner)
        .ok_or("the launch credential has already been used")
}

/// Grants the desktop commands to exactly this launch's origin.
///
/// Registered here rather than in `capabilities/` because the port is not
/// known until the server has bound: a static capability would have to name a
/// wildcard port, and every extra origin it matched would be another WebView
/// that could ask. Nothing outside this list is reachable from the page — no
/// shell, no filesystem, no process control (ADR 0041).
fn grant_desktop(app: &tauri::App, addr: std::net::SocketAddr) -> Result<(), ()> {
    let capability = tauri::ipc::CapabilityBuilder::new("desktop-bootstrap")
        .local(false)
        .remote(format!("http://{addr}"))
        .window("main")
        .permission("allow-launch-credential")
        .permission("allow-desktop-report")
        .permission("allow-choose-media-root")
        .permission("allow-set-notifications")
        .permission("allow-set-autostart")
        .permission("allow-notify")
        .permission("allow-reveal");
    app.add_capability(capability).map_err(|error| {
        tracing::error!(%error, "the desktop capability could not be granted");
    })
}

/// Points the WebView at the loopback server the engine is already serving.
fn open_window(app: &tauri::App, addr: std::net::SocketAddr) -> Result<(), ()> {
    let url = format!("http://{addr}/");
    let target = match url.parse() {
        Ok(target) => target,
        Err(error) => {
            tracing::error!(%error, "the loopback address is not a usable url");
            return Err(());
        }
    };
    match tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(target))
        .title(TITLE)
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(640.0, 480.0)
        .build()
    {
        Ok(_) => Ok(()),
        Err(error) => {
            tracing::error!(%error, "the desktop window could not be created");
            Err(())
        }
    }
}

/// Says in a native dialog why Uguisu is not starting, and returns once it is dismissed.
///
/// A Windows build has no console, so a refusal that is only logged is never
/// seen. The dialog plugin shows its dialogs through the event loop, so the
/// loop runs for this one dialog and ends when it is closed. In the Flatpak,
/// rfd can show a message only through `zenity`; without it nothing appears
/// and the log alone has the reason.
fn refuse(app: tauri::App, message: &str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    let handle = app.handle().clone();
    app.dialog()
        .message(message)
        .title(TITLE)
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
    app.run_return(|_, _| {});
}

/// Says why the remembered archive folder cannot be used, and offers to choose another (ADR 0044).
///
/// A usable choice is remembered and Uguisu starts again with it; quitting
/// ends here. The archive never moves anywhere the person did not pick.
fn ask_for_folder(app: tauri::App, settings: PathBuf, message: String) {
    ask(app.handle().clone(), settings, message);
    app.run_return(|_, _| {});
}

/// The message with its two choices, and the picker behind the first.
///
/// In the Flatpak rfd can show a message only through `zenity`, and without it
/// the answer is "Quit" with nothing on screen. The portal's folder picker
/// always appears, so there it opens straight away with the reason as its
/// title, and cancelling it quits.
fn ask(handle: tauri::AppHandle, settings: PathBuf, message: String) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    if ops::sandboxed() {
        pick(handle, settings, message);
        return;
    }
    let next = handle.clone();
    handle
        .dialog()
        .message(message.clone())
        .title(TITLE)
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Choose another folder".to_owned(),
            "Quit".to_owned(),
        ))
        .show(move |choose| {
            if choose {
                pick(next, settings, message);
            } else {
                next.exit(1);
            }
        });
}

/// Opens the folder picker; a usable folder is remembered and Uguisu restarts.
fn pick(handle: tauri::AppHandle, settings: PathBuf, message: String) {
    use tauri_plugin_dialog::DialogExt;

    let sandboxed = ops::sandboxed();
    let title = if sandboxed {
        message.clone()
    } else {
        "Choose the archive folder".to_owned()
    };
    let chooser = handle.dialog().file().set_title(title);
    chooser.pick_folder(move |picked| {
        let Some(picked) = picked else {
            if sandboxed {
                handle.exit(1);
            } else {
                ask(handle, settings, message);
            }
            return;
        };
        let remembered = picked
            .into_path()
            .map_err(|e| format!("that folder cannot be used: {e}"))
            .and_then(|folder| config::usable_folder(&folder))
            .and_then(|folder| {
                let mut desktop = config::Desktop::load(&settings);
                desktop.media_root = Some(folder);
                desktop
                    .save(&settings)
                    .map_err(|e| format!("it could not be remembered: {e}"))
            });
        match remembered {
            Ok(()) => handle.request_restart(),
            Err(reason) => ask(
                handle,
                settings,
                format!("The folder you chose cannot be used: {reason}."),
            ),
        }
    });
}

/// Whether this machine has a desktop session to put a window on.
///
/// Checked rather than discovered: the toolkit aborts the process when it
/// cannot reach a display, which would be a stack trace where a sentence
/// belongs. Only Linux can lack one: Windows always has a desktop to draw on.
fn graphical_session() -> Result<(), &'static str> {
    if cfg!(not(target_os = "linux"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("DISPLAY").is_some()
    {
        Ok(())
    } else {
        Err(
            "no graphical session was found: neither WAYLAND_DISPLAY nor DISPLAY is set. \
             Run `uguisu serve` instead on a machine without a desktop.",
        )
    }
}

/// Turns Ctrl-C and SIGTERM into an ordinary application exit.
///
/// Without this the WebView owns the process and a signal kills it outright,
/// which would skip the engine's shutdown and leave the data directory to be
/// recovered on the next launch instead of closed cleanly.
fn watch_for_termination(runtime: &tokio::runtime::Runtime, handle: tauri::AppHandle) {
    runtime.spawn(async move {
        let interrupt = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        #[cfg(unix)]
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => tracing::warn!(%error, "SIGTERM is not being watched"),
            }
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        tokio::select! {
            () = interrupt => {}
            () = terminate => {}
        }
        handle.exit(0);
    });
}

/// Prints the bound port for a test harness.
///
/// The port is not a secret — anything on the machine can read it from the
/// socket table — and nothing else is ever written to stdout.
#[allow(clippy::print_stdout, reason = "the one machine-readable line")]
fn announce(port: u16) {
    use std::io::Write;
    println!("port={port}");
    let _ = std::io::stdout().flush();
}

/// Where the built web UI lives, and which rule found it.
///
/// In a package it is a bundled resource, which every platform puts somewhere
/// different — `/usr/lib/<product>/` on Linux, beside the executable on
/// Windows — so the question is asked of Tauri rather than guessed from the
/// binary's path. `UGUISU_WEB_DIR` overrides for a developer.
///
/// The checkout's `web/dist`, relative to the working directory, is consulted
/// by debug builds only: a release binary that found it would hide a package
/// that shipped without its interface, which is exactly how one once did.
fn web_dir(app: &tauri::App) -> Option<(PathBuf, &'static str)> {
    let from_env = std::env::var_os("UGUISU_WEB_DIR").map(|dir| (PathBuf::from(dir), "env"));
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| (dir.join("web"), "bundled"));
    let checkout = cfg!(debug_assertions).then(|| (PathBuf::from("web/dist"), "checkout"));
    from_env
        .into_iter()
        .chain(bundled)
        .chain(checkout)
        .find(|(candidate, _)| candidate.join("index.html").is_file())
}

/// The bundle identifier, as `tauri.conf.json` states it.
///
/// Needed before the Tauri app exists, to put the log where Tauri's own
/// `app_log_dir` would; `check-desktop-layout.py` fails if the two differ.
const IDENTIFIER: &str = "io.github.suzora.Uguisu";

/// Logs to stderr and to a file, filtered by `UGUISU_LOG`.
///
/// A release build on Windows has no console, so without the file its log
/// would go nowhere. One file per running launch, and the previous one is
/// kept as `.1`, so the directory never grows; a refused launch appends to the
/// running one's (`logfile`). A log directory that cannot be created costs the
/// file, not the launch.
fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = std::env::var("UGUISU_LOG").unwrap_or_else(|_| "info".to_owned());
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let file = log_file().map(|file| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(false)
            .with_writer(std::sync::Mutex::new(file))
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .with(file)
        .init();
}

/// Opens this launch's log file in the per-user log directory.
fn log_file() -> Option<std::fs::File> {
    let dir = directories::BaseDirs::new()?
        .data_local_dir()
        .join(IDENTIFIER)
        .join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let log = logfile::open(&dir).ok()?;
    // Held until the process exits, which is what releases it.
    std::mem::forget(log.rotation);
    Some(log.file)
}
