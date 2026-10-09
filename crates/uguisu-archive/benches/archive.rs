//! Archive engine benchmarks (`docs/benchmarks/`). Everything here is
//! either pure computation or a local filesystem operation, so the numbers
//! measure the archive logic itself — no network, no database.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use time::OffsetDateTime;
use uguisu_archive::collision::{self, Holder, Occupancy};
use uguisu_archive::path::{self, RelativePath};
use uguisu_archive::policy::{EffectivePolicy, Position};
use uguisu_archive::sanitize;
use uguisu_archive::template::{Context, Template};
use uguisu_archive::verify::{self, Expectation};
use uguisu_core::archive::{PathProfile, PolicyMode, VerifyDepth};
use uguisu_core::download::Priority;
use uguisu_core::ids::{EpisodeId, PodcastId};

const MIB: u64 = 1024 * 1024;

const DEFAULT_TEMPLATE: &str =
    "{podcast.title}/{episode.year}/{episode.date} - {episode.title}.{extension}";
const RICH_TEMPLATE: &str = "{podcast.title|slug}/[S{episode.season|pad:2}E{episode.number|pad:2} - ]{episode.title|truncate:60}.{extension}";

/// A title with the awkward parts a real feed has: Unicode, punctuation,
/// separators and length.
const TITLE: &str = "Ep. 141: The Pizza Problem — Wie »Käse/Ketchup« das Büro sprengte (Teil 2/3)";

fn context<'a>(podcast: &'a str, episode: &'a str) -> Context<'a> {
    Context::synthetic(
        podcast,
        episode,
        "mp3",
        PodcastId::new(),
        EpisodeId::new(),
        Some(OffsetDateTime::from_unix_timestamp(1_705_325_400).unwrap()),
    )
}

fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("template");
    let default = Template::parse(DEFAULT_TEMPLATE).unwrap();
    let rich = Template::parse(RICH_TEMPLATE).unwrap();
    let ctx = context("Darknet Diaries", TITLE);

    group.bench_function("parse/default", |b| {
        b.iter(|| Template::parse(black_box(DEFAULT_TEMPLATE)).unwrap());
    });
    group.bench_function("parse/filters_and_groups", |b| {
        b.iter(|| Template::parse(black_box(RICH_TEMPLATE)).unwrap());
    });
    for profile in PathProfile::ALL {
        group.bench_function(format!("render/default/{profile}"), |b| {
            b.iter(|| default.render(black_box(&ctx), profile).unwrap());
        });
    }
    group.bench_function("render/filters_and_groups", |b| {
        b.iter(|| rich.render(black_box(&ctx), PathProfile::Portable).unwrap());
    });
    group.finish();

    // What a whole library costs: 100k paths, the figure the roadmap asks
    // for. Separate group so the sample size does not distort the rest.
    let mut bulk = c.benchmark_group("template_bulk");
    bulk.throughput(Throughput::Elements(100_000));
    bulk.sample_size(10);
    bulk.bench_function("render/100k_paths", |b| {
        b.iter(|| {
            for i in 0..100_000u32 {
                let title = format!("{TITLE} #{i}");
                let ctx = context("Darknet Diaries", &title);
                black_box(default.render(&ctx, PathProfile::Portable).unwrap());
            }
        });
    });
    bulk.finish();
}

fn bench_sanitize(c: &mut Criterion) {
    let mut group = c.benchmark_group("sanitize");
    let long = "Ä".repeat(400);
    for profile in PathProfile::ALL {
        group.bench_function(format!("segment/typical/{profile}"), |b| {
            b.iter(|| sanitize::segment(black_box(TITLE), profile));
        });
    }
    group.bench_function("segment/400_chars_truncated", |b| {
        b.iter(|| sanitize::segment(black_box(&long), PathProfile::Portable));
    });
    group.bench_function("file_name/with_suffix", |b| {
        b.iter(|| {
            sanitize::file_name(
                black_box(TITLE),
                "mp3",
                Some("[a1b2c3]"),
                PathProfile::Portable,
            )
        });
    });
    group.finish();
}

/// Occupancy answered from memory, to measure the rule and not the disk.
struct Table(std::collections::HashMap<String, EpisodeId>);

impl Occupancy for Table {
    fn holder(&self, p: &RelativePath, episode: EpisodeId) -> Holder {
        match self.0.get(p.as_str()) {
            None => Holder::Free,
            Some(owner) if *owner == episode => Holder::SameEpisode,
            Some(_) => Holder::OtherEpisode,
        }
    }
}

fn bench_collision(c: &mut Criterion) {
    let mut group = c.benchmark_group("collision");
    let preferred = RelativePath::parse("Darknet Diaries/2024/2024-01-15 - Ep.mp3").unwrap();
    let episode = EpisodeId::new();
    let free = Table(std::collections::HashMap::new());
    let taken = Table(
        [(preferred.as_str().to_owned(), EpisodeId::new())]
            .into_iter()
            .collect(),
    );

    group.bench_function("free", |b| {
        b.iter(|| collision::place(black_box(&preferred), episode, PathProfile::Portable, &free));
    });
    group.bench_function("taken/suffix", |b| {
        b.iter(|| {
            collision::place(
                black_box(&preferred),
                episode,
                PathProfile::Portable,
                &taken,
            )
        });
    });
    group.bench_function("suffix_for", |b| {
        b.iter(|| collision::suffix_for(black_box(episode)));
    });
    group.finish();
}

fn bench_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("path");
    let root = Path::new("/media/podcasts");
    let relative =
        RelativePath::parse("Darknet Diaries/2024/2024-01-15 - The Pizza Problem.mp3").unwrap();
    let joined = path::resolve(root, &relative).unwrap();

    group.bench_function("parse", |b| {
        b.iter(|| RelativePath::parse(black_box("Darknet Diaries/2024/Ep.mp3")).unwrap());
    });
    group.bench_function("resolve", |b| {
        b.iter(|| path::resolve(black_box(root), &relative).unwrap());
    });
    group.bench_function("is_inside", |b| {
        b.iter(|| path::is_inside(black_box(root), &joined));
    });

    // The symlink-aware variant touches the filesystem once per existing
    // ancestor, which is what makes it worth measuring separately.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Darknet Diaries/2024")).unwrap();
    group.bench_function("resolve_checked", |b| {
        b.iter(|| path::resolve_checked(black_box(dir.path()), &relative).unwrap());
    });
    group.finish();
}

fn bench_verify(c: &mut Criterion) {
    use sha2::{Digest, Sha256};

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Show")).unwrap();
    let relative = RelativePath::parse("Show/Ep.mp3").unwrap();
    let full = dir.path().join("Show").join("Ep.mp3");

    let mut group = c.benchmark_group("verify");
    group.sample_size(10);
    // 100 MiB by default; 1 GiB only when explicitly asked for, like the
    // download engine's large-file bench.
    let mut sizes = vec![100 * MIB];
    if std::env::var("UGUISU_TEST_LARGE").is_ok() {
        sizes.push(1024 * MIB);
    }
    for size in sizes {
        let bytes: Vec<u8> = (0..size)
            .map(|i| u8::try_from(i % 251).unwrap_or(0))
            .collect();
        std::fs::write(&full, &bytes).unwrap();
        let meta = std::fs::metadata(&full).unwrap();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok());
        let expect = Expectation {
            relative_path: relative.clone(),
            size_bytes: size,
            hash_value: hex::encode(Sha256::digest(&bytes)),
            mtime_unix: mtime,
        };

        // Only the full pass reads the file, so only it has a byte
        // throughput. Declaring one for `existence` or `light` would
        // divide the file size by a time that never touched the bytes and
        // report a number in the tens of TiB/s — impressive and untrue.
        group.throughput(Throughput::Elements(1));
        for depth in [VerifyDepth::Existence, VerifyDepth::Light] {
            group.bench_function(format!("{depth}/{}MiB", size / MIB), |b| {
                b.iter(|| verify::verify(black_box(dir.path()), &expect, depth));
            });
        }
        group.throughput(Throughput::Bytes(size));
        group.bench_function(format!("full/{}MiB", size / MIB), |b| {
            b.iter(|| verify::verify(black_box(dir.path()), &expect, VerifyDepth::Full));
        });
    }
    group.finish();
}

fn bench_policy(c: &mut Criterion) {
    let podcast_id = PodcastId::new();
    let id = EpisodeId::new();
    let now = OffsetDateTime::now_utc();
    let episode = uguisu_core::model::Episode {
        id,
        podcast_id,
        guid: Some("g".into()),
        guid_is_permalink: None,
        identity: uguisu_core::model::EpisodeIdentity {
            key: "guid:g".into(),
            source: uguisu_core::model::IdentitySource::Guid,
            guid_key: Some("g".into()),
            enclosure_key: None,
            fingerprint_key: None,
            reason: "bench".into(),
        },
        title: TITLE.into(),
        subtitle: None,
        sort_title: TITLE.to_lowercase(),
        description_html: None,
        description_text: None,
        link: None,
        published_at: Some(now),
        published_at_raw: None,
        published_at_quality: uguisu_core::model::DateQuality::Exact,
        updated_at_source: None,
        duration_secs: Some(3600),
        duration_raw: None,
        season: Some(1),
        episode_number: Some(141),
        episode_type: None,
        explicit: None,
        artwork_url: None,
        author: None,
        content_hash: "c".into(),
        archive_state: uguisu_core::model::ArchiveState::Expected,
        skip_reason: None,
        malformed: false,
        malformed_reason: None,
        duplicate_of_episode_id: None,
        duplicate_reasons: Vec::new(),
        missing_streak: 0,
        first_seen_at: now,
        last_seen_in_feed_at: now,
        removed_from_feed_at: None,
        sort_at: now,
        source_metadata: None,
        enclosures: vec![uguisu_core::model::Enclosure {
            id: uguisu_core::ids::EnclosureId::new(),
            episode_id: id,
            url: url::Url::parse("https://cdn.test/a.mp3").unwrap(),
            mime_type: Some("audio/mpeg".into()),
            length_bytes: Some(1024),
            is_primary: true,
            kind: uguisu_core::model::EnclosureKind::Audio,
            position: 0,
            bitrate: None,
            height: None,
            codecs: None,
            lang: None,
            title: None,
            integrity_type: None,
            integrity_value: None,
            sources: Vec::new(),
        }],
        extras: uguisu_core::model::EpisodeExtras::default(),
        created_at: now,
        updated_at: now,
    };
    let auto = EffectivePolicy {
        mode: PolicyMode::Auto,
        max_backlog: 3,
        max_age_days: 90,
        priority: Priority::Normal,
    };
    let manual = EffectivePolicy {
        mode: PolicyMode::Manual,
        ..auto
    };

    let mut group = c.benchmark_group("policy");
    group.bench_function("decide/queue", |b| {
        b.iter(|| {
            uguisu_archive::policy::decide(black_box(&auto), &episode, Position::default(), now)
        });
    });
    group.bench_function("decide/disabled", |b| {
        b.iter(|| {
            uguisu_archive::policy::decide(black_box(&manual), &episode, Position::default(), now)
        });
    });
    group.finish();
}

// Sidecars, manifests, scanning, matching and images.

fn sample_sidecar() -> uguisu_core::archive::Sidecar {
    use uguisu_core::archive::{
        ArchiveOrigin, Sidecar, SidecarArchive, SidecarEpisode, SidecarPodcast, SidecarSource,
        TagState,
    };
    let at = OffsetDateTime::from_unix_timestamp(1_705_325_400).unwrap();
    Sidecar {
        schema: Sidecar::SCHEMA,
        generator: "uguisu/0.1.0".to_owned(),
        written_at: at,
        podcast: SidecarPodcast {
            id: PodcastId::new(),
            title: "Die Pizza-Akte".to_owned(),
            author: Some("Die Autorin".to_owned()),
            publisher: None,
            feed_url: Some("https://feeds.example/pizza.xml".parse().unwrap()),
            language: Some("de".to_owned()),
            categories: vec!["Technology".to_owned(), "Society".to_owned()],
        },
        episode: SidecarEpisode {
            id: EpisodeId::new(),
            identity_key: "guid:pizza-141".to_owned(),
            identity_source: Some("guid".to_owned()),
            title: TITLE.to_owned(),
            published_at: Some(at),
            season: Some(2),
            number: Some(141),
            duration_secs: Some(3_600),
            description_text: Some("x".repeat(2_000)),
            guid: Some("pizza-141".to_owned()),
            link: None,
            enclosure_url: Some("https://cdn.example/141.mp3".parse().unwrap()),
            enclosure_type: Some("audio/mpeg".to_owned()),
            enclosure_length_bytes: Some(48_000_000),
            artwork_url: None,
            chapters: Vec::new(),
            transcripts: Vec::new(),
        },
        archive: SidecarArchive {
            relative_path: "Die Pizza-Akte/2024/2024-01-15 - Ep. 141.mp3".to_owned(),
            size_bytes: 48_000_000,
            hash_algo: "sha256".to_owned(),
            hash_value: "ab".repeat(32),
            content_type: Some("audio/mpeg".to_owned()),
            sniffed_type: Some("mp3".to_owned()),
            origin: ArchiveOrigin::Download,
            tag_state: TagState::Untagged,
            tag_mode: None,
            tagged_at: None,
            registered_at: at,
            original_tags: None,
        },
        source: Some(SidecarSource {
            hash_algo: "sha256".to_owned(),
            hash_value: "ab".repeat(32),
            size_bytes: 48_000_000,
            origin_detail: Some("https://cdn.example/141.mp3".to_owned()),
        }),
    }
}

/// Rendering and reading one sidecar. Both run once per archived file, so
/// the number that matters is per document, not per byte.
fn bench_sidecar(c: &mut Criterion) {
    use uguisu_archive::sidecar;

    let document = sample_sidecar();
    let text = sidecar::render(&document);
    let mut group = c.benchmark_group("sidecar");
    group.throughput(Throughput::Elements(1));
    group.bench_function("render", |b| {
        b.iter(|| sidecar::render(black_box(&document)));
    });
    group.bench_function("parse", |b| {
        b.iter(|| sidecar::parse(black_box(text.as_bytes())).unwrap());
    });
    group.finish();
}

fn manifest_entries(n: usize) -> Vec<uguisu_core::archive::ManifestEntry> {
    (0..n)
        .map(|i| uguisu_core::archive::ManifestEntry {
            relative_path: format!("Die Pizza-Akte/2024/2024-01-15 - Folge {i:05}.mp3"),
            hash_value: format!("{i:064x}"),
        })
        .collect()
}

/// Manifest rendering and reading at the sizes a real library reaches.
/// One entry per archived episode, so 10 000 is a large podcast.
fn bench_manifest(c: &mut Criterion) {
    use uguisu_archive::manifest;

    let podcast = PodcastId::new();
    let at = OffsetDateTime::from_unix_timestamp(1_705_325_400).unwrap();
    let mut group = c.benchmark_group("manifest");
    for size in [1_000usize, 10_000] {
        let entries = manifest_entries(size);
        let text = manifest::render(podcast, at, entries.clone());
        group.throughput(Throughput::Elements(size as u64));
        group.bench_function(format!("render/{size}"), |b| {
            b.iter(|| manifest::render(black_box(podcast), at, entries.clone()));
        });
        group.bench_function(format!("parse/{size}"), |b| {
            b.iter(|| manifest::parse(black_box(&text)).unwrap());
        });
        let found: std::collections::BTreeMap<String, Option<String>> = entries
            .iter()
            .map(|e| (e.relative_path.clone(), Some(e.hash_value.clone())))
            .collect();
        group.bench_function(format!("compare/{size}"), |b| {
            b.iter(|| manifest::compare(black_box(&entries), black_box(&found)));
        });
    }
    group.finish();
}

/// Scanning a tree. The point of the number is that it is linear in the
/// number of files and flat in memory, which is what makes a rebuild or
/// an import over a large archive possible at all.
fn bench_scan(c: &mut Criterion) {
    use uguisu_archive::scan::{self, ScanOptions};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for show in 0..10 {
        let folder = root.join(format!("Show {show}"));
        std::fs::create_dir_all(&folder).unwrap();
        for episode in 0..100 {
            std::fs::write(
                folder.join(format!("2024-01-15 - Folge {episode:03}.mp3")),
                b"ID3............",
            )
            .unwrap();
        }
    }
    let mut group = c.benchmark_group("scan");
    group.throughput(Throughput::Elements(1_000));
    let with_head = ScanOptions::default();
    group.bench_function("walk/1000/with-head", |b| {
        b.iter(|| {
            scan::walk(black_box(root), &with_head)
                .filter_map(Result::ok)
                .count()
        });
    });
    let sizes_only = ScanOptions {
        read_head: false,
        ..ScanOptions::default()
    };
    group.bench_function("walk/1000/sizes-only", |b| {
        b.iter(|| {
            scan::walk(black_box(root), &sizes_only)
                .filter_map(Result::ok)
                .count()
        });
    });
    group.finish();
}

/// Matching one file against a podcast's episodes. An import scores every
/// candidate against every episode of one show, so this is the inner loop
/// of the whole operation.
fn bench_matching(c: &mut Criterion) {
    use time::{Date, Month};
    use uguisu_archive::import::{Candidate, EpisodeFacts, classify};

    let episodes: Vec<EpisodeFacts> = (0..1_000u32)
        .map(|i| EpisodeFacts {
            id: EpisodeId::new(),
            title: format!("Ep. {i}: The Pizza Problem — Teil {}", i % 7),
            published: Date::from_calendar_date(2024, Month::January, 1 + (i % 28) as u8).ok(),
            season: Some(1 + i / 100),
            number: Some(i),
            duration_secs: Some(3_600),
            enclosure_bytes: Some(48_000_000),
        })
        .collect();
    let candidate = Candidate {
        title: Some("Ep. 500: The Pizza Problem - Teil 3".to_owned()),
        published: Date::from_calendar_date(2024, Month::January, 9).ok(),
        season: Some(6),
        number: Some(500),
        duration_secs: Some(3_600),
        size_bytes: 48_000_000,
        ..Candidate::empty()
    };
    let mut group = c.benchmark_group("import");
    group.throughput(Throughput::Elements(episodes.len() as u64));
    group.bench_function("classify/1000-episodes", |b| {
        b.iter(|| classify(black_box(&candidate), black_box(&episodes), 85));
    });
    group.finish();
}

/// Recognising artwork. It runs once per fetch over a few hundred
/// kilobytes, and only ever looks at the first twelve bytes.
fn bench_image(c: &mut Criterion) {
    use uguisu_archive::image;

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend(std::iter::repeat_n(0u8, 512 * 1024));
    let mut group = c.benchmark_group("artwork");
    group.throughput(Throughput::Elements(1));
    group.bench_function("validate/512KiB", |b| {
        b.iter(|| image::validate(black_box(Some("image/png")), black_box(&png)).unwrap());
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_render,
    bench_sanitize,
    bench_collision,
    bench_path,
    bench_verify,
    bench_policy,
    bench_sidecar,
    bench_manifest,
    bench_scan,
    bench_matching,
    bench_image
);
criterion_main!(benches);
