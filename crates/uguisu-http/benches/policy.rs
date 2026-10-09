//! Benchmarks for the network policy (SSRF classification and URL checks).
#![allow(clippy::expect_used)]

use std::hint::black_box;
use std::net::IpAddr;

use criterion::{Criterion, criterion_group, criterion_main};
use uguisu_http::{NetworkPolicy, Url, classify_ip};

fn bench_classify(c: &mut Criterion) {
    let addrs: Vec<IpAddr> = [
        "8.8.8.8",
        "10.0.0.1",
        "169.254.169.254",
        "::ffff:127.0.0.1",
        "2606:4700::1111",
        "fe80::1",
        "2002:0a00:0001::1",
    ]
    .iter()
    .map(|s| s.parse().expect("ip"))
    .collect();
    c.bench_function("classify_ip x7", |b| {
        b.iter(|| {
            for ip in &addrs {
                black_box(classify_ip(*ip));
            }
        });
    });
}

fn bench_check_url(c: &mut Criterion) {
    let policy = NetworkPolicy::strict().allow_private_hosts(["nas.local"]);
    let urls: Vec<Url> = [
        "https://feeds.example.com/podcast.rss",
        "http://example.com:8080/feed",
        "http://127.0.0.1/",
        "http://nas.local/feed",
        "ftp://example.com/",
    ]
    .iter()
    .map(|s| Url::parse(s).expect("url"))
    .collect();
    c.bench_function("check_url x5", |b| {
        b.iter(|| {
            for u in &urls {
                black_box(policy.check_url(u).is_ok());
            }
        });
    });
}

criterion_group!(benches, bench_classify, bench_check_url);
criterion_main!(benches);
