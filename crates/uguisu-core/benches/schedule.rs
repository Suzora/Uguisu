//! The scheduling arithmetic and the query builder: pure functions the
//! scheduler and every search call run (`docs/benchmarks/`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use time::OffsetDateTime;
use uguisu_core::PodcastId;
use uguisu_core::schedule::{plan_next, slot_permille};
use uguisu_core::search::parse_query;

fn scheduling(c: &mut Criterion) {
    let ids: Vec<PodcastId> = (0..1000).map(|_| PodcastId::new()).collect();
    let now = OffsetDateTime::now_utc();
    let hour = Duration::from_secs(3600);
    let mut group = c.benchmark_group("schedule");
    group.bench_function("slot_permille", |b| {
        let mut i = 0;
        b.iter(|| {
            i = (i + 1) % ids.len();
            slot_permille(ids[i])
        });
    });
    group.bench_function("plan_next", |b| {
        let mut i = 0;
        b.iter(|| {
            i = (i + 1) % ids.len();
            plan_next(ids[i], Some(now), now, hour)
        });
    });
    // What a restart after downtime runs, once per drained podcast.
    group.bench_function("plan_next_catch_up", |b| {
        let mut i = 0;
        b.iter(|| {
            i = (i + 1) % ids.len();
            plan_next(ids[i], None, now, hour)
        });
    });
    group.finish();
}

fn query_building(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_query");
    group.bench_function("short", |b| {
        b.iter(|| parse_query("async runtimes", false).map(|q| q.match_expression()));
    });
    // Sixteen terms is the cap, so this is the worst case a user can ask
    // for — including one that is all operators.
    group.bench_function("long", |b| {
        b.iter(|| {
            parse_query(
                "one two three four five six seven eight nine ten eleven twelve \
                 thirteen fourteen fifteen sixteen seventeen",
                true,
            )
            .map(|q| q.match_expression())
        });
    });
    group.bench_function("hostile", |b| {
        b.iter(|| {
            parse_query(
                "\"unterminated OR NEAR(x y) col:value ^anchor -negated ****",
                true,
            )
            .map(|q| q.match_expression())
        });
    });
    group.finish();
}

criterion_group!(benches, scheduling, query_building);
criterion_main!(benches);
