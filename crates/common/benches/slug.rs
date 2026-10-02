//! Slug generation benchmark.
//!
//! Slugify runs on every post save and on every import row, so its cost is
//! paid per write rather than per read — which is why it is measured apart
//! from the render path.
//!
//! Run with `cargo bench -p vyasa-common`.

#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use vyasa_common::slugify;

fn bench_slugify(c: &mut Criterion) {
    let plain = "A Simple Post Title";
    // Accents and punctuation take the transliteration path, which is the
    // expensive one and the one an importer hits constantly.
    let awkward = "A Post Title With Punctuation, Accents (café, naïve) & Numbers — 2026";
    let long = awkward.repeat(10);

    c.bench_function("slugify/plain", |b| {
        b.iter(|| slugify(black_box(plain)));
    });
    c.bench_function("slugify/accented", |b| {
        b.iter(|| slugify(black_box(awkward)));
    });
    c.bench_function("slugify/long", |b| {
        b.iter(|| slugify(black_box(&long)));
    });
}

criterion_group!(benches, bench_slugify);
criterion_main!(benches);
