//! Service storage benchmarks: the queries the scheduler runs on every
//! wake, the full-text search, and the settings round trip
//! (`docs/benchmarks/`). No network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use time::OffsetDateTime;
use tokio::runtime::Runtime;
use uguisu_core::ids::PodcastId;
use uguisu_core::model::PodcastStatus;
use uguisu_core::search::parse_query;
use uguisu_storage::{Storage, episodes, podcasts, search, settings};

/// A library of `podcasts_n` shows with `per_podcast` episodes each,
/// indexed by the triggers on the way in.
async fn library(podcasts_n: usize, per_podcast: usize) -> (Storage, Vec<PodcastId>) {
    let store = Storage::open_temp().await.unwrap();
    let now = OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    let mut tx = store.begin().await.unwrap();
    for n in 0..podcasts_n {
        let mut podcast = podcasts::sample(&format!("Bench Show {n}"));
        podcast.description_text = Some(format!("A benchmark show about topic {n}"));
        // Half are due, half are not: the shape a real library has.
        podcast.next_refresh_at = Some(if n % 2 == 0 {
            now - time::Duration::minutes(5)
        } else {
            now + time::Duration::hours(1)
        });
        podcast.status = if n % 17 == 0 {
            PodcastStatus::Paused
        } else {
            PodcastStatus::Active
        };
        podcasts::insert(&mut tx, &podcast).await.unwrap();
        let mut batch = Vec::with_capacity(per_podcast);
        for e in 0..per_podcast {
            let mut episode = episodes::sample(podcast.id, &format!("{n}-{e}"), &title(n, e), now);
            episode.description_text = Some(notes(n, e));
            batch.push(episode);
        }
        if !batch.is_empty() {
            episodes::upsert_all(&mut tx, &batch).await.unwrap();
        }
        ids.push(podcast.id);
    }
    tx.commit().await.unwrap();
    (store, ids)
}

fn title(podcast: usize, episode: usize) -> String {
    const WORDS: [&str; 8] = [
        "ownership",
        "borrowing",
        "async",
        "runtimes",
        "unsafe",
        "macros",
        "traits",
        "lifetimes",
    ];
    format!(
        "{} and {} in show {podcast}",
        WORDS[episode % WORDS.len()],
        WORDS[(episode / 8 + 1) % WORDS.len()]
    )
}

fn notes(podcast: usize, episode: usize) -> String {
    format!(
        "Show notes for episode {episode} of {podcast}. We talk about systems programming, \
         memory safety and the occasional detour into build times. {}",
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(20)
    )
}

fn due_queries(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("scheduler_tick");
    for size in [0_usize, 100, 1000] {
        let (store, ids) = rt.block_on(library(size, 0));
        let excluded: Vec<PodcastId> = ids.iter().take(8).copied().collect();
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("due_ids", size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let mut reader = store.reader().await.unwrap();
                    podcasts::due_ids(&mut reader, OffsetDateTime::now_utc(), &excluded, 8)
                        .await
                        .unwrap()
                })
            });
        });
        group.bench_with_input(BenchmarkId::new("next_due_at", size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let mut reader = store.reader().await.unwrap();
                    podcasts::next_due_at(&mut reader).await.unwrap()
                })
            });
        });
    }
    group.finish();
}

fn full_text_search(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("search");
    group.measurement_time(Duration::from_secs(10));
    for (podcasts_n, per) in [(20_usize, 500_usize), (100, 1000)] {
        let episodes_n = podcasts_n * per;
        let (store, _) = rt.block_on(library(podcasts_n, per));
        let expression = parse_query("async runtimes", false)
            .unwrap()
            .match_expression();
        group.throughput(Throughput::Elements(episodes_n as u64));
        group.bench_with_input(
            BenchmarkId::new("episodes", episodes_n),
            &expression,
            |b, expression| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut reader = store.reader().await.unwrap();
                        search::search_episodes(&mut reader, expression, 25)
                            .await
                            .unwrap()
                    })
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("prefix", episodes_n),
            &parse_query("async run", true).unwrap().match_expression(),
            |b, expression| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut reader = store.reader().await.unwrap();
                        search::search_episodes(&mut reader, expression, 25)
                            .await
                            .unwrap()
                    })
                });
            },
        );
        // Rebuilding is the expensive half, and the one a first start
        // pays. Measured whole, so the batch size is visible in it — and
        // only at the smaller size: a rebuild is linear in rows, so ten
        // seconds per iteration at the larger one would measure the
        // harness's patience rather than the code.
        if episodes_n > 10_000 {
            continue;
        }
        group.bench_with_input(
            BenchmarkId::new("reindex", episodes_n),
            &episodes_n,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut writer = store.writer().await.unwrap();
                        search::clear(&mut writer).await.unwrap();
                        let mut cursor = None;
                        loop {
                            let (_, last) =
                                search::index_episodes(&mut writer, cursor, search::BATCH)
                                    .await
                                    .unwrap();
                            match last {
                                Some(id) => cursor = Some(id),
                                None => break,
                            }
                        }
                    });
                });
            },
        );
    }
    group.finish();
}

fn stored_settings(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let store = rt.block_on(Storage::open_temp()).unwrap();
    let mut group = c.benchmark_group("settings");
    group.bench_function("write", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut tx = store.begin().await.unwrap();
                settings::set(
                    &mut tx,
                    "UGUISU_ARCHIVE_MAX_BACKLOG",
                    "7",
                    Some("bench"),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
            });
        });
    });
    group.bench_function("read_all", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut reader = store.reader().await.unwrap();
                settings::list(&mut reader).await.unwrap()
            })
        });
    });
    group.finish();
}

fn query_parsing(c: &mut Criterion) {
    c.bench_function("parse_query", |b| {
        b.iter(|| parse_query("async runtimes \"and\" OR everything -else", true));
    });
}

criterion_group!(
    benches,
    due_queries,
    full_text_search,
    stored_settings,
    query_parsing
);
criterion_main!(benches);
