//! How long one password verification costs, which is the durable control
//! behind a login (`docs/SECURITY.md` §3.5 asks for roughly 100 ms).
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::hint::black_box;
use std::time::Instant;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use criterion::{Criterion, criterion_group, criterion_main};

fn argon2_for(m: u32, t: u32, p: u32) -> Argon2<'static> {
    let params = Params::new(m, t, p, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn verify(c: &mut Criterion) {
    let salt = SaltString::encode_b64(b"sixteen bytes!!!").unwrap();
    let mut group = c.benchmark_group("argon2id_verify");
    group.sample_size(20);
    for (m, t, p) in [
        (19_456, 2, 1),
        (47_104, 1, 1),
        (65_536, 2, 1),
        (65_536, 3, 1),
    ] {
        let hasher = argon2_for(m, t, p);
        let hash = hasher
            .hash_password(b"correct horse", &salt)
            .unwrap()
            .to_string();
        let parsed = PasswordHash::new(&hash).unwrap();
        // One untimed run so the report can be read without running it.
        let start = Instant::now();
        hasher.verify_password(b"correct horse", &parsed).unwrap();
        println!("argon2id m={m} t={t} p={p}: {:?}", start.elapsed());
        group.bench_function(format!("m{m}_t{t}_p{p}"), |b| {
            b.iter(|| {
                let _ = black_box(hasher.verify_password(b"correct horse", &parsed));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, verify);
criterion_main!(benches);
