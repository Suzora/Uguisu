//! Server-rendered pages. Everything here returns HTML; maud escapes all interpolated text.

use std::collections::HashSet;

use maud::{DOCTYPE, Markup, html};
use uguisu_core::model::{EpisodeView, Podcast, PodcastSummary, Stats};
use uguisu_core::rss::media_url;
use uguisu_core::search::{Provider, SearchHit};

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
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn format_date(timestamp: i64) -> String {
    uguisu_core::util::format_utc(timestamp, "%-d %b %Y").unwrap_or_default()
}

fn format_datetime(timestamp: i64) -> String {
    uguisu_core::util::format_utc(timestamp, "%-d %b %Y, %H:%M UTC").unwrap_or_default()
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
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                meta name="htmx-config" content=r#"{"includeIndicatorStyles":false}"#;
                title { (title) " · Uguisu" }
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
            header.topbar {
                div.topbar-inner {
                    a.brand href="/" { (icon()) span { "Uguisu" } }
                    nav {
                        a class=(nav_class("library")) href="/" { "Library" }
                        a class=(nav_class("add")) href="/add" { "Add podcast" }
                    }
                    form.logout method="post" action="/logout" {
                        button.button.is-quiet type="submit" { "Sign out" }
                    }
                }
            }
            main.container { (content) }
            footer.footer {
                "Uguisu " (env!("CARGO_PKG_VERSION")) " · free software under the AGPL-3.0-or-later"
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
            h1 { (title) }
            p.muted { (message) }
            a.button href="/" { "Back to the library" }
        },
    )
}

pub fn setup_page(error: Option<&str>) -> Markup {
    card_page(
        "Welcome",
        html! {
            h1 { "Welcome" }
            p.muted { "Choose a password to protect your archive. It is the only account on this instance." }
            @if let Some(error) = error { p.notice.is-error role="alert" { (error) } }
            form.stack method="post" action="/setup" {
                label { "Password"
                    input type="password" name="password" required minlength="10" autocomplete="new-password" autofocus;
                }
                label { "Repeat password"
                    input type="password" name="confirm" required minlength="10" autocomplete="new-password";
                }
                button.button.is-primary type="submit" { "Create archive" }
            }
        },
    )
}

pub fn login_page(error: Option<&str>) -> Markup {
    card_page(
        "Sign in",
        html! {
            h1 { "Sign in" }
            @if let Some(error) = error { p.notice.is-error role="alert" { (error) } }
            form.stack method="post" action="/login" {
                label { "Password"
                    input type="password" name="password" required autocomplete="current-password" autofocus;
                }
                button.button.is-primary type="submit" { "Sign in" }
            }
        },
    )
}

// ── Library ────────────────────────────────────────────────────────────────────────────────

fn cover_image(podcast: &Podcast, class: &str) -> Markup {
    html! {
        @if podcast.cover_file.is_some() {
            img class=(class) src=(format!("/cover/{}", podcast.id)) alt="" loading="lazy";
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
) -> Markup {
    shell(
        "Library",
        "library",
        html! {
            @if space_low {
                p.notice {
                    "The disk is almost full. Downloads are on hold until there is free space again; feeds are still checked."
                }
            }
            @if let Some(count) = import {
                p.notice {
                    "Importing " (count) " feeds in the background. Podcasts appear here as they are fetched – reload to see progress."
                }
            }
            @if podcasts.is_empty() {
                section.empty {
                    (icon())
                    h1 { "Your archive is empty" }
                    p.muted { "Add a podcast and Uguisu keeps every episode, even after it disappears from the feed." }
                    a.button.is-primary href="/add" { "Add your first podcast" }
                }
            } @else {
                section.stats {
                    div.stat { span.stat-value { (stats.podcasts) } span.stat-label { "Podcasts" } }
                    div.stat {
                        span.stat-value { (stats.archived) small { " / " (stats.episodes) } }
                        span.stat-label { "Episodes archived" }
                    }
                    div.stat { span.stat-value { (format_bytes(stats.bytes)) } span.stat-label { "On disk" } }
                    @if stats.damaged > 0 {
                        div.stat.is-error title="Damaged or missing files are downloaded again while the source still offers them" {
                            span.stat-value { (stats.damaged) }
                            span.stat-label { "Damaged files" }
                        }
                    } @else if let Some(oldest) = stats.oldest_check {
                        div.stat.is-ok title="Every archived file matched its recorded checksum when it was last read" {
                            span.stat-value { "Intact" }
                            span.stat-label { "All files checked since " (format_date(oldest)) }
                        }
                    }
                    div.stat-actions {
                        a.button href="/opml" title="Subscription list of all archive feeds, for import into a podcast app" { "Export OPML" }
                    }
                }
                section.grid {
                    @for summary in podcasts {
                        @let p = &summary.podcast;
                        a.card href=(format!("/podcasts/{}", p.id)) {
                            div.card-cover {
                                (cover_image(p, "cover"))
                                @if p.last_fetch_error.is_some() {
                                    span.badge.is-error title="The last feed check failed" { "!" }
                                }
                            }
                            div.card-body {
                                h2 { (p.title) }
                                @if let Some(author) = &p.author { p.muted { (author) } }
                                div.progress title=(format!("{} of {} episodes archived", summary.archived_count, summary.episode_count)) {
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

pub fn add_page(error: Option<&str>, url: &str) -> Markup {
    shell(
        "Add podcast",
        "add",
        html! {
            h1.page-title { "Add a podcast" }
            @if let Some(error) = error { p.notice.is-error role="alert" { (error) } }

            section.panel {
                h2 { "Search" }
                input.search type="search" name="q" placeholder="Search podcast directories…" autocomplete="off" autofocus
                    hx-get="/search" hx-trigger="input changed delay:400ms, search" hx-target="#results"
                    hx-sync="this:replace";
                div #results {}
            }

            div.columns {
                section.panel {
                    h2 { "Feed address" }
                    p.muted { "Paste the feed address of a podcast, including private feeds with a token in the address. The address of the podcast's web page usually works too." }
                    form.inline method="post" action="/podcasts" {
                        input type="text" name="url" value=(url) placeholder="https://example.org/feed.xml" required inputmode="url";
                        button.button.is-primary type="submit" { "Archive" }
                        label.check title="Earlier episodes are listed, but their files are not downloaded. This can be changed later." {
                            input type="checkbox" name="only_new";
                            "Only episodes from today on"
                        }
                    }
                }
                section.panel {
                    h2 { "Import OPML" }
                    p.muted { "Bring the subscription list exported from your podcast app." }
                    form.inline method="post" action="/opml" enctype="multipart/form-data" {
                        input type="file" name="file" accept=".opml,.xml,text/xml,application/xml,text/x-opml" required;
                        button.button type="submit" { "Import" }
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
                    p.muted.searching { "Searching " (provider.name()) "…" }
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
pub fn provider_results(provider: Provider, hits: Option<&[SearchHit]>) -> Markup {
    html! {
        div.provider-results {
            @match hits {
                None => p.muted.small { (provider.name()) " did not answer." },
                Some([]) => p.muted.small { "No further results from " (provider.name()) "." },
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
                                span.muted.small { "via " (hit.provider) }
                            }
                            input type="hidden" name="url" value=(hit.feed_url);
                            button.button.is-primary type="submit" { "Archive" }
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
                    img.hero-backdrop src=(format!("/cover/{}", p.id)) alt="" aria-hidden="true";
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
                            li { strong { (summary.archived_count) } " of " (summary.episode_count) " episodes archived" }
                            li { strong { (format_bytes(summary.archived_bytes)) } " on disk" }
                            @if removed > 0 {
                                li { strong { (removed) } " no longer in the source feed" }
                            }
                            @if let Some(checked) = p.last_fetched_at {
                                li { "Checked " (format_datetime(checked)) }
                            }
                        }
                    }
                }
            }

            @if let Some(error) = &p.last_fetch_error {
                p.notice.is-error role="alert" { strong { "The last feed check failed. " } (error) }
            }

            section.panel {
                h2 { "Archive feed" }
                p.muted {
                    "Subscribe to this address in your podcast app. It serves the archived files and keeps working when the original feed is gone. Anyone who knows the address can listen, so treat it like a password."
                }
                div.copy-row {
                    input #feed-url type="text" readonly value=(feed_url) aria-label="Archive feed address";
                    button.button.is-primary type="button" data-copy="#feed-url" { "Copy" }
                }
                p.muted.small {
                    "For a shorter feed, add " code { "?limit=50" } " to the address: only the newest 50 episodes are published."
                }
                div.actions {
                    form method="post" action=(format!("/podcasts/{}/refresh", p.id)) {
                        button.button type="submit" { "Check source feed now" }
                    }
                    form method="post" action=(format!("/podcasts/{}/settings", p.id)) {
                        label.check {
                            input type="checkbox" name="include_removed" checked[p.include_removed] data-autosubmit;
                            "Publish episodes that left the source feed"
                        }
                        label.check title="When the address of an episode's media file changes, download it again and keep both copies. Uses more disk space if a podcast moves to another host." {
                            input type="checkbox" name="keep_versions" checked[p.keep_versions] data-autosubmit;
                            "Keep a new copy when the source replaces a file"
                        }
                        label.check title="Episodes published before this day are listed, but their files are not downloaded. Leave empty to archive everything. Files already archived are never removed." {
                            "Archive episodes from "
                            input type="date" name="archive_from" data-autosubmit
                                value=[p.archive_from.and_then(|t| uguisu_core::util::format_utc(t, "%Y-%m-%d"))];
                        }
                        noscript { button.button type="submit" { "Save" } }
                    }
                    form method="post" action=(format!("/podcasts/{}/token", p.id))
                        data-confirm="Create a new feed address? Apps using the current address lose access." {
                        button.button.is-quiet type="submit" { "New feed address" }
                    }
                }
                @let source = uguisu_core::http::redact(&p.feed_url);
                p.muted.small { "Source: " a href=(source) rel="noreferrer noopener" { (source) } }
            }

            (episode_list(summary, episodes, downloading, page))

            details.panel.danger {
                summary { "Remove this podcast" }
                p.muted {
                    "Uguisu stops checking the source feed and the archive feed stops working. The archived files stay in "
                    code { "archive/" (p.dir) }
                    " unless you choose to delete them."
                }
                form.actions method="post" action=(format!("/podcasts/{}/remove", p.id))
                    data-confirm="Remove this podcast from Uguisu?" {
                    label.check {
                        input type="checkbox" name="delete_files";
                        "Also delete the archived files (" (format_bytes(summary.archived_bytes)) ")"
                    }
                    button.button.is-danger type="submit" { "Remove podcast" }
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
                "Episodes"
                @if busy { span.pill.is-busy { "Archiving · " (summary.pending_count) " left" } }
            }
            @if summary.episode_count == 0 {
                p.muted { "The feed has no episodes yet." }
            } @else if episodes.is_empty() {
                p.muted { "There is no page " (page) "." }
            }
            @if pages > 1 {
                nav.pager aria-label="Episode pages" {
                    @if page > 1 {
                        a.button href=(page_link(page - 1)) rel="prev" { "Newer" }
                    }
                    span.muted {
                        @if !episodes.is_empty() {
                            (first) "–" (first + episodes.len() as i64 - 1) " of "
                        }
                        (summary.episode_count)
                    }
                    @if page < pages {
                        a.button href=(page_link(page + 1)) rel="next" { "Older" }
                    }
                }
            }
            ol.episode-list {
                @for (view, status) in episodes.iter().zip(&statuses) {
                    @let e = &view.episode;
                    li.episode {
                        div.episode-main {
                            @if let Some(file) = view.media_file_name() {
                                a.episode-title href=(media_url("", &podcast.feed_token, e.id, file)) { (e.title) }
                            } @else {
                                span.episode-title { (e.title) }
                            }
                            div.episode-meta {
                                @if let Some(date) = e.published_at { span { (format_date(date)) } }
                                @if let Some(duration) = e.duration { span { (format_duration(duration)) } }
                                @if e.removed_at.is_some() {
                                    span.tag title="This episode is no longer listed in the source feed" { "Gone from feed" }
                                }
                                @if view.attachment_count > 0 {
                                    span.tag title="Transcripts, chapter files and episode images archived with this episode" {
                                        "+" (view.attachment_count) @if view.attachment_count == 1 { " file" } @else { " files" }
                                    }
                                }
                                @if refetching(view) {
                                    span.tag title="The source replaced the media file; the new copy is being downloaded" { "Fetching new version" }
                                }
                                @if view.media_versions - view.media_damaged > 1 {
                                    span.tag title="The source replaced the media file; earlier copies are kept in the archive and the newest is published" {
                                        (view.media_versions - view.media_damaged) " versions"
                                    }
                                }
                                @if view.media_damaged > 0 && view.media_size.is_some() {
                                    span.tag.is-error title="An earlier copy failed its integrity check" { "Damaged copy" }
                                }
                            }
                        }
                        @match status {
                            Status::Archived(size) => span.pill.is-ok title="Archived and hashed" { "Archived · " (format_bytes(*size)) },
                            Status::Damaged => span.pill.is-error title="The archived file failed its integrity check and could not be downloaded again" { "Damaged" },
                            Status::Downloading => span.pill.is-busy { "Downloading" },
                            Status::Queued => span.pill { "Queued" },
                            Status::Skipped => span.pill title="Published before this podcast's start date; the file is not downloaded" { "Not archived" },
                            Status::Retrying(error) => span.pill.is-warn title=(error) { "Retrying" },
                            Status::Failed(error) => span.pill.is-error title=(error) { "Failed" },
                            Status::NoMedia => span.pill title="The feed lists no media file for this episode" { "No media" },
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
