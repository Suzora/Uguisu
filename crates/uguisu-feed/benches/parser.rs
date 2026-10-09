//! Parser, normalizer and identity benchmarks (`docs/benchmarks/`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use time::OffsetDateTime;
use uguisu_core::config::FeedLimits;
use uguisu_core::ids::EpisodeId;
use uguisu_feed::identity::{comparable_hash, resolve_identities, signals};
use uguisu_feed::normalize::{normalize_channel, normalize_item};
use uguisu_feed::parse;

/// A realistic RSS feed with `count` items (GUIDs, dates, HTML notes,
/// iTunes and Podcasting 2.0 tags, enclosures).
fn feed(count: usize) -> Vec<u8> {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\" xmlns:podcast=\"https://podcastindex.org/namespace/1.0\" xmlns:content=\"http://purl.org/rss/1.0/modules/content/\">\n<channel>\n<title>Bench Show</title>\n<link>https://bench.example/</link>\n<description>Generated</description>\n<itunes:author>Bench</itunes:author>\n<itunes:category text=\"Technology\"/>\n",
    );
    for i in 0..count {
        let day = 1 + (i % 28);
        let month = 1 + (i / 28) % 12;
        let year = 2018 + (i / 336) % 8;
        let _ = writeln!(
            s,
            "<item><title>Episode {i}: Ünïcode &amp; entities</title><guid isPermaLink=\"false\">ep-{i}</guid>\
             <pubDate>{day:02} {mon} {year} 10:00:00 +0000</pubDate>\
             <description><![CDATA[<p>Show notes for <b>{i}</b>, see <a href=\"https://bench.example/{i}\">the page</a>.</p><ul><li>one</li><li>two</li></ul>]]></description>\
             <content:encoded><![CDATA[<p>Longer notes for episode {i} repeated a few times to look like real feeds. Longer notes for episode {i}.</p>]]></content:encoded>\
             <itunes:duration>{}:{:02}:00</itunes:duration><itunes:episode>{}</itunes:episode><itunes:explicit>no</itunes:explicit>\
             <podcast:transcript url=\"https://bench.example/{i}.vtt\" type=\"text/vtt\"/>\
             <podcast:person role=\"host\">Host {}</podcast:person>\
             <enclosure url=\"https://cdn.bench.example/media/{i}.mp3?utm_source=rss\" type=\"audio/mpeg\" length=\"{}\"/>\
             </item>",
            (i % 3) + 1,
            i % 60,
            i + 1,
            i % 4,
            10_000_000 + i * 1000,
            mon = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
            ][month - 1],
        );
    }
    s.push_str("</channel>\n</rss>\n");
    s.into_bytes()
}

fn bench_parse(c: &mut Criterion) {
    let limits = FeedLimits::default();
    let mut g = c.benchmark_group("parse");
    for &n in &[10usize, 200, 1_000, 10_000] {
        let body = feed(n);
        g.throughput(Throughput::Bytes(body.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &body, |b, body| {
            b.iter(|| parse(black_box(body), &limits).unwrap());
        });
    }
    g.finish();
}

fn bench_normalize(c: &mut Criterion) {
    let limits = FeedLimits::default();
    let now = OffsetDateTime::now_utc();
    let mut g = c.benchmark_group("normalize");
    for &n in &[10usize, 200, 1_000, 10_000] {
        let parsed = parse(&feed(n), &limits).unwrap();
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &parsed, |b, parsed| {
            b.iter(|| {
                let ch = normalize_channel(&parsed.channel);
                let items: Vec<_> = parsed
                    .items
                    .iter()
                    .map(|i| normalize_item(i, EpisodeId::new(), now))
                    .collect();
                black_box((ch, items))
            });
        });
    }
    g.finish();
}

fn bench_identity(c: &mut Criterion) {
    let limits = FeedLimits::default();
    let now = OffsetDateTime::now_utc();
    let mut g = c.benchmark_group("identity");
    for &n in &[10usize, 200, 1_000, 10_000] {
        let parsed = parse(&feed(n), &limits).unwrap();
        let normalized: Vec<_> = parsed
            .items
            .iter()
            .map(|i| normalize_item(i, EpisodeId::new(), now))
            .collect();
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(
            BenchmarkId::from_parameter(n),
            &(parsed, normalized),
            |b, (parsed, normalized)| {
                b.iter(|| {
                    let sigs: Vec<_> = parsed
                        .items
                        .iter()
                        .zip(normalized)
                        .map(|(p, n)| signals(p, n))
                        .collect();
                    let ids = resolve_identities(&sigs);
                    let hashes: Vec<String> = normalized.iter().map(comparable_hash).collect();
                    black_box((ids, hashes))
                });
            },
        );
    }
    g.finish();
}

criterion_group!(benches, bench_parse, bench_normalize, bench_identity);
criterion_main!(benches);
