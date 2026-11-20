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
                section.panel {
                    h2 { "One feed for everything" }
                    p.muted.small {
                        "The newest 100 episodes of all podcasts in a single feed. Treat the address like a password."
                    }
                    div.copy-row {
                        input #combined-feed type="text" readonly value=(combined_feed) aria-label="Address of the feed over all podcasts";
                        button.button type="button" data-copy="#combined-feed" { "Copy" }
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
                        span { "Season " (season) ", episode " (number) }
                    } @else if let Some(number) = e.episode_no {
                        span { "Episode " (number) }
                    }
                    @if let Some(removed) = e.removed_at {
                        span.tag title="This episode is no longer listed in the source feed" {
                            "Gone from feed since " (format_date(removed))
                        }
                    }
                    @match status_of(view, downloading, podcast.archive_from) {
                        Status::Archived(size) => span.pill.is-ok { "Archived · " (format_bytes(size)) },
                        Status::Damaged => span.pill.is-error { "Damaged" },
                        Status::Downloading => span.pill.is-busy { "Downloading" },
                        Status::Queued => span.pill { "Queued" },
                        Status::Skipped => span.pill { "Not archived" },
                        Status::Retrying(_) => span.pill.is-warn { "Retrying" },
                        Status::Failed(_) => span.pill.is-error { "Failed" },
                        Status::NoMedia => span.pill { "No media" },
                    }
                }
                @if let Some(error) = &e.download_error {
                    p.notice.is-error { "Last download attempt: " (error) }
                }
                @if let Some(link) = e.link.as_deref().filter(|l| l.starts_with("http")) {
                    p.small { a href=(link) rel="noreferrer noopener" { (link) } }
                }
                p.muted.small { "First seen " (format_datetime(e.first_seen_at)) " · identifier " code { (shorten(&e.guid, 80)) } }
            }

            section.panel {
                h2 { "Files" }
                @if media.is_empty() && attachments.is_empty() {
                    p.muted { "Nothing is archived for this episode yet." }
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
                                    " · downloaded " (format_datetime(m.downloaded_at))
                                }
                                div.muted.small {
                                    "BLAKE3 " code { (m.blake3) }
                                    " · last verified " (format_datetime(m.verified_at))
                                }
                            }
                            @match m.status.as_str() {
                                "ok" if Some(m.id) == newest_ok => span.pill.is-ok { "Published" },
                                "ok" => span.pill { "Earlier version" },
                                "missing" => span.pill.is-error { "Missing" },
                                _ => span.pill.is-error { "Damaged" },
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
                                "ok" => span.pill.is-ok { "Archived" },
                                "pending" if a.error.is_some() => span.pill.is-warn { "Not archived" },
                                "pending" if skipped => span.pill { "Not archived" },
                                "pending" => span.pill { "Queued" },
                                "missing" => span.pill.is-error { "Missing" },
                                _ => span.pill.is-error { "Damaged" },
                            }
                        }
                    }
                }
            }

            @if let Some(description) = e.description.as_deref().filter(|d| !d.trim().is_empty()) {
                section.panel {
                    h2 { "Show notes" }
                    p.muted.small { "As text. The published feed carries the original formatting." }
                    p.show-notes { (notes_text(description, 20_000)) }
                }
            }

            section.panel {
                h2 { "History of the feed entry" }
                @if revisions.len() <= 1 {
                    p.muted { "The publisher has not changed this entry since it was first seen." }
                } @else {
                    p.muted.small {
                        "The publisher changed this entry " (revisions.len() - 1)
                        @if revisions.len() == 2 { " time" } @else { " times" }
                        ". Every state is kept in the archive."
                    }
                }
                ol.revision-list {
                    @for (index, revision) in revisions.iter().enumerate().rev() {
                        li {
                            strong { (format_datetime(revision.seen_at)) }
                            @if revision.meta_hash == e.meta_hash { " " span.tag { "Current" } }
                            @if index == 0 {
                                span.muted.small { " · first seen" }
                            } @else {
                                @match entry_changes(&revisions[index - 1].raw_xml, &revision.raw_xml) {
                                    Some(changes) if changes.is_empty() => {
                                        p.muted.small { "Only spacing or the order of attributes changed." }
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
                                                span.muted { "… and " (changes.len() - 60) " more lines\n" }
                                            }
                                        }
                                    }
                                    None => p.muted.small { "Too long to compare here; both states are in the archive." },
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
                            a.episode-title href=(format!("/podcasts/{}/episodes/{}", podcast.id, e.id)) { (e.title) }
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
