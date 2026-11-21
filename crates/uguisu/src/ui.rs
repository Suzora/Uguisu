//! Server-rendered pages. Everything here returns HTML; maud escapes all interpolated text.

use std::collections::HashSet;

use maud::{DOCTYPE, Markup, html};
use uguisu_core::model::{EpisodeView, Podcast, PodcastSummary, Stats};
use uguisu_core::rss::media_url;
use uguisu_core::search::{Provider, SearchHit};

use crate::i18n::{Lang, t};

/// The emblem without the wordmark. Decorative: the name is always written next to it.
fn icon() -> Markup {
    html! { img.logo src="/assets/icon.webp" alt="" width="256" height="256"; }
}

// ── Formatting helpers ─────────────────────────────────────────────────────────────────────

fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if unit > 0 && value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    };
    match crate::i18n::current() {
        Lang::En => text,
        Lang::De => text.replace('.', ","),
    }
}

fn format_date(timestamp: i64) -> String {
    let pattern = match crate::i18n::current() {
        Lang::En => "%-d %b %Y",
        Lang::De => "%d.%m.%Y",
    };
    uguisu_core::util::format_utc(timestamp, pattern).unwrap_or_default()
}

fn format_datetime(timestamp: i64) -> String {
    let pattern = match crate::i18n::current() {
        Lang::En => "%-d %b %Y, %H:%M UTC",
        Lang::De => "%d.%m.%Y, %H:%M UTC",
    };
    uguisu_core::util::format_utc(timestamp, pattern).unwrap_or_default()
}

fn format_duration(seconds: i64) -> String {
    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Feed descriptions contain arbitrary HTML. The UI shows them as plain text only.
fn plain_text(html: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max_chars {
        let cut: String = collapsed.chars().take(max_chars).collect();
        format!("{}…", cut.trim_end())
    } else {
        collapsed
    }
}

fn percent(part: i64, total: i64) -> i64 {
    if total <= 0 {
        0
    } else {
        (part * 100 / total).clamp(0, 100)
    }
}

// ── Layout ─────────────────────────────────────────────────────────────────────────────────

fn document(title: &str, body_class: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang=(crate::i18n::current().code()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                meta name="htmx-config" content=r#"{"includeIndicatorStyles":false}"#;
                title { (t(title)) " · Uguisu" }
                link rel="icon" href="/assets/favicon.png" type="image/png";
                link rel="stylesheet" href="/assets/app.css";
                script src="/assets/htmx.min.js" defer {}
                script src="/assets/app.js" defer {}
            }
            body class=(body_class) { (content) }
        }
    }
}

/// Page frame for the logged-in interface.
fn shell(title: &str, active: &str, content: Markup) -> Markup {
    let nav_class = |name: &str| {
        if name == active {
            "nav-link is-active"
        } else {
            "nav-link"
        }
    };
    document(
        title,
        "app",
        html! {
            a.skip-link href="#content" { (t("Skip to the content")) }
            header.topbar {
                div.topbar-inner {
                    a.brand href="/" { (icon()) span { "Uguisu" } }
                    nav aria-label=(t("Main")) {
                        @for (name, address, label) in [
                            ("library", "/", "Library"),
                            ("add", "/add", "Add podcast"),
                            ("status", "/status", "Status"),
                            ("settings", "/settings", "Settings"),
                        ] {
                            a class=(nav_class(name)) href=(address)
                                aria-current=[(name == active).then_some("page")] { (t(label)) }
                        }
                    }
                    form.logout method="post" action="/logout" {
                        button.button.is-quiet type="submit" { (t("Sign out")) }
                    }
                }
            }
            main.container #content tabindex="-1" { (content) }
            footer.footer {
                "Uguisu " (env!("CARGO_PKG_VERSION")) (t(" · free software under the AGPL-3.0-or-later"))
            }
        },
    )
}

/// Centered card for setup, login and error pages.
fn card_page(title: &str, content: Markup) -> Markup {
    document(
        title,
        "centered",
        html! {
            main.auth-card {
                div.auth-brand { (icon()) span { "Uguisu" } }
                (content)
            }
        },
    )
}

pub fn message_page(title: &str, message: &str) -> Markup {
    card_page(
        title,
        html! {
            h1 { (t(title)) }
            p.muted { (t(message)) }
            a.button href="/" { (t("Back to the library")) }
        },
    )
}

pub fn setup_page(error: Option<&str>) -> Markup {
    card_page(
        "Welcome",
        html! {
            h1 { (t("Welcome")) }
            p.muted { (t("Choose a password to protect your archive. It is the only account on this instance.")) }
            @if let Some(error) = error { p.notice.is-error role="alert" { (t(error)) } }
            form.stack method="post" action="/setup" {
                label { (t("Password"))
                    input type="password" name="password" required minlength="10" autocomplete="new-password" autofocus;
                }
                label { (t("Repeat password"))
                    input type="password" name="confirm" required minlength="10" autocomplete="new-password";
                }
                button.button.is-primary type="submit" { (t("Create archive")) }
            }
        },
    )
}

/// What the source feed of a podcast did over time, and every stored version of it.
pub fn history_page(
    podcast: &Podcast,
    snapshots: &[uguisu_core::model::FeedSnapshot],
    events: &[uguisu_core::model::HistoryEvent],
) -> Markup {
    // Events of the same kind seen at the same check belong together: adding a podcast
    // brings hundreds of episodes at once.
    let mut groups: Vec<&[uguisu_core::model::HistoryEvent]> = Vec::new();
    let mut start = 0;
    for index in 1..=events.len() {
        let ends = index == events.len()
            || events[index].at != events[start].at
            || events[index].kind != events[start].kind;
        if ends {
            groups.push(&events[start..index]);
            start = index;
        }
    }
    shell(
        &format!("{} {}", t("History of"), podcast.title),
        "library",
        html! {
            p.muted.small { a href=(format!("/podcasts/{}", podcast.id)) { "← " (podcast.title) } }
            h1 { (t("History")) }
            @if let Some(error) = &podcast.last_fetch_error {
                p.notice.is-error { (t("The last check of the source feed failed: ")) (error) }
            }
            section.panel {
                h2 { (t("What changed in the source feed")) }
                @if groups.is_empty() {
                    p.muted { (t("Nothing yet.")) }
                }
                ol.revision-list {
                    @for group in &groups {
                        @let first = &group[0];
                        li {
                            strong { (format_datetime(first.at)) }
                            " "
                            @match first.kind.as_str() {
                                "added" => span.tag { (t("New")) },
                                "changed" => span.tag { (t("Changed")) },
                                _ => span.tag.is-error { (t("Gone from feed")) },
                            }
                            @if group.len() > 8 {
                                span.muted { " " (group.len()) (t(" episodes")) }
                            } @else {
                                ul.plain {
                                    @for event in group.iter() {
                                        li { a href=(format!("/podcasts/{}/episodes/{}", podcast.id, event.episode_id)) { (event.title) } }
                                    }
                                }
                            }
                        }
                    }
                }
                @if events.len() >= 500 {
                    p.muted.small { (t("Older events are not listed here; they are in the archive.")) }
                }
            }
            section.panel {
                h2 { (t("Stored versions of the source feed")) }
                p.muted.small {
                    (t("Every distinct version of the feed is kept exactly as the server sent it. "))
                    (snapshots.len()) @if snapshots.len() == 1 { (t(" version")) } @else { (t(" versions")) } (t(" so far."))
                }
                ul.file-list {
                    @for snapshot in snapshots {
                        li {
                            div {
                                a href=(format!("/podcasts/{}/snapshots/{}", podcast.id, snapshot.id)) download {
                                    (format_datetime(snapshot.fetched_at))
                                }
                                span.muted.small { " · " (format_bytes(snapshot.size)) }
                                div.muted.small { "BLAKE3 " code { (snapshot.blake3) } }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// What the status page shows.
pub struct StatusReport<'a> {
    pub podcasts: &'a [PodcastSummary],
    pub stats: &'a Stats,
    pub problems: &'a [uguisu_core::model::DownloadProblem],
    pub damaged: &'a [uguisu_core::model::DamagedFile],
    /// Downloads running at this moment.
    pub downloading: usize,
    pub free_space: Option<u64>,
    pub reserve: u64,
}

/// What the archive is doing and what needs attention.
pub fn status_page(status: &StatusReport) -> Markup {
    let waiting: i64 = status.podcasts.iter().map(|p| p.pending_count).sum();
    let feed_errors: Vec<&PodcastSummary> = status
        .podcasts
        .iter()
        .filter(|p| p.podcast.last_fetch_error.is_some())
        .collect();
    let (gave_up, retrying): (Vec<_>, Vec<_>) = status
        .problems
        .iter()
        .partition(|problem| problem.download_attempts >= 5);
    let space_low = status
        .free_space
        .is_some_and(|free| status.reserve > 0 && free < status.reserve);
    let all_well = feed_errors.is_empty()
        && status.problems.is_empty()
        && status.damaged.is_empty()
        && !space_low;
    shell(
        "Status",
        "status",
        html! {
            h1 { (t("Status")) }
            @if all_well {
                p.notice role="status" { (t("Everything is in order.")) }
            }
            section.stats {
                div.stat {
                    span.stat-value { (status.downloading) }
                    span.stat-label { (t("Downloading now")) }
                }
                div.stat title=(t("Episodes whose files are still to be downloaded")) {
                    span.stat-value { (waiting) }
                    span.stat-label { (t("Waiting")) }
                }
                div class=(if gave_up.is_empty() { "stat" } else { "stat is-error" }) {
                    span.stat-value { (gave_up.len()) }
                    span.stat-label { (t("Downloads given up")) }
                }
                div class=(if status.damaged.is_empty() { "stat" } else { "stat is-error" }) {
                    span.stat-value { (status.damaged.len()) }
                    span.stat-label { (t("Damaged or missing files")) }
                }
                div.stat {
                    span.stat-value { (format_bytes(status.stats.bytes)) }
                    span.stat-label { (t("In the archive")) }
                }
                @if let Some(free) = status.free_space {
                    div class=(if space_low { "stat is-error" } else { "stat" })
                        title=(t("Downloads wait while less than {} is free").replace("{}", &format_bytes(status.reserve as i64))) {
                        span.stat-value { (format_bytes(free as i64)) }
                        span.stat-label { (t("Free on the disk")) }
                    }
                }
            }
            @if space_low {
                p.notice.is-error {
                    (t("The disk is almost full. Downloads are on hold until more than "))
                    (format_bytes(status.reserve as i64)) (t(" is free again."))
                }
            }
            @if let Some(oldest) = status.stats.oldest_check {
                p.muted.small {
                    (t("Every archived file has been checked against its checksum since "))
                    (format_date(oldest)) "."
                }
            }

            @if !feed_errors.is_empty() {
                section.panel {
                    h2 { (t("Feeds that could not be checked")) }
                    ul.file-list {
                        @for p in &feed_errors {
                            li {
                                div {
                                    a href=(format!("/podcasts/{}", p.podcast.id)) { (p.podcast.title) }
                                    div.muted.small { (p.podcast.last_fetch_error.as_deref().unwrap_or("")) }
                                }
                                @if let Some(checked) = p.podcast.last_fetched_at {
                                    span.muted.small { (t("last tried ")) (format_datetime(checked)) }
                                }
                            }
                        }
                    }
                }
            }

            @if !status.problems.is_empty() {
                section.panel {
                    h2 { (t("Downloads that failed")) }
                    p.muted.small {
                        (t("A download is tried five times, with growing pauses. After that it is given up until you ask for another try."))
                    }
                    form method="post" action="/status/retry" {
                        button.button type="submit" { (t("Try all of them again")) }
                    }
                    ul.file-list {
                        @for problem in gave_up.iter().chain(retrying.iter()) {
                            li {
                                div {
                                    a href=(format!("/podcasts/{}/episodes/{}", problem.podcast_id, problem.episode_id)) { (problem.title) }
                                    span.muted.small { " · " (problem.podcast_title) }
                                    div.muted.small { (problem.download_error) }
                                }
                                @if problem.download_attempts >= 5 {
                                    span.pill.is-error { (t("Given up")) }
                                } @else {
                                    span.pill.is-warn title=(format!("{} {}", t("Next attempt"), format_datetime(problem.next_attempt_at))) { (t("Retrying")) }
                                }
                            }
                        }
                    }
                }
            }

            @if !status.damaged.is_empty() {
                section.panel {
                    h2 { (t("Damaged or missing files")) }
                    p.muted.small {
                        (t("These files no longer match their checksums or are gone from the disk. They are downloaded again while the source still offers them."))
                    }
                    ul.file-list {
                        @for file in status.damaged {
                            li {
                                div {
                                    a href=(format!("/podcasts/{}/episodes/{}", file.podcast_id, file.episode_id)) { (file.title) }
                                    span.muted.small { " · " (file.podcast_title) }
                                    div.muted.small { code { (file.path) } }
                                }
                                @if file.status == "missing" {
                                    span.pill.is-error { (t("Missing")) }
                                } @else {
                                    span.pill.is-error { (t("Damaged")) }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// Results of a search in the archive.
pub fn find_page(query: &str, results: &[uguisu_core::model::SearchResult]) -> Markup {
    shell(
        "Search",
        "library",
        html! {
            h1 { (t("Search the archive")) }
            form.inline.find method="get" action="/find" role="search" {
                input type="search" name="q" value=(query) autofocus
                    placeholder=(t("Search titles, show notes and transcripts…")) aria-label=(t("Search the archive"));
                button.button.is-primary type="submit" { (t("Search")) }
            }
            @if query.is_empty() {
                p.muted { (t("Every word has to occur in the title, the show notes or the transcript of an episode. The last word may be the beginning of a word.")) }
            } @else if results.is_empty() {
                p.muted { (t("No episode matches “")) (query) (t("”.")) }
            } @else {
                section.panel {
                    p.muted.small {
                        (results.len()) @if results.len() == 1 { (t(" episode")) } @else { (t(" episodes")) }
                        @if results.len() >= 100 { (t(" – the best 100 matches")) }
                    }
                    ol.episode-list {
                        @for hit in results {
                            li.episode {
                                div.episode-main {
                                    a.episode-title href=(format!("/podcasts/{}/episodes/{}", hit.podcast_id, hit.episode_id)) { (hit.title) }
                                    div.episode-meta {
                                        span { (hit.podcast_title) }
                                        @if let Some(date) = hit.published_at { span { (format_date(date)) } }
                                    }
                                    @if !hit.snippet.trim().is_empty() {
                                        p.muted.small { (marked(&hit.snippet)) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// A search snippet with its matches highlighted. The text is escaped first; only the marks
/// are markup.
fn marked(snippet: &str) -> Markup {
    // The escaping drops control characters, so the marks travel as private-use characters.
    let carried = snippet
        .replace('\u{1}', "\u{e000}")
        .replace('\u{2}', "\u{e001}");
    let escaped = uguisu_core::util::xml_escape(&carried)
        .replace('\u{e000}', "<mark>")
        .replace('\u{e001}', "</mark>");
    maud::PreEscaped(escaped)
}

pub fn settings_page(
    sessions: i64,
    defaults: uguisu_core::model::NewPodcastDefaults,
    webhook: Option<&str>,
    notice: Option<&str>,
    error: Option<&str>,
) -> Markup {
    shell(
        "Settings",
        "settings",
        html! {
            h1 { (t("Settings")) }
            @if let Some(notice) = notice { p.notice role="status" { (t(notice)) } }
            div.columns {
                section.panel {
                    h2 { (t("Password")) }
                    @if let Some(error) = error { p.notice.is-error role="alert" { (t(error)) } }
                    form.stack method="post" action="/settings/password" {
                        label { (t("Current password"))
                            input type="password" name="current" required autocomplete="current-password";
                        }
                        label { (t("New password"))
                            input type="password" name="password" required minlength="10" autocomplete="new-password";
                        }
                        label { (t("Repeat new password"))
                            input type="password" name="confirm" required minlength="10" autocomplete="new-password";
                        }
                        button.button.is-primary type="submit" { (t("Change password")) }
                    }
                    p.muted.small {
                        (t("Changing the password signs out all other devices. If the password is lost, "))
                        code { "uguisu reset-password" } (t(" on the server lets you choose a new one."))
                    }
                }
                section.panel {
                    h2 { (t("Signed-in devices")) }
                    p.muted {
                        @if sessions <= 1 {
                            (t("Only this browser is signed in."))
                        } @else {
                            (sessions) (t(" browsers are signed in, this one included."))
                        }
                    }
                    form method="post" action="/settings/sessions" {
                        button.button type="submit" disabled[sessions <= 1] { (t("Sign out all other devices")) }
                    }
                    p.muted.small {
                        (t("Podcast apps are not affected: feeds are protected by their secret addresses, not by the login."))
                    }
                }
                section.panel {
                    h2 { (t("New podcasts")) }
                    p.muted.small { (t("What a podcast starts with when it is added. Each podcast can be set differently afterwards; podcasts already in the archive are not changed.")) }
                    form.stack method="post" action="/settings/new-podcasts" {
                        label.check {
                            input type="checkbox" name="include_removed" checked[defaults.include_removed];
                            (t("Publish episodes that left the source feed"))
                        }
                        label.check {
                            input type="checkbox" name="keep_versions" checked[defaults.keep_versions];
                            (t("Keep a new copy when the source replaces a file"))
                        }
                        label.check {
                            input type="checkbox" name="only_new" checked[defaults.only_new];
                            (t("Only episodes from the day it is added"))
                        }
                        button.button type="submit" { (t("Save")) }
                    }
                }
                section.panel {
                    h2 { (t("Language")) }
                    form.stack method="post" action="/settings/language" {
                        label { (t("Language of this interface"))
                            select name="language" {
                                @for lang in Lang::ALL {
                                    option value=(lang.code()) selected[lang == crate::i18n::current()] { (lang.name()) }
                                }
                            }
                        }
                        button.button type="submit" { (t("Save")) }
                    }
                    p.muted.small {
                        (t("Error messages from feeds and servers, and everything in the archive, stay as they are."))
                    }
                }
                section.panel {
                    h2 { (t("Notifications")) }
                    @if let Some(host) = webhook {
                        p.muted {
                            (t("Damaged or missing files and feeds that keep failing are reported to "))
                            code { (host) } "."
                        }
                        form method="post" action="/settings/notification" {
                            button.button type="submit" { (t("Send a test notification")) }
                        }
                    } @else {
                        p.muted {
                            (t("Uguisu can report damaged files and feeds that keep failing to another service. "))
                            (t("Start it with ")) code { "UGUISU_WEBHOOK_URL" } (t(" set to the address that should receive the reports."))
                        }
                    }
                }
            }
        },
    )
}

pub fn login_page(error: Option<&str>) -> Markup {
    card_page(
        "Sign in",
        html! {
            h1 { (t("Sign in")) }
            @if let Some(error) = error { p.notice.is-error role="alert" { (t(error)) } }
            form.stack method="post" action="/login" {
                label { (t("Password"))
                    input type="password" name="password" required autocomplete="current-password" autofocus;
                }
                button.button.is-primary type="submit" { (t("Sign in")) }
            }
        },
    )
}

// ── Library ────────────────────────────────────────────────────────────────────────────────

fn cover_image(podcast: &Podcast, class: &str) -> Markup {
    html! {
        @if podcast.cover_file.is_some() {
            img class=(class) src=(format!("/cover/{}?size=small", podcast.id)) alt="" loading="lazy";
        } @else {
            div class=(format!("{class} is-placeholder")) aria-hidden="true" {
                (podcast.title.chars().next().unwrap_or('?'))
            }
        }
    }
}

pub fn library_page(
    podcasts: &[PodcastSummary],
    stats: &Stats,
    import: Option<usize>,
    space_low: bool,
    combined_feed: &str,
) -> Markup {
    shell(
        "Library",
        "library",
        html! {
            @if space_low {
                p.notice {
                    (t("The disk is almost full. Downloads are on hold until there is free space again; feeds are still checked."))
                }
            }
            @if let Some(count) = import {
                p.notice {
                    (t("Importing ")) (count) (t(" feeds in the background. Podcasts appear here as they are fetched – reload to see progress."))
                }
            }
            @if podcasts.is_empty() {
                section.empty {
                    (icon())
                    h1 { (t("Your archive is empty")) }
                    p.muted { (t("Add a podcast and Uguisu keeps every episode, even after it disappears from the feed.")) }
                    a.button.is-primary href="/add" { (t("Add your first podcast")) }
                }
            } @else {
                h1.visually-hidden { (t("Library")) }
                form.inline.find method="get" action="/find" role="search" {
                    input type="search" name="q" placeholder=(t("Search titles, show notes and transcripts in your archive…")) aria-label=(t("Search the archive"));
                    button.button type="submit" { (t("Search")) }
                }
                section.stats {
                    div.stat { span.stat-value { (stats.podcasts) } span.stat-label { (t("Podcasts")) } }
                    div.stat {
                        span.stat-value { (stats.archived) small { " / " (stats.episodes) } }
                        span.stat-label { (t("Episodes archived")) }
                    }
                    div.stat { span.stat-value { (format_bytes(stats.bytes)) } span.stat-label { (t("On disk")) } }
                    @if stats.damaged > 0 {
                        div.stat.is-error title=(t("Damaged or missing files are downloaded again while the source still offers them")) {
                            span.stat-value { (stats.damaged) }
                            span.stat-label { (t("Damaged files")) }
                        }
                    } @else if let Some(oldest) = stats.oldest_check {
                        div.stat.is-ok title=(t("Every archived file matched its recorded checksum when it was last read")) {
                            span.stat-value { (t("Intact")) }
                            span.stat-label { (t("All files checked since ")) (format_date(oldest)) }
                        }
                    }
                    div.stat-actions {
                        a.button href="/opml" title=(t("Subscription list of all archive feeds, for import into a podcast app")) { (t("Export OPML")) }
                    }
                }
                section.panel {
                    h2 { (t("One feed for everything")) }
                    p.muted.small {
                        (t("The newest 100 episodes of all podcasts in a single feed. Treat the address like a password."))
                    }
                    div.copy-row {
                        input #combined-feed type="text" readonly value=(combined_feed) aria-label=(t("Address of the feed over all podcasts"));
                        button.button type="button" data-copy="#combined-feed" data-copied=(t("Copied")) { (t("Copy")) }
                    }
                }
                section.grid {
                    @for summary in podcasts {
                        @let p = &summary.podcast;
                        a.card href=(format!("/podcasts/{}", p.id)) {
                            div.card-cover {
                                (cover_image(p, "cover"))
                                @if p.last_fetch_error.is_some() {
                                    span.badge.is-error title=(t("The last feed check failed")) { "!" }
                                }
                            }
                            div.card-body {
                                h2 { (p.title) }
                                @if let Some(author) = &p.author { p.muted { (author) } }
                                div.progress title=(format!("{}{}{}{}", summary.archived_count, t(" of "), summary.episode_count, t(" episodes archived"))) {
                                    progress max="100" value=(percent(summary.archived_count, summary.episode_count)) {}
                                    span { (summary.archived_count) " / " (summary.episode_count) }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

// ── Add ────────────────────────────────────────────────────────────────────────────────────

pub fn add_page(error: Option<&str>, url: &str, only_new: bool) -> Markup {
    shell(
        "Add podcast",
        "add",
        html! {
            h1.page-title { (t("Add a podcast")) }
            @if let Some(error) = error { p.notice.is-error role="alert" { (t(error)) } }

            section.panel {
                h2 { (t("Search")) }
                input.search type="search" name="q" placeholder=(t("Search podcast directories…")) autocomplete="off" autofocus
                    hx-get="/search" hx-trigger="input changed delay:400ms, search" hx-target="#results"
                    hx-sync="this:replace";
                div #results {}
            }

            div.columns {
                section.panel {
                    h2 { (t("Feed address")) }
                    p.muted { (t("Paste the feed address of a podcast, including private feeds with a token in the address. The address of the podcast's web page usually works too.")) }
                    form.inline method="post" action="/podcasts" {
                        input type="text" name="url" value=(url) placeholder="https://example.org/feed.xml" required inputmode="url";
                        button.button.is-primary type="submit" { (t("Archive")) }
                        label.check title=(t("Earlier episodes are listed, but their files are not downloaded. This can be changed later.")) {
                            input type="checkbox" name="only_new" checked[only_new];
                            (t("Only episodes from today on"))
                        }
                    }
                }
                section.panel {
                    h2 { (t("Import OPML")) }
                    p.muted { (t("Bring the subscription list exported from your podcast app.")) }
                    form.inline method="post" action="/opml" enctype="multipart/form-data" {
                        input type="file" name="file" aria-label=(t("OPML file")) accept=".opml,.xml,text/xml,application/xml,text/x-opml" required;
                        button.button type="submit" { (t("Import")) }
                    }
                }
            }
        },
    )
}

/// One self-loading section per directory.
pub fn search_placeholders(query: &str) -> Markup {
    html! {
        @if query.chars().count() >= 2 {
            @for provider in Provider::ALL {
                div.provider-results
                    hx-get=(format!("/search/{}", provider.key())) hx-vals=(serde_query(query))
                    hx-trigger="load" hx-swap="outerHTML" {
                    p.muted.searching { (t("Searching ")) (provider.name()) "…" }
                }
            }
        }
    }
}

fn serde_query(query: &str) -> String {
    // JSON object for hx-vals; maud escapes it for the attribute.
    let mut escaped = String::with_capacity(query.len() + 8);
    for c in query.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            c if (c as u32) < 0x20 => escaped.push(' '),
            c => escaped.push(c),
        }
    }
    format!("{{\"q\":\"{escaped}\"}}")
}

/// Results of one directory; `None` means the directory could not be reached.
pub fn provider_results(provider: Provider, hits: Option<&[SearchHit]>, only_new: bool) -> Markup {
    html! {
        div.provider-results {
            @match hits {
                None => p.muted.small { (provider.name()) (t(" did not answer.")) },
                Some([]) => p.muted.small { (t("No further results from ")) (provider.name()) "." },
                Some(hits) => div.results {
                    @for hit in hits {
                        form.result method="post" action="/podcasts" {
                            @if let Some(image) = hit.image_url.as_deref().filter(|u| u.starts_with("https://")) {
                                img.result-cover src=(image) alt="" loading="lazy" referrerpolicy="no-referrer";
                            } @else {
                                div.result-cover.is-placeholder aria-hidden="true" { (hit.title.chars().next().unwrap_or('?')) }
                            }
                            div.result-body {
                                strong { (hit.title) }
                                @if let Some(author) = &hit.author { span.muted { (author) } }
                                span.muted.small { (t("via ")) (hit.provider) }
                            }
                            input type="hidden" name="url" value=(hit.feed_url);
                            @if only_new { input type="hidden" name="only_new" value="on"; }
                            button.button.is-primary type="submit" { (t("Archive")) }
                        }
                    }
                },
            }
        }
    }
}

// ── Podcast ────────────────────────────────────────────────────────────────────────────────

pub fn podcast_page(
    summary: &PodcastSummary,
    episodes: &[EpisodeView],
    downloading: &HashSet<i64>,
    feed_url: &str,
    page: i64,
) -> Markup {
    let p = &summary.podcast;
    let removed = summary.removed_count;
    shell(
        &p.title,
        "library",
        html! {
            section.hero {
                @if p.cover_file.is_some() {
                    img.hero-backdrop src=(format!("/cover/{}?size=small", p.id)) alt="" aria-hidden="true";
                }
                div.hero-content {
                    (cover_image(p, "hero-cover"))
                    div.hero-text {
                        h1 { (p.title) }
                        @if let Some(author) = &p.author { p.hero-author { (author) } }
                        @if let Some(description) = &p.description {
                            p.hero-description { (plain_text(description, 420)) }
                        }
                        ul.facts {
                            li { strong { (summary.archived_count) } (t(" of ")) (summary.episode_count) (t(" episodes archived")) }
                            li { strong { (format_bytes(summary.archived_bytes)) } (t(" on disk")) }
                            @if removed > 0 {
                                li { strong { (removed) } (t(" no longer in the source feed")) }
                            }
                            @if let Some(checked) = p.last_fetched_at {
                                li { (t("Checked ")) (format_datetime(checked)) }
                            }
                        }
                    }
                }
            }

            @if let Some(error) = &p.last_fetch_error {
                p.notice.is-error role="alert" { strong { (t("The last feed check failed. ")) } (error) }
            }

            section.panel {
                h2 { (t("Archive feed")) }
                p.muted {
                    (t("Subscribe to this address in your podcast app. It serves the archived files and keeps working when the original feed is gone. Anyone who knows the address can listen, so treat it like a password."))
                }
                div.copy-row {
                    input #feed-url type="text" readonly value=(feed_url) aria-label=(t("Archive feed address"));
                    button.button.is-primary type="button" data-copy="#feed-url" data-copied=(t("Copied")) { (t("Copy")) }
                }
                p.muted.small {
                    (t("For a shorter feed, add ")) code { "?limit=50" } (t(" to the address: only the newest 50 episodes are published."))
                }
                div.actions {
                    form method="post" action=(format!("/podcasts/{}/refresh", p.id)) {
                        button.button type="submit" { (t("Check source feed now")) }
                    }
                    form method="post" action=(format!("/podcasts/{}/settings", p.id)) {
                        label.check {
                            input type="checkbox" name="include_removed" checked[p.include_removed] data-autosubmit;
                            (t("Publish episodes that left the source feed"))
                        }
                        label.check title=(t("When the address of an episode's media file changes, download it again and keep both copies. Uses more disk space if a podcast moves to another host.")) {
                            input type="checkbox" name="keep_versions" checked[p.keep_versions] data-autosubmit;
                            (t("Keep a new copy when the source replaces a file"))
                        }
                        label.check title=(t("Episodes published before this day are listed, but their files are not downloaded. Leave empty to archive everything. Files already archived are never removed.")) {
                            (t("Archive episodes from "))
                            input type="date" name="archive_from" data-autosubmit
                                value=[p.archive_from.and_then(|t| uguisu_core::util::format_utc(t, "%Y-%m-%d"))];
                        }
                        noscript { button.button type="submit" { (t("Save")) } }
                    }
                    form method="post" action=(format!("/podcasts/{}/token", p.id))
                        data-confirm=(t("Create a new feed address? Apps using the current address lose access.")) {
                        button.button.is-quiet type="submit" { (t("New feed address")) }
                    }
                }
                p.small { a href=(format!("/podcasts/{}/history", p.id)) { (t("History of the source feed")) } }
                @let source = uguisu_core::http::redact(&p.feed_url);
                p.muted.small { (t("Source: ")) a href=(source) rel="noreferrer noopener" { (source) } }
            }

            (episode_list(summary, episodes, downloading, page))

            details.panel.danger {
                summary { (t("Remove this podcast")) }
                p.muted {
                    (t("Uguisu stops checking the source feed and the archive feed stops working. The archived files stay in "))
                    code { "archive/" (p.dir) }
                    (t(" unless you choose to delete them."))
                }
                form.actions method="post" action=(format!("/podcasts/{}/remove", p.id))
                    data-confirm=(t("Remove this podcast from Uguisu?")) {
                    label.check {
                        input type="checkbox" name="delete_files";
                        (t("Also delete the archived files (")) (format_bytes(summary.archived_bytes)) ")"
                    }
                    button.button.is-danger type="submit" { (t("Remove podcast")) }
                }
            }
        },
    )
}

enum Status<'a> {
    Archived(i64),
    Damaged,
    Downloading,
    Queued,
    /// Published before the podcast's start date.
    Skipped,
    Retrying(&'a str),
    Failed(&'a str),
    NoMedia,
}

fn status_of<'a>(
    view: &'a EpisodeView,
    downloading: &HashSet<i64>,
    archive_from: Option<i64>,
) -> Status<'a> {
    let e = &view.episode;
    if let Some(size) = view.media_size {
        Status::Archived(size)
    } else if downloading.contains(&e.id) {
        Status::Downloading
    } else if view.media_damaged > 0 && e.download_attempts >= 5 {
        Status::Damaged
    } else if e.enclosure_url.is_none() {
        Status::NoMedia
    } else if let (Some(from), Some(published)) = (archive_from, e.published_at)
        && published < from
    {
        Status::Skipped
    } else if e.download_attempts >= 5 {
        Status::Failed(e.download_error.as_deref().unwrap_or("unknown error"))
    } else if let Some(error) = e.download_error.as_deref() {
        Status::Retrying(error)
    } else {
        Status::Queued
    }
}

/// Splits a feed entry so that every element starts a line of its own. Feeds are often written
/// without line breaks, and a comparison by lines would then show the whole entry as changed.
fn entry_lines(xml: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = xml.chars().peekable();
    while let Some(c) = chars.next() {
        // A closing tag stays with the text it closes, but not with another element.
        let closes_text = chars.peek() == Some(&'/') && !current.trim_end().ends_with('>');
        if c == '<' && !closes_text && !current.trim().is_empty() {
            lines.push(current.trim().to_string());
            current.clear();
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        lines.push(current.trim().to_string());
    }
    lines
}

/// What changed between two states of a feed entry: `(added, line)` for every line that is
/// only in one of them, in the order of the entry. `None` if the entries are too long to
/// compare at a reasonable cost.
fn entry_changes(old: &str, new: &str) -> Option<Vec<(bool, String)>> {
    let (a, b) = (entry_lines(old), entry_lines(new));
    if a.len() * b.len() > 4_000_000 {
        return None;
    }
    // Longest common subsequence, by the table of suffix lengths.
    let mut table = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut changes = Vec::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            changes.push((false, a[i].clone()));
            i += 1;
        } else {
            changes.push((true, b[j].clone()));
            j += 1;
        }
    }
    changes.extend(a[i..].iter().map(|line| (false, line.clone())));
    changes.extend(b[j..].iter().map(|line| (true, line.clone())));
    Some(changes)
}

/// Show notes as readable text: paragraphs, line breaks and list items become lines, all other
/// markup is dropped. The notes come from the publisher, so they are never shown as HTML.
fn notes_text(html: &str, max_chars: usize) -> String {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].trim().to_ascii_lowercase();
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or("");
        match name {
            "br" | "tr" => text.push('\n'),
            "li" if !tag.starts_with('/') => text.push('\n'),
            "p" | "div" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" => {
                text.push_str("\n\n");
            }
            _ => {}
        }
        if name == "li" && !tag.starts_with('/') {
            text.push_str("• ");
        }
        rest = &rest[start + end + 1..];
    }
    text.push_str(rest);

    // Character references, named and numeric.
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let end = tail.find(';').filter(|end| *end <= 8);
        let replacement = end.and_then(|end| {
            let name = &tail[..end];
            let character = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((character, end))
        });
        match replacement {
            Some((character, end)) => {
                decoded.push(character);
                rest = &tail[end + 1..];
            }
            None => {
                decoded.push('&');
                rest = tail;
            }
        }
    }
    decoded.push_str(rest);

    // Tidy: single spaces within a line, at most one empty line in a row.
    let mut out = String::new();
    let mut empty = 0;
    for line in decoded.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() || line == "•" {
            empty += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if empty > 0 { "\n\n" } else { "\n" });
        }
        empty = 0;
        out.push_str(&line);
    }
    shorten(&out, max_chars)
}

fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let cut: String = text.chars().take(max_chars).collect();
        format!("{cut}…")
    } else {
        text.to_string()
    }
}

/// Everything the archive holds about one episode.
pub fn episode_page(
    podcast: &Podcast,
    view: &EpisodeView,
    media: &[uguisu_core::model::MediaFile],
    attachments: &[uguisu_core::model::AttachmentDetail],
    revisions: &[uguisu_core::model::Revision],
    downloading: &HashSet<i64>,
) -> Markup {
    let e = &view.episode;
    let file_name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
    let newest_ok = media.iter().rev().find(|m| m.status == "ok").map(|m| m.id);
    let skipped = matches!(
        status_of(view, downloading, podcast.archive_from),
        Status::Skipped
    );
    shell(
        &e.title,
        "library",
        html! {
            p.muted.small { a href=(format!("/podcasts/{}", podcast.id)) { "← " (podcast.title) } }
            section.panel {
                h1 { (e.title) }
                div.episode-meta {
                    @if let Some(date) = e.published_at { span { (format_date(date)) } }
                    @if let Some(duration) = e.duration { span { (format_duration(duration)) } }
                    @if let (Some(season), Some(number)) = (e.season, e.episode_no) {
                        span { (t("Season ")) (season) (t(", episode ")) (number) }
                    } @else if let Some(number) = e.episode_no {
                        span { (t("Episode ")) (number) }
                    }
                    @if let Some(removed) = e.removed_at {
                        span.tag title=(t("This episode is no longer listed in the source feed")) {
                            (t("Gone from feed since ")) (format_date(removed))
                        }
                    }
                    @match status_of(view, downloading, podcast.archive_from) {
                        Status::Archived(size) => span.pill.is-ok { (t("Archived · ")) (format_bytes(size)) },
                        Status::Damaged => span.pill.is-error { (t("Damaged")) },
                        Status::Downloading => span.pill.is-busy { (t("Downloading")) },
                        Status::Queued => span.pill { (t("Queued")) },
                        Status::Skipped => span.pill { (t("Not archived")) },
                        Status::Retrying(_) => span.pill.is-warn { (t("Retrying")) },
                        Status::Failed(_) => span.pill.is-error { (t("Failed")) },
                        Status::NoMedia => span.pill { (t("No media")) },
                    }
                }
                @if let Some(error) = &e.download_error {
                    p.notice.is-error { (t("Last download attempt: ")) (error) }
                    form method="post" action=(format!("/podcasts/{}/episodes/{}/retry", podcast.id, e.id)) {
                        button.button type="submit" { (t("Try the download again")) }
                    }
                }
                @if let Some(link) = e.link.as_deref().filter(|l| l.starts_with("https://") || l.starts_with("http://")) {
                    p.small { a href=(link) rel="noreferrer noopener" { (link) } }
                }
                p.muted.small { (t("First seen ")) (format_datetime(e.first_seen_at)) (t(" · identifier ")) code { (shorten(&e.guid, 80)) } }
            }

            section.panel {
                h2 { (t("Files")) }
                @if media.is_empty() && attachments.is_empty() {
                    p.muted { (t("Nothing is archived for this episode yet.")) }
                }
                ul.file-list {
                    @for m in media {
                        @let name = file_name(&m.path);
                        li {
                            div {
                                @if m.status == "ok" {
                                    @if Some(m.id) == newest_ok {
                                        a href=(media_url("", &podcast.feed_token, e.id, &name)) { (name) }
                                    } @else {
                                        span { (name) }
                                    }
                                } @else {
                                    span { (name) }
                                }
                                span.muted.small {
                                    " · " (format_bytes(m.size)) " · " (m.mime)
                                    (t(" · downloaded ")) (format_datetime(m.downloaded_at))
                                }
                                div.muted.small {
                                    "BLAKE3 " code { (m.blake3) }
                                    (t(" · last verified ")) (format_datetime(m.verified_at))
                                }
                            }
                            @match m.status.as_str() {
                                "ok" if Some(m.id) == newest_ok => span.pill.is-ok { (t("Published")) },
                                "ok" => span.pill { (t("Earlier version")) },
                                "missing" => span.pill.is-error { (t("Missing")) },
                                _ => span.pill.is-error { (t("Damaged")) },
                            }
                        }
                    }
                    @for a in attachments {
                        li {
                            div {
                                @if let (Some(path), "ok") = (&a.path, a.status.as_str()) {
                                    @let name = file_name(path);
                                    a href=(format!("/media/{}/{}/a/{}/{}", podcast.feed_token, e.id, a.id, name)) { (name) }
                                } @else {
                                    span { (shorten(&uguisu_core::http::redact(&a.source_url), 90)) }
                                }
                                span.muted.small {
                                    " · " (a.kind)
                                    @if let Some(size) = a.size { " · " (format_bytes(size)) }
                                    @if let Some(mime) = &a.mime { " · " (mime) }
                                }
                                @if let Some(error) = &a.error {
                                    div.muted.small { (error) }
                                }
                            }
                            @match a.status.as_str() {
                                "ok" => span.pill.is-ok { (t("Archived")) },
                                "pending" if a.error.is_some() => span.pill.is-warn { (t("Not archived")) },
                                "pending" if skipped => span.pill { (t("Not archived")) },
                                "pending" => span.pill { (t("Queued")) },
                                "missing" => span.pill.is-error { (t("Missing")) },
                                _ => span.pill.is-error { (t("Damaged")) },
                            }
                        }
                    }
                }
            }

            @if let Some(description) = e.description.as_deref().filter(|d| !d.trim().is_empty()) {
                section.panel {
                    h2 { (t("Show notes")) }
                    p.muted.small { (t("As text. The published feed carries the original formatting.")) }
                    p.show-notes { (notes_text(description, 20_000)) }
                }
            }

            section.panel {
                h2 { (t("History of the feed entry")) }
                @if revisions.len() <= 1 {
                    p.muted { (t("The publisher has not changed this entry since it was first seen.")) }
                } @else {
                    p.muted.small {
                        (t("The publisher changed this entry ")) (revisions.len() - 1)
                        @if revisions.len() == 2 { (t(" time")) } @else { (t(" times")) }
                        (t(". Every state is kept in the archive."))
                    }
                }
                ol.revision-list {
                    @for (index, revision) in revisions.iter().enumerate().rev() {
                        li {
                            strong { (format_datetime(revision.seen_at)) }
                            @if revision.meta_hash == e.meta_hash { " " span.tag { (t("Current")) } }
                            @if index == 0 {
                                span.muted.small { (t(" · first seen")) }
                            } @else {
                                @match entry_changes(&revisions[index - 1].raw_xml, &revision.raw_xml) {
                                    Some(changes) if changes.is_empty() => {
                                        p.muted.small { (t("Only spacing or the order of attributes changed.")) }
                                    }
                                    Some(changes) => {
                                        pre.diff {
                                            @for (added, line) in changes.iter().take(60) {
                                                @if *added {
                                                    span.diff-add { "+ " (shorten(line, 400)) "\n" }
                                                } @else {
                                                    span.diff-del { "− " (shorten(line, 400)) "\n" }
                                                }
                                            }
                                            @if changes.len() > 60 {
                                                span.muted { (t("… and ")) (changes.len() - 60) (t(" more lines\n")) }
                                            }
                                        }
                                    }
                                    None => p.muted.small { (t("Too long to compare here; both states are in the archive.")) },
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// The episode list. While downloads are outstanding it reloads itself every few seconds.
pub fn episode_list(
    summary: &PodcastSummary,
    episodes: &[EpisodeView],
    downloading: &HashSet<i64>,
    page: i64,
) -> Markup {
    let podcast = &summary.podcast;
    let statuses: Vec<Status> = episodes
        .iter()
        .map(|v| status_of(v, downloading, podcast.archive_from))
        .collect();
    // A replaced source file is fetched while the earlier copy stays published.
    let refetching =
        |view: &EpisodeView| view.episode.refetch && view.episode.download_attempts < 5;
    // Work may be going on for episodes on other pages, so this is counted for the podcast.
    let busy = summary.pending_count > 0;
    let poll_url = format!("/podcasts/{}/episodes?page={page}", podcast.id);
    let per_page = uguisu_core::service::EPISODES_PER_PAGE;
    let pages = ((summary.episode_count + per_page - 1) / per_page).max(1);
    let first = (page - 1) * per_page + 1;
    let page_link = |number: i64| format!("/podcasts/{}?page={number}#episodes", podcast.id);
    html! {
        section #episodes .panel.episodes
            hx-get=[busy.then_some(poll_url.as_str())]
            hx-trigger=[busy.then_some("every 4s")]
            hx-swap=[busy.then_some("outerHTML")] {
            h2 {
                (t("Episodes"))
                @if busy { span.pill.is-busy { (t("Archiving · ")) (summary.pending_count) (t(" left")) } }
            }
            @if summary.episode_count == 0 {
                p.muted { (t("The feed has no episodes yet.")) }
            } @else if episodes.is_empty() {
                p.muted { (t("There is no page ")) (page) "." }
            }
            @if pages > 1 {
                nav.pager aria-label=(t("Episode pages")) {
                    @if page > 1 {
                        a.button href=(page_link(page - 1)) rel="prev" { (t("Newer")) }
                    }
                    span.muted {
                        @if !episodes.is_empty() {
                            (first) "–" (first + episodes.len() as i64 - 1) (t(" of "))
                        }
                        (summary.episode_count)
                    }
                    @if page < pages {
                        a.button href=(page_link(page + 1)) rel="next" { (t("Older")) }
                    }
                }
            }
            ol.episode-list {
                @for (view, status) in episodes.iter().zip(&statuses) {
                    @let e = &view.episode;
                    li.episode {
                        div.episode-main {
                            a.episode-title href=(format!("/podcasts/{}/episodes/{}", podcast.id, e.id)) { (e.title) }
                            div.episode-meta {
                                @if let Some(date) = e.published_at { span { (format_date(date)) } }
                                @if let Some(duration) = e.duration { span { (format_duration(duration)) } }
                                @if e.removed_at.is_some() {
                                    span.tag title=(t("This episode is no longer listed in the source feed")) { (t("Gone from feed")) }
                                }
                                @if view.attachment_count > 0 {
                                    span.tag title=(t("Transcripts, chapter files and episode images archived with this episode")) {
                                        "+" (view.attachment_count) @if view.attachment_count == 1 { (t(" file")) } @else { (t(" files")) }
                                    }
                                }
                                @if refetching(view) {
                                    span.tag title=(t("The source replaced the media file; the new copy is being downloaded")) { (t("Fetching new version")) }
                                }
                                @if view.media_versions - view.media_damaged > 1 {
                                    span.tag title=(t("The source replaced the media file; earlier copies are kept in the archive and the newest is published")) {
                                        (view.media_versions - view.media_damaged) (t(" versions"))
                                    }
                                }
                                @if view.media_damaged > 0 && view.media_size.is_some() {
                                    span.tag.is-error title=(t("An earlier copy failed its integrity check")) { (t("Damaged copy")) }
                                }
                            }
                        }
                        @match status {
                            Status::Archived(size) => span.pill.is-ok title=(t("Archived and hashed")) { (t("Archived · ")) (format_bytes(*size)) },
                            Status::Damaged => span.pill.is-error title=(t("The archived file failed its integrity check and could not be downloaded again")) { (t("Damaged")) },
                            Status::Downloading => span.pill.is-busy { (t("Downloading")) },
                            Status::Queued => span.pill { (t("Queued")) },
                            Status::Skipped => span.pill title=(t("Published before this podcast's start date; the file is not downloaded")) { (t("Not archived")) },
                            Status::Retrying(error) => span.pill.is-warn title=(error) { (t("Retrying")) },
                            Status::Failed(error) => span.pill.is-error title=(error) { (t("Failed")) },
                            Status::NoMedia => span.pill title=(t("The feed lists no media file for this episode")) { (t("No media")) },
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_between_feed_entries() {
        let old = "<item><title>One</title><guid>1</guid><enclosure url=\"a\"/></item>";
        let new =
            "<item><title>One, corrected</title><guid>1</guid><enclosure url=\"a\"/><x:y/></item>";
        let changes: Vec<String> = entry_changes(old, new)
            .unwrap()
            .into_iter()
            .map(|(added, line)| format!("{} {line}", if added { '+' } else { '-' }))
            .collect();
        assert_eq!(
            changes,
            [
                "- <title>One</title>",
                "+ <title>One, corrected</title>",
                "+ <x:y/>",
            ]
        );
        assert!(entry_changes(old, old).unwrap().is_empty());
        // Line breaks and indentation alone are no change.
        let spaced =
            "<item>\n  <title>One</title>\n  <guid>1</guid>\n  <enclosure url=\"a\"/></item>";
        assert!(entry_changes(old, spaced).unwrap().is_empty());
    }

    #[test]
    fn show_notes_become_readable_text() {
        let html = "<p>First &amp; <b>bold</b>&#8211;dash&#x21;</p>\n<p>Second<br/>line</p>\
                    <ul><li>one</li><li>two &lt;3</li></ul><script>alert(1)</script>";
        assert_eq!(
            notes_text(html, 1000),
            "First & bold–dash!\n\nSecond\nline\n\n• one\n• two <3\n\nalert(1)"
        );
        assert_eq!(
            notes_text("AT&T & co; 5 &unknown; x", 100),
            "AT&T & co; 5 &unknown; x"
        );
        assert_eq!(notes_text("<p>abcdef</p>", 3), "abc…");
    }

    #[test]
    fn formatting() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(52 * 1024 * 1024), "52 MB");
        assert_eq!(format_duration(59), "0:59");
        assert_eq!(format_duration(3723), "1:02:03");
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(0, 0), 0);
    }

    #[test]
    fn query_becomes_valid_json() {
        assert_eq!(serde_query("radio lab"), r#"{"q":"radio lab"}"#);
        assert_eq!(serde_query(r#"a"b\c"#), r#"{"q":"a\"b\\c"}"#);
    }

    #[test]
    fn descriptions_become_plain_text() {
        assert_eq!(
            plain_text(
                "<p>Hello <b>world</b></p><script>alert(1)</script> &amp; more",
                100
            ),
            "Hello world alert(1) & more"
        );
        assert_eq!(plain_text("abcdefgh", 4), "abcd…");
    }
}
