//! The `uguisu` binary.
//!
//! `uguisu serve` runs the server (with the download workers). Discovery
//! commands (`search`, `resolve`), the library commands (`podcast
//! add|list|show|refresh`, `feed inspect|refresh|status`) and the download
//! queue commands (`download …`) run embedded by default and talk to a
//! running server only when `--server` is given (ADR 0003, amended).
//! Embedded stateful commands take the data directory's
//! lock, so they refuse to run while `uguisu serve` holds it. The
//! remaining subcommands are scaffolded and report the roadmap phase that
//! delivers them. Exit codes are documented in `docs/CLI.md`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use uguisu_core::UguisuError;
use uguisu_core::config::{Config, DiscoveryConfig};
use uguisu_core::model::DuplicateResolution;
use uguisu_core::provider::ProviderId;
use uguisu_discovery::{Discovery, ResolveError, SearchOutcome, SearchRequest, assemble};
use uguisu_engine::{Engine, EngineConfig};

mod archive;
mod auth;
mod client;
mod db;
mod download;
mod library;
mod output;
mod service;

use output::Exit;

/// Uguisu — podcast downloader and archiver.
#[derive(Debug, Parser)]
#[command(name = "uguisu", version = uguisu_core::VERSION, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON on stdout instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Address of a running Uguisu server; discovery commands run embedded when unset.
    #[arg(long, global = true, env = "UGUISU_SERVER")]
    server: Option<String>,

    /// API token for server mode (see `uguisu auth token create`).
    ///
    /// A `Secret`, so the `Debug` of this struct — which is what a `--log
    /// debug` line would print — says `[redacted]` rather than the token.
    #[arg(long, global = true, env = "UGUISU_TOKEN", hide_env_values = true)]
    token: Option<uguisu_core::secret::Secret<String>>,

    /// Log filter, e.g. `info`, `uguisu_discovery=debug`.
    #[arg(long, global = true, env = "UGUISU_LOG", default_value = "warn")]
    log: String,

    /// Log line format on stderr: `pretty` for a person, `json` for one object per line.
    #[arg(
        long,
        global = true,
        env = "UGUISU_LOG_FORMAT",
        value_enum,
        default_value = "pretty"
    )]
    log_format: LogFormat,

    /// Data directory (database and lock); defaults to the platform data directory.
    #[arg(long, global = true, env = "UGUISU_DATA_DIR", value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the Uguisu server (API, scheduler, downloads, web UI).
    Serve(ServeArgs),
    /// Ask a running server whether it is up; exit 0 if it is, 1 if not.
    Health(HealthArgs),
    /// Search podcast directories.
    Search {
        #[command(subcommand)]
        what: SearchCommand,
    },
    /// Resolve a feed URL, website or directory page to a verified feed.
    Resolve {
        /// RSS/Atom URL, podcast website or directory page URL.
        input: String,
    },
    /// Manage podcasts in the library.
    Podcast {
        #[command(subcommand)]
        action: PodcastCommand,
    },
    /// Inspect feeds and their fetch state.
    Feed {
        #[command(subcommand)]
        action: FeedCommand,
    },
    /// Inspect and download episodes.
    Episode {
        #[command(subcommand)]
        action: EpisodeCommand,
    },
    /// Verify, reconcile and import the archive.
    Archive {
        #[command(subcommand)]
        action: ArchiveCommand,
    },
    /// Queue episodes for download and control the download queue.
    Download(DownloadArgs),
    /// Validate, show and change configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Control the feed-refresh scheduler (`uguisu serve` runs it).
    Scheduler {
        #[command(subcommand)]
        action: SchedulerCommand,
    },
    /// Database maintenance.
    Db {
        #[command(subcommand)]
        action: DbCommand,
    },
    /// The password, and the tokens that stand in for it.
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },
}

/// A boolean flag that also takes the words an `UGUISU_*` variable takes, so
/// `UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1` means what the refusal says it does.
fn truthy(raw: &str) -> Result<bool, String> {
    uguisu_core::config::boolean(raw).ok_or_else(|| format!("expected true/false, got `{raw}`"))
}

#[derive(Debug, Args)]
struct HealthArgs {
    /// The address `serve` binds, asked on loopback when it is unspecified
    /// (`0.0.0.0`, `::`). Ignored with `--server`.
    #[arg(long, env = "UGUISU_BIND", default_value = uguisu_server::DEFAULT_BIND)]
    bind: SocketAddr,
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Socket address to bind.
    #[arg(long, env = "UGUISU_BIND", default_value = uguisu_server::DEFAULT_BIND)]
    bind: SocketAddr,
    /// Directory holding the built web UI, relative to the working directory
    /// unless absolute. A directory that is not there serves the API alone.
    #[arg(long = "web", env = "UGUISU_WEB_DIR", default_value = "web/dist")]
    web: PathBuf,

    /// Bind a network address even with no password set (ADR 0037).
    ///
    /// A deployment knob like `--bind`, not a stored setting: a row in the
    /// database that permits insecure exposure could be written through the
    /// API it is supposed to protect.
    #[arg(
        long,
        env = "UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE",
        default_value_t = false,
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        value_parser = truthy,
    )]
    allow_insecure_exposure: bool,

    /// Mark the session cookie `Secure`, for a deployment with TLS in front.
    #[arg(
        long,
        env = "UGUISU_AUTH_COOKIE_SECURE",
        default_value_t = false,
        num_args = 0..=1,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        value_parser = truthy,
    )]
    cookie_secure: bool,

    /// Reverse proxies, by IP or network, whose `X-Forwarded-For` names the
    /// client for the login limiter and the logs (ADR 0062). A process
    /// option, never a stored setting.
    #[arg(
        long = "trusted-proxy",
        env = "UGUISU_TRUSTED_PROXIES",
        value_delimiter = ',',
        value_name = "IP|CIDR"
    )]
    trusted_proxies: Vec<uguisu_server::auth::TrustedProxy>,
}

#[derive(Debug, Subcommand)]
enum SearchCommand {
    /// Search for a podcast by name (a URL is resolved instead).
    Podcast(SearchArgs),
    /// Search the local library: podcasts and episodes already added.
    Library(LibrarySearchArgs),
    /// Rebuild the local search index.
    Reindex,
}

#[derive(Debug, Args, Clone)]
struct LibrarySearchArgs {
    /// What to look for. Operators are not special: every word is a word.
    query: String,
    /// Hits of each kind (1..=200).
    #[arg(long, default_value_t = 25)]
    limit: u32,
    /// Match the last word as a prefix (for a search box).
    #[arg(long)]
    prefix: bool,
    /// Show how each score was arrived at.
    #[arg(long)]
    explain: bool,
}

#[derive(Debug, Args, Clone)]
struct SearchArgs {
    /// Free-text query, e.g. "darknet diaries", or a feed/website URL.
    query: String,
    /// Restrict to these providers (comma-separated: apple, podcastindex, gpoddernet).
    #[arg(long, value_delimiter = ',')]
    provider: Vec<String>,
    /// Maximum number of results.
    #[arg(long)]
    limit: Option<usize>,
    /// Storefront country (ISO 3166-1 alpha-2), e.g. DE.
    #[arg(long)]
    country: Option<String>,
    /// Show the ranking signals for every result.
    #[arg(long)]
    explain: bool,
    /// Also resolve and verify the top result's feed.
    #[arg(long)]
    resolve: bool,
    /// Bypass the discovery cache.
    #[arg(long)]
    no_cache: bool,
}

#[derive(Debug, Subcommand)]
enum PodcastCommand {
    /// Add a podcast by RSS URL, website URL or search term.
    Add {
        /// Feed URL, website URL or search term.
        input: String,
        /// Skip the confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
    /// List podcasts.
    List,
    /// Plan an OPML import, or add its new feeds with --apply (ADR 0049).
    Import(ImportArgs),
    /// Print every podcast as an OPML file.
    Export,
    /// Show one podcast.
    Show {
        /// Podcast ID.
        id: String,
    },
    /// Move a podcast to another feed URL, such as one its feed announces
    /// (ADR 0052). The feed is checked first; one that fails the check is
    /// moved to only with --force.
    MoveFeed(MoveFeedArgs),
    /// Refresh one podcast's feed now (or every podcast with --all).
    Refresh {
        /// Podcast ID.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<String>,
        /// Refresh every active podcast.
        #[arg(long)]
        all: bool,
        /// Ignore ETag/Last-Modified and the body fingerprint.
        #[arg(long)]
        force: bool,
    },
    /// Take a podcast out of the schedule (manual refreshes still work).
    Pause {
        /// Podcast ID.
        id: String,
    },
    /// Put a paused or archived podcast back in the schedule.
    Resume {
        /// Podcast ID.
        id: String,
    },
    /// Set when a podcast is next due.
    Schedule {
        /// Podcast ID.
        id: String,
        /// RFC 3339 timestamp; omit for "as soon as the scheduler looks".
        #[arg(long)]
        at: Option<String>,
    },
    /// Stop fetching a podcast and keep everything it has (ADR 0055);
    /// `podcast resume` brings it back.
    Archive {
        /// Podcast ID.
        id: String,
    },
    /// Remove a podcast from the library. Every file stays on disk; its
    /// records do not (ADR 0055).
    Remove {
        /// Podcast ID.
        id: String,
        /// Confirm the removal; without it nothing is removed.
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum FeedCommand {
    /// Fetch and parse a feed URL without storing anything.
    Inspect {
        /// Feed URL.
        url: String,
    },
    /// Refresh the podcast behind a source now.
    Refresh {
        /// Source ID (see `podcast show`).
        source_id: String,
        /// Ignore ETag/Last-Modified and the body fingerprint.
        #[arg(long)]
        force: bool,
    },
    /// Show a source's fetch state and its last fetch.
    Status {
        /// Source ID.
        source_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum EpisodeCommand {
    /// List a podcast's episodes, newest first.
    List {
        /// Podcast ID.
        podcast_id: String,
    },
    /// Show one episode: what the feed says, its download job and its archived file.
    Show {
        /// Episode ID.
        id: String,
    },
    /// List candidate duplicates: episodes a refresh stored as probably
    /// the same as an earlier one, skipped until you resolve them.
    Duplicates {
        /// Only this podcast's.
        #[arg(long)]
        podcast: Option<String>,
    },
    /// Resolve a candidate duplicate: `--same` merges it into the episode it
    /// duplicates, `--separate` keeps both.
    Resolve {
        /// Candidate episode ID.
        id: String,
        /// One episode: merge the candidate into the original (cannot be undone).
        #[arg(
            long,
            conflicts_with = "separate",
            required_unless_present = "separate"
        )]
        same: bool,
        /// Two episodes: keep the candidate as an episode of its own.
        #[arg(long)]
        separate: bool,
    },
    /// Queue an episode for download (alias of `uguisu download <id>`).
    Download {
        /// Episode ID.
        id: String,
        /// Queue priority.
        #[arg(long, default_value = "normal", value_parser = PRIORITIES)]
        priority: String,
        /// Run the download workers until this job stops.
        #[arg(long)]
        wait: bool,
    },
}

const PRIORITIES: [&str; 3] = ["low", "normal", "high"];

/// `uguisu download <episode-id>` or one of the queue subcommands.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
struct DownloadArgs {
    #[command(subcommand)]
    action: Option<DownloadCommand>,
    /// Episode ID to queue.
    #[arg(required = true, value_name = "EPISODE_ID")]
    episode_id: Option<String>,
    /// Queue priority.
    #[arg(long, default_value = "normal", value_parser = PRIORITIES)]
    priority: String,
    /// Run the download workers in this process until the job stops.
    #[arg(long)]
    wait: bool,
}

#[derive(Debug, Subcommand)]
enum DownloadCommand {
    /// Queue every downloadable episode of a podcast.
    Podcast {
        /// Podcast ID.
        id: String,
        /// Queue priority.
        #[arg(long, default_value = "normal", value_parser = PRIORITIES)]
        priority: String,
    },
    /// List download jobs, newest first.
    List {
        /// Only jobs in this state (queued, downloading, finalizing, retrying, paused, completed, failed, cancelled).
        #[arg(long)]
        state: Option<String>,
        /// Only jobs of this podcast.
        #[arg(long)]
        podcast: Option<String>,
        /// Continue after this job id (keyset paging).
        #[arg(long)]
        after: Option<String>,
        /// Page size (1..=500).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show one job with its attempts.
    Show {
        /// Job ID.
        id: String,
    },
    /// Cancel a job (the partial file is removed later by the worker; the row stays).
    Cancel {
        /// Job ID.
        id: String,
    },
    /// Pause a job (its partial file is kept).
    Pause {
        /// Job ID.
        id: String,
    },
    /// Resume a paused job.
    Resume {
        /// Job ID.
        id: String,
    },
    /// Queue a failed or cancelled job again with a fresh attempt budget.
    Retry {
        /// Job ID.
        id: String,
    },
    /// Queue every failed job again.
    RetryFailed,
    /// Pause the whole queue (persists across restarts).
    PauseAll,
    /// Resume the whole queue.
    ResumeAll,
    /// Queue statistics.
    Stats,
    /// Reconcile the queue with the media directory.
    Reconcile {
        /// Also check that every completed file is still there.
        #[arg(long)]
        deep: bool,
    },
    /// Run the download workers in this process.
    Run {
        /// Stop once the queue is idle instead of running until Ctrl-C.
        #[arg(long)]
        until_idle: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ArchiveCommand {
    /// List archived files, newest first.
    List {
        /// Only files of this podcast.
        #[arg(long)]
        podcast: Option<String>,
        /// Only files in this verification state.
        #[arg(long)]
        state: Option<String>,
        /// Only files whose feed now points at different audio.
        #[arg(long)]
        source_changed: bool,
        /// How many to show.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show one episode's archived file.
    Show {
        /// Episode id.
        episode_id: String,
    },
    /// Verify that archived files are still there and still correct.
    Verify {
        /// One episode; omit with `--all` or `--podcast`.
        episode_id: Option<String>,
        /// Verify every archived file.
        #[arg(long)]
        all: bool,
        /// Verify one podcast's files.
        #[arg(long)]
        podcast: Option<String>,
        /// Hash every file instead of the light size/mtime check.
        #[arg(long)]
        full: bool,
    },
    /// List files whose recording says they are gone.
    Missing,
    /// List files that are there but do not match their record.
    Invalid,
    /// Show where the current template would put an episode.
    PathPreview {
        /// Episode id.
        episode_id: String,
    },
    /// Move archived files to the paths the current template produces.
    Relocate {
        /// One episode; omit with `--all` or `--podcast`.
        episode_id: Option<String>,
        /// Relocate every archived file.
        #[arg(long)]
        all: bool,
        /// Relocate one podcast's files.
        #[arg(long)]
        podcast: Option<String>,
        /// Report what would move without moving anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read or change the automatic archive policy.
    Policy {
        #[command(subcommand)]
        action: PolicyCommand,
    },
    /// Reconcile the database with the filesystem.
    Reconcile {
        /// Also verify every file (size, not hash).
        #[arg(long)]
        deep: bool,
        /// Rebuild archive records from the sidecars on disk.
        #[arg(long)]
        rebuild: bool,
        /// With `--rebuild`: write the records. Without it, nothing is
        /// written and the report says what would be.
        #[arg(long)]
        apply: bool,
        /// With `--rebuild`: only this podcast's documents.
        #[arg(long)]
        podcast: Option<String>,
    },
    /// Report what nothing owns under the media directory: leftovers of
    /// interrupted writes, orphan `.part` files, media without a record and
    /// stray sidecars. Reads only; exits 1 when anything is found.
    Orphans,
    /// Import an existing podcast archive from a directory.
    ///
    /// Reads only: files are copied, the source is never modified, and a
    /// file that matches more than one episode is reported rather than
    /// placed. Nothing is copied without `--apply`.
    Import {
        /// Root directory of the existing archive.
        path: std::path::PathBuf,
        /// The layout to read it as (`podgrab`, `generic`); detected when
        /// omitted.
        #[arg(long)]
        format: Option<String>,
        /// Copy the files. Without it this is a dry run.
        #[arg(long)]
        apply: bool,
        /// Import only into this podcast, skipping the directory gate.
        #[arg(long)]
        podcast: Option<String>,
        /// Podgrab's `podgrab.db`, read without writing it, to match each
        /// file exactly; implies `--format podgrab`.
        #[arg(long)]
        podgrab_db: Option<std::path::PathBuf>,
    },
    /// Put missing archived files back from a folder: only a file with the
    /// record's exact bytes is copied, and only with `--apply`.
    Restore {
        /// A folder that may hold copies of the missing files.
        path: std::path::PathBuf,
        /// Copy the files back. Without it this is a dry run.
        #[arg(long)]
        apply: bool,
        /// Only this podcast's missing files.
        #[arg(long)]
        podcast: Option<String>,
    },
    /// Download an episode again whose archived file is missing.
    Redownload {
        /// The episode.
        episode_id: String,
    },
    /// Read or write the portable document beside an archived file.
    Sidecar {
        #[command(subcommand)]
        action: SidecarCommand,
    },
    /// Read, write or check the per-podcast checksum manifests.
    Manifest {
        #[command(subcommand)]
        action: ManifestCommand,
    },
    /// Show or fetch a podcast's artwork.
    Artwork {
        #[command(subcommand)]
        action: ArtworkCommand,
    },
    /// Write Uguisu's metadata into archived media files.
    Tags {
        #[command(subcommand)]
        action: TagsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SidecarCommand {
    /// Show the document beside one episode's media file.
    Show {
        /// Episode id.
        episode_id: String,
    },
    /// Write it, from what the archive record says.
    Write {
        /// Episode id.
        episode_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum ManifestCommand {
    /// Show which manifests are current and which have fallen behind.
    Status,
    /// Write the manifests that have fallen behind.
    Write {
        /// Only this podcast's manifest.
        #[arg(long)]
        podcast: Option<String>,
    },
    /// Compare a manifest with the archive.
    Verify {
        /// Podcast id.
        #[arg(long)]
        podcast: String,
        /// Re-read and hash the files instead of comparing against the
        /// database.
        #[arg(long)]
        full: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ArtworkCommand {
    /// Show the artwork Uguisu holds for a podcast.
    Show {
        /// Podcast id.
        podcast_id: String,
    },
    /// Fetch it from the URL the feed gives.
    Fetch {
        /// Podcast id.
        podcast_id: String,
        /// Fetch the bytes again even if the server says nothing changed.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TagsCommand {
    /// Show the tags a media file currently carries.
    Show {
        /// Episode id.
        episode_id: String,
    },
    /// Write Uguisu's metadata into it.
    Write {
        /// Episode id.
        episode_id: String,
        /// `fill_missing` writes only what is absent, `sync` makes the
        /// managed fields match Uguisu. Neither removes anything.
        #[arg(long)]
        mode: Option<String>,
    },
}

/// `podcast move-feed`.
#[derive(Debug, Args)]
pub(crate) struct MoveFeedArgs {
    /// Podcast ID.
    pub id: String,
    /// The feed URL to move to.
    pub url: String,
    /// Check only; never move.
    #[arg(long)]
    pub dry_run: bool,
    /// Move even when the feed fails the same-show check.
    #[arg(long)]
    pub force: bool,
}

/// `podcast import`.
#[derive(Debug, Args)]
pub(crate) struct ImportArgs {
    /// OPML file, or `-` for standard input.
    pub file: PathBuf,
    /// Verify and add the new feeds. Without it this is a dry run that sends no request.
    #[arg(long)]
    pub apply: bool,
    /// Store a policy with each podcast the import adds: `auto` or `manual`.
    #[arg(long)]
    pub mode: Option<String>,
    /// Episodes of each new podcast that may await archiving at once.
    #[arg(long, requires = "mode")]
    pub max_backlog: Option<u32>,
    /// Ignore episodes published longer ago than this (0 = no limit).
    #[arg(long, requires = "mode")]
    pub max_age_days: Option<u32>,
    /// Priority for jobs the policy creates.
    #[arg(long, requires = "mode")]
    pub priority: Option<String>,
}

#[derive(Debug, Subcommand)]
enum PolicyCommand {
    /// Show one podcast's policy and the values in force.
    Show {
        /// Podcast id.
        podcast_id: String,
    },
    /// Set one podcast's policy.
    Set {
        /// Podcast id.
        podcast_id: String,
        /// `auto` queues discovered episodes, `manual` queues nothing.
        #[arg(long)]
        mode: String,
        /// Episodes of this podcast that may await archiving at once.
        #[arg(long)]
        max_backlog: Option<u32>,
        /// Ignore episodes published longer ago than this (0 = no limit).
        #[arg(long)]
        max_age_days: Option<u32>,
        /// Priority for jobs the policy creates.
        #[arg(long)]
        priority: Option<String>,
    },
    /// Remove one podcast's policy, so the global defaults apply again.
    Clear {
        /// Podcast id.
        podcast_id: String,
    },
    /// List every stored per-podcast policy.
    List,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Validate configuration; fails when a stored value is being ignored.
    Validate,
    /// Show every key with its value, origin and provenance.
    List,
    /// Show one key.
    Get {
        /// The `UGUISU_*` name.
        key: String,
    },
    /// Store a value in the database (the environment still wins).
    Set {
        /// The `UGUISU_*` name.
        key: String,
        /// The value, in the same syntax the environment variable uses.
        value: String,
    },
    /// Clear a stored value.
    Unset {
        /// The `UGUISU_*` name.
        key: String,
    },
}

#[derive(Debug, Subcommand)]
enum SchedulerCommand {
    /// What the scheduler is doing.
    Status,
    /// Stop automatic refreshing (persisted across restarts).
    Pause {
        /// Why, for whoever reads it next.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Resume automatic refreshing.
    Resume,
    /// Run one pass now instead of waiting for the loop.
    Run,
    /// Run the housekeeping pass now.
    Maintenance,
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    /// Apply pending migrations, and say which schema the database had and has.
    Migrate,
    /// Write a consistent copy of the database; never overwrites a file (ADR 0056).
    Backup {
        /// Destination file; by default `<data dir>/backups/uguisu-<UTC time>.db`.
        /// With --server the server always picks its own `backups/` directory.
        path: Option<std::path::PathBuf>,
    },
    /// Run SQLite's integrity and foreign-key checks; exit 1 on a finding.
    Check,
    /// Rebuild the database file without its free pages.
    Vacuum,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Set the password, reading it from the terminal without echoing it.
    SetPassword {
        /// The operator's name. Keeps the current one, or `uguisu`, when unset.
        #[arg(long)]
        username: Option<String>,
    },
    /// API tokens for the CLI and automation.
    Token {
        #[command(subcommand)]
        action: TokenCommand,
    },
}

#[derive(Debug, Subcommand)]
enum TokenCommand {
    /// Issue a token and print its secret once.
    Create {
        /// A label, so the list is readable later.
        name: String,
        /// `read` or `write`.
        #[arg(long, default_value = "write")]
        scope: String,
        /// RFC 3339 instant after which it stops working.
        #[arg(long, value_name = "RFC3339")]
        expires_at: Option<String>,
    },
    /// List every token, revoked ones included. Never a secret.
    List,
    /// Stop a token working.
    Revoke {
        /// The token id from `auth token list`.
        id: String,
    },
}

fn init_tracing(filter: &str, format: LogFormat) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("warn"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Pretty => subscriber.init(),
        LogFormat::Json => subscriber.json().init(),
    }
}

/// Where discovery commands run.
enum Backend {
    Embedded(Box<Discovery>),
    Remote(client::ApiClient),
}

fn backend(cli: &Cli) -> Result<Backend, String> {
    if let Some(server) = &cli.server {
        return client::ApiClient::new(server, cli.token.as_ref())
            .map(Backend::Remote)
            .map_err(|e| e.to_string());
    }
    let config = DiscoveryConfig::from_env().map_err(|e| e.to_string())?;
    assemble(config)
        .map(|d| Backend::Embedded(Box::new(d)))
        .map_err(|e| e.to_string())
}

fn parse_providers(names: &[String]) -> Result<Option<Vec<ProviderId>>, String> {
    if names.is_empty() {
        return Ok(None);
    }
    names
        .iter()
        .map(|n| n.parse::<ProviderId>().map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

#[allow(clippy::print_stdout, clippy::print_stderr, clippy::too_many_lines)]
async fn run_search(cli: &Cli, args: &SearchArgs) -> Exit {
    let json = cli.json;
    let query = args.query.trim();
    if uguisu_discovery::NormalizedQuery::parse(query).is_url() {
        return run_resolve(cli, query).await;
    }
    let providers = match parse_providers(&args.provider) {
        Ok(p) => p,
        Err(e) => {
            output::error(json, "search", &e);
            return Exit::Usage;
        }
    };
    let request = SearchRequest {
        query: query.to_owned(),
        providers,
        limit: args.limit,
        country: args.country.clone(),
        no_cache: args.no_cache,
    };
    let backend = match backend(cli) {
        Ok(b) => b,
        Err(e) => {
            output::error(json, "search", &e);
            return Exit::Error;
        }
    };
    let cancel = CancellationToken::new();
    let response = match &backend {
        Backend::Embedded(d) => d.engine.search(&request, cancel.clone()).await,
        Backend::Remote(c) => match c.search(&request).await {
            Ok(r) => r,
            Err(e) => {
                output::error(json, "search", &e.to_string());
                return Exit::Error;
            }
        },
    };
    let exit = Exit::for_outcome(response.outcome);
    if !args.resolve || response.outcome != SearchOutcome::Results {
        if json {
            output::json(&response);
        } else {
            print!("{}", output::render_search(&response, args.explain));
        }
        return exit;
    }
    let top = &response.results[0].candidate;
    let resolved = match &backend {
        Backend::Embedded(d) => d.resolver.resolve_candidate(top, cancel).await,
        Backend::Remote(c) => {
            let Some(target) = top.feed_url.as_ref().or(top.website.as_ref()) else {
                output::error(
                    json,
                    "resolve",
                    "top result has neither a feed nor a website to resolve",
                );
                return Exit::Unresolvable;
            };
            match c.resolve(target.as_str()).await {
                Ok(r) => r,
                Err(e) => {
                    output::error(json, "resolve", &e.to_string());
                    return Exit::Error;
                }
            }
        }
    };
    match resolved {
        Ok(feed) => {
            if json {
                output::json(&serde_json::json!({ "search": response, "resolved": feed }));
            } else {
                print!("{}", output::render_search(&response, args.explain));
                print!("\nResolved top result:\n{}", output::render_resolved(&feed));
            }
            Exit::Ok
        }
        Err(failure) => {
            if json {
                output::json(
                    &serde_json::json!({ "search": response, "resolved": output::FailureBody::new(&failure) }),
                );
            } else {
                print!("{}", output::render_search(&response, args.explain));
                eprint!("{}", output::render_failure(&failure));
            }
            Exit::for_resolve_error(&failure.error)
        }
    }
}

#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn run_resolve(cli: &Cli, input: &str) -> Exit {
    let json = cli.json;
    let backend = match backend(cli) {
        Ok(b) => b,
        Err(e) => {
            output::error(json, "resolve", &e);
            return Exit::Error;
        }
    };
    let result = match &backend {
        Backend::Embedded(d) => d.resolver.resolve(input, CancellationToken::new()).await,
        Backend::Remote(c) => match c.resolve(input).await {
            Ok(r) => r,
            Err(e) => {
                output::error(json, "resolve", &e.to_string());
                return Exit::Error;
            }
        },
    };
    match result {
        Ok(feed) => {
            if json {
                output::json(&feed);
            } else {
                print!("{}", output::render_resolved(&feed));
            }
            Exit::Ok
        }
        Err(failure) => {
            if json {
                output::json(&output::FailureBody::new(&failure));
            } else {
                eprint!("{}", output::render_failure(&failure));
            }
            if matches!(failure.error, ResolveError::NotAUrl { .. }) {
                output::error(
                    json,
                    "resolve",
                    "input is not a URL; use `uguisu search podcast` to search by name",
                );
            }
            Exit::for_resolve_error(&failure.error)
        }
    }
}

/// The full configuration with the `--data-dir` override applied.
fn engine_config(cli: &Cli) -> Result<EngineConfig, String> {
    let mut config = Config::from_env().map_err(|e| e.to_string())?;
    if let Some(dir) = &cli.data_dir {
        config.data.data_dir = Some(dir.clone());
    }
    Ok(EngineConfig::from(config))
}

/// Opens the engine for an embedded stateful command.
async fn open_engine(cli: &Cli, command: &str) -> Result<Engine, Exit> {
    let config = engine_config(cli).map_err(|e| {
        output::error(cli.json, command, &e);
        Exit::Error
    })?;
    Engine::open(config).await.map_err(|e| {
        let hint = match &e {
            UguisuError::Locked(_) => {
                "; if `uguisu serve` is running, use --server http://127.0.0.1:8484"
            }
            _ => "",
        };
        output::error(cli.json, command, &format!("{e}{hint}"));
        Exit::for_engine_error(&e)
    })
}

/// `serve`: the API server, the download workers and the feed-refresh
/// scheduler, until Ctrl-C or SIGTERM; running downloads are parked as
/// `queued(shutdown)` on the way out.
///
/// This is the only command that starts anything long-lived. Everything
/// else does what it was asked and exits (ADR 0027).
async fn run_serve(cli: &Cli, args: &ServeArgs) -> Exit {
    let engine = match open_engine(cli, "serve").await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let credential_set = match engine.credential_set().await {
        Ok(set) => set,
        Err(e) => {
            output::error(cli.json, "serve", &e.to_string());
            engine.close().await;
            return Exit::for_engine_error(&e);
        }
    };
    // Refuse before starting the workers and the scheduler: a server that is
    // not going to serve should not have touched the queue either.
    if let Err(err) =
        uguisu_server::expose::check(args.bind, credential_set, args.allow_insecure_exposure)
    {
        engine.close().await;
        return refuse_exposed_bind(cli, args.bind, &err);
    }
    let discovery = engine.discovery().clone();
    let web = args.web.clone();
    let web = web.is_dir().then_some(web);
    tracing::info!(web = ?web, auth = credential_set, "web ui");
    let state = uguisu_server::AppState::new(discovery, Some(engine.clone()))
        .with_web_dir(web)
        .with_auth(uguisu_server::auth::AuthOptions {
            required: credential_set,
            cookie_secure: args.cookie_secure,
            allow_insecure_exposure: args.allow_insecure_exposure,
            trusted_proxies: args.trusted_proxies.clone(),
        });
    engine.start_downloads();
    engine.start_refresh_scheduler();
    engine.start_search_index();
    let result =
        uguisu_server::serve_with_shutdown(args.bind, state, download::shutdown_signal()).await;
    engine.close().await;
    match result {
        Ok(()) => Exit::Ok,
        Err(uguisu_server::expose::ServeError::InsecureExposure { bind }) => refuse_exposed_bind(
            cli,
            bind,
            &uguisu_server::expose::ServeError::InsecureExposure { bind },
        ),
        Err(err) => {
            output::error(cli.json, "serve", &format!("server failed: {err}"));
            Exit::Error
        }
    }
}

/// Explains a refused bind, and says every way out of it.
///
/// Four lines rather than one, because the operator has three real choices and
/// a message that named only the problem would send them looking for the
/// override in the source.
fn refuse_exposed_bind(
    cli: &Cli,
    bind: SocketAddr,
    err: &uguisu_server::expose::ServeError,
) -> Exit {
    if cli.json {
        output::error(true, "serve", &err.to_string());
    } else {
        output::error(
            false,
            "serve",
            &format!("refusing to bind {bind} without authentication"),
        );
        output::hint("set a credential first: uguisu auth set-password");
        output::hint("or bind to loopback: --bind 127.0.0.1:8484");
        output::hint("to override deliberately: UGUISU_AUTH_ALLOW_INSECURE_EXPOSURE=1");
    }
    Exit::InsecureExposure
}

#[allow(clippy::too_many_lines)] // one arm per subcommand; the table is the point
async fn run_archive(cli: &Cli, action: &ArchiveCommand) -> Exit {
    match action {
        ArchiveCommand::List {
            podcast,
            state,
            source_changed,
            limit,
        } => {
            archive::run_list(
                cli,
                podcast.as_deref(),
                state.as_deref(),
                *source_changed,
                *limit,
            )
            .await
        }
        ArchiveCommand::Show { episode_id } => archive::run_show(cli, episode_id).await,
        ArchiveCommand::Verify {
            episode_id,
            all,
            podcast,
            full,
        } => archive::run_verify(cli, episode_id.as_deref(), *all, podcast.as_deref(), *full).await,
        ArchiveCommand::Missing => archive::run_problem_list(cli, "missing").await,
        ArchiveCommand::Invalid => archive::run_problem_list(cli, "invalid").await,
        ArchiveCommand::PathPreview { episode_id } => {
            archive::run_path_preview(cli, episode_id).await
        }
        ArchiveCommand::Relocate {
            episode_id,
            all,
            podcast,
            dry_run,
        } => {
            archive::run_relocate(
                cli,
                episode_id.as_deref(),
                *all,
                podcast.as_deref(),
                *dry_run,
            )
            .await
        }
        ArchiveCommand::Policy { action } => match action {
            PolicyCommand::Show { podcast_id } => archive::run_policy_show(cli, podcast_id).await,
            PolicyCommand::Set {
                podcast_id,
                mode,
                max_backlog,
                max_age_days,
                priority,
            } => {
                archive::run_policy_set(
                    cli,
                    podcast_id,
                    mode,
                    *max_backlog,
                    *max_age_days,
                    priority.as_deref(),
                )
                .await
            }
            PolicyCommand::Clear { podcast_id } => archive::run_policy_clear(cli, podcast_id).await,
            PolicyCommand::List => archive::run_policy_list(cli).await,
        },
        ArchiveCommand::Reconcile {
            deep,
            rebuild,
            apply,
            podcast,
        } => {
            if *rebuild {
                archive::run_rebuild(cli, *apply, podcast.as_deref()).await
            } else {
                archive::run_reconcile(cli, *deep).await
            }
        }
        ArchiveCommand::Orphans => archive::run_orphans(cli).await,
        ArchiveCommand::Import {
            path,
            format,
            apply,
            podcast,
            podgrab_db,
        } => {
            archive::run_import(
                cli,
                path,
                format.as_deref(),
                *apply,
                podcast.as_deref(),
                podgrab_db.as_deref(),
            )
            .await
        }
        ArchiveCommand::Restore {
            path,
            apply,
            podcast,
        } => archive::run_restore(cli, path, *apply, podcast.as_deref()).await,
        ArchiveCommand::Redownload { episode_id } => archive::run_redownload(cli, episode_id).await,
        ArchiveCommand::Sidecar { action } => match action {
            SidecarCommand::Show { episode_id } => archive::run_sidecar_show(cli, episode_id).await,
            SidecarCommand::Write { episode_id } => {
                archive::run_sidecar_write(cli, episode_id).await
            }
        },
        ArchiveCommand::Manifest { action } => match action {
            ManifestCommand::Status => archive::run_manifest_status(cli).await,
            ManifestCommand::Write { podcast } => {
                archive::run_manifest_write(cli, podcast.as_deref()).await
            }
            ManifestCommand::Verify { podcast, full } => {
                archive::run_manifest_verify(cli, podcast, *full).await
            }
        },
        ArchiveCommand::Artwork { action } => match action {
            ArtworkCommand::Show { podcast_id } => {
                archive::run_artwork(cli, podcast_id, false, false).await
            }
            ArtworkCommand::Fetch { podcast_id, force } => {
                archive::run_artwork(cli, podcast_id, true, *force).await
            }
        },
        ArchiveCommand::Tags { action } => match action {
            TagsCommand::Show { episode_id } => archive::run_tags_show(cli, episode_id).await,
            TagsCommand::Write { episode_id, mode } => {
                archive::run_tags_write(cli, episode_id, mode.as_deref()).await
            }
        },
    }
}

async fn run_podcast(cli: &Cli, action: &PodcastCommand) -> Exit {
    match action {
        PodcastCommand::Add { input, yes } => library::run_podcast_add(cli, input, *yes).await,
        PodcastCommand::List => library::run_podcast_list(cli).await,
        PodcastCommand::Import(args) => library::run_podcast_import(cli, args).await,
        PodcastCommand::Export => library::run_podcast_export(cli).await,
        PodcastCommand::Show { id } => library::run_podcast_show(cli, id).await,
        PodcastCommand::MoveFeed(args) => library::run_move_feed(cli, args).await,
        PodcastCommand::Archive { id } => library::run_podcast_archive(cli, id).await,
        PodcastCommand::Remove { id, yes } => library::run_podcast_remove(cli, id, *yes).await,
        PodcastCommand::Refresh { id, all, force } => {
            library::run_podcast_refresh(cli, id.as_deref(), *all, *force).await
        }
        PodcastCommand::Pause { id } => service::run_podcast_pause(cli, id, true).await,
        PodcastCommand::Resume { id } => service::run_podcast_pause(cli, id, false).await,
        PodcastCommand::Schedule { id, at } => {
            service::run_podcast_schedule(cli, id, at.as_deref()).await
        }
    }
}

async fn run_episode(cli: &Cli, action: &EpisodeCommand) -> Exit {
    match action {
        EpisodeCommand::Download { id, priority, wait } => {
            download::run_enqueue(cli, id, priority, *wait).await
        }
        EpisodeCommand::Duplicates { podcast } => {
            library::run_duplicates(cli, podcast.as_deref()).await
        }
        EpisodeCommand::Resolve { id, same, .. } => {
            let resolution = if *same {
                DuplicateResolution::Same
            } else {
                DuplicateResolution::Separate
            };
            library::run_resolve(cli, id, resolution).await
        }
        EpisodeCommand::List { podcast_id } => library::run_episode_list(cli, podcast_id).await,
        EpisodeCommand::Show { id } => library::run_episode_show(cli, id).await,
    }
}

async fn run_download(cli: &Cli, args: &DownloadArgs) -> Exit {
    match &args.action {
        None => {
            download::run_enqueue(
                cli,
                args.episode_id.as_deref().unwrap_or_default(),
                &args.priority,
                args.wait,
            )
            .await
        }
        Some(DownloadCommand::Podcast { id, priority }) => {
            download::run_enqueue_podcast(cli, id, priority).await
        }
        Some(DownloadCommand::List {
            state,
            podcast,
            after,
            limit,
        }) => {
            download::run_list(
                cli,
                state.as_deref(),
                podcast.as_deref(),
                after.as_deref(),
                *limit,
            )
            .await
        }
        Some(DownloadCommand::Show { id }) => download::run_show(cli, id).await,
        Some(DownloadCommand::Cancel { id }) => download::run_job_command(cli, "cancel", id).await,
        Some(DownloadCommand::Pause { id }) => download::run_job_command(cli, "pause", id).await,
        Some(DownloadCommand::Resume { id }) => download::run_job_command(cli, "resume", id).await,
        Some(DownloadCommand::Retry { id }) => download::run_job_command(cli, "retry", id).await,
        Some(DownloadCommand::RetryFailed) => download::run_retry_failed(cli).await,
        Some(DownloadCommand::PauseAll) => download::run_control(cli, true).await,
        Some(DownloadCommand::ResumeAll) => download::run_control(cli, false).await,
        Some(DownloadCommand::Stats) => download::run_stats(cli).await,
        Some(DownloadCommand::Reconcile { deep }) => download::run_reconcile(cli, *deep).await,
        Some(DownloadCommand::Run { until_idle }) => download::run_workers(cli, *until_idle).await,
    }
}

#[tokio::main]
#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli.log, cli.log_format);

    let exit = match &cli.command {
        Command::Serve(args) => run_serve(&cli, args).await,
        Command::Health(args) => service::run_health(&cli, args.bind).await,
        Command::Search {
            what: SearchCommand::Podcast(args),
        } => run_search(&cli, args).await,
        Command::Resolve { input } => run_resolve(&cli, input).await,
        Command::Podcast { action } => run_podcast(&cli, action).await,
        Command::Feed {
            action: FeedCommand::Inspect { url },
        } => library::run_feed_inspect(&cli, url).await,
        Command::Feed {
            action: FeedCommand::Refresh { source_id, force },
        } => library::run_feed_refresh(&cli, source_id, *force).await,
        Command::Feed {
            action: FeedCommand::Status { source_id },
        } => library::run_feed_status(&cli, source_id).await,
        Command::Episode { action } => run_episode(&cli, action).await,
        Command::Download(args) => run_download(&cli, args).await,
        Command::Archive { action } => run_archive(&cli, action).await,
        Command::Search {
            what: SearchCommand::Library(args),
        } => service::run_search(&cli, &args.query, args.limit, args.prefix, args.explain).await,
        Command::Search {
            what: SearchCommand::Reindex,
        } => service::run_reindex(&cli).await,
        Command::Scheduler { action } => match action {
            SchedulerCommand::Status => service::run_status(&cli).await,
            SchedulerCommand::Pause { reason } => {
                service::run_pause(&cli, true, reason.as_deref()).await
            }
            SchedulerCommand::Resume => service::run_pause(&cli, false, None).await,
            SchedulerCommand::Run => service::run_pass(&cli).await,
            SchedulerCommand::Maintenance => service::run_maintenance(&cli).await,
        },
        Command::Config { action } => {
            let action = match action {
                ConfigCommand::Validate => service::ConfigAction::Validate,
                ConfigCommand::List => service::ConfigAction::List,
                ConfigCommand::Get { key } => service::ConfigAction::Get { key },
                ConfigCommand::Set { key, value } => service::ConfigAction::Set { key, value },
                ConfigCommand::Unset { key } => service::ConfigAction::Unset { key },
            };
            service::run_config(&cli, action).await
        }
        Command::Auth { action } => match action {
            AuthCommand::SetPassword { username } => {
                auth::run_set_password(&cli, username.as_deref()).await
            }
            AuthCommand::Token { action } => match action {
                TokenCommand::Create {
                    name,
                    scope,
                    expires_at,
                } => auth::run_token_create(&cli, name, scope, expires_at.as_deref()).await,
                TokenCommand::List => auth::run_token_list(&cli).await,
                TokenCommand::Revoke { id } => auth::run_token_revoke(&cli, id).await,
            },
        },
        Command::Db { action } => db::run(&cli, action).await,
    };
    exit.into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_brief_examples() {
        let examples = [
            vec!["uguisu", "search", "podcast", "Darknet Diaries"],
            vec![
                "uguisu",
                "search",
                "podcast",
                "darknet",
                "--provider",
                "apple,podcastindex",
                "--explain",
                "--limit",
                "5",
                "--country",
                "DE",
                "--resolve",
                "--no-cache",
            ],
            vec!["uguisu", "resolve", "https://example.com/feed.xml"],
            vec!["uguisu", "podcast", "add", "https://example.com/feed.xml"],
            vec!["uguisu", "podcast", "add", "--yes", "https://example.com/"],
            vec!["uguisu", "podcast", "list"],
            vec!["uguisu", "podcast", "import", "subscriptions.opml"],
            vec![
                "uguisu",
                "podcast",
                "import",
                "-",
                "--apply",
                "--mode",
                "auto",
                "--max-backlog",
                "2",
                "--priority",
                "low",
            ],
            vec!["uguisu", "podcast", "export"],
            vec!["uguisu", "podcast", "show", "01J"],
            vec!["uguisu", "podcast", "refresh", "01J"],
            vec!["uguisu", "podcast", "refresh", "01J", "--force"],
            vec!["uguisu", "podcast", "refresh", "--all"],
            vec!["uguisu", "podcast", "refresh", "--all", "--force"],
            vec!["uguisu", "feed", "inspect", "https://example.com/feed.xml"],
            vec!["uguisu", "feed", "refresh", "01J", "--force"],
            vec!["uguisu", "feed", "status", "01J"],
            vec!["uguisu", "--data-dir", "/tmp/x", "podcast", "list"],
            vec!["uguisu", "episode", "download", "01J"],
            vec!["uguisu", "episode", "download", "01J", "--wait"],
            vec!["uguisu", "download", "01J"],
            vec!["uguisu", "download", "01J", "--priority", "high", "--wait"],
            vec!["uguisu", "download", "podcast", "01J", "--priority", "low"],
            vec!["uguisu", "download", "list"],
            vec![
                "uguisu", "download", "list", "--state", "failed", "--limit", "5",
            ],
            vec!["uguisu", "download", "show", "01J"],
            vec!["uguisu", "download", "cancel", "01J"],
            vec!["uguisu", "download", "pause", "01J"],
            vec!["uguisu", "download", "resume", "01J"],
            vec!["uguisu", "download", "retry", "01J"],
            vec!["uguisu", "download", "retry-failed"],
            vec!["uguisu", "download", "pause-all"],
            vec!["uguisu", "download", "resume-all"],
            vec!["uguisu", "download", "stats"],
            vec!["uguisu", "download", "reconcile", "--deep"],
            vec!["uguisu", "download", "run", "--until-idle"],
            vec!["uguisu", "archive", "verify"],
            vec!["uguisu", "archive", "reconcile"],
            vec!["uguisu", "archive", "import", "/podcasts"],
            vec![
                "uguisu",
                "archive",
                "import",
                "/srv/podgrab/assets",
                "--podgrab-db",
                "/srv/podgrab/config/podgrab.db",
                "--apply",
            ],
            vec!["uguisu", "config", "validate"],
            vec!["uguisu", "--json", "download", "list"],
            vec![
                "uguisu",
                "serve",
                "--bind",
                "0.0.0.0:8484",
                "--allow-insecure-exposure",
            ],
            vec![
                "uguisu",
                "--server",
                "http://127.0.0.1:8484",
                "search",
                "podcast",
                "x",
            ],
        ];
        for example in examples {
            Cli::try_parse_from(&example).unwrap_or_else(|e| panic!("{example:?}: {e}"));
        }
    }

    #[test]
    fn command_lines_parse_or_refuse() {
        for args in [
            vec!["uguisu", "episode", "list", "01J"],
            vec!["uguisu", "episode", "show", "01J"],
            vec!["uguisu", "db", "backup"],
            vec!["uguisu", "db", "backup", "copy.db"],
            vec!["uguisu", "db", "check"],
            vec!["uguisu", "podcast", "remove", "01J", "--yes"],
            vec!["uguisu", "download", "01J"],
            vec!["uguisu", "podcast", "refresh", "--all"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
        // `download` needs an episode id or a subcommand, never both.
        assert!(Cli::try_parse_from(["uguisu", "download"]).is_err());
        assert!(Cli::try_parse_from(["uguisu", "download", "01J", "list"]).is_err());
        assert!(
            Cli::try_parse_from(["uguisu", "download", "01J", "--priority", "urgent"]).is_err()
        );
        assert!(Cli::try_parse_from(["uguisu", "queue", "list"]).is_err());
        assert!(Cli::try_parse_from(["uguisu", "podcast", "refresh"]).is_err());
        assert!(Cli::try_parse_from(["uguisu", "podcast", "refresh", "01J", "--all"]).is_err());
        assert!(
            Cli::try_parse_from(["uguisu", "podcast", "remove", "01J", "--delete-files"]).is_err()
        );
    }

    #[test]
    fn provider_list_parsing() {
        assert_eq!(parse_providers(&[]).unwrap(), None);
        assert_eq!(
            parse_providers(&["apple".into(), "PodcastIndex".into()]).unwrap(),
            Some(vec![ProviderId::APPLE, ProviderId::PODCAST_INDEX])
        );
        assert!(parse_providers(&["spotify".into()]).is_err());
    }
}
