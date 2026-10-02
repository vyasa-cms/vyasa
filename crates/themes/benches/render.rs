//! Micro benchmarks for the hot rendering paths.
//!
//! These are the functions on the critical path of every uncached public
//! page view: block rendering, token compilation and sanitization. They are
//! measured separately from the end-to-end load tests in `perf/` because a
//! regression here is a code change, whereas a regression there could be
//! the database, the cache or the machine.
//!
//! Run with `cargo bench -p vyasa-themes`.

#![allow(clippy::unwrap_used, missing_docs)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::hint::black_box;
use vyasa_core::block::types::{Block, BlockKind};
use vyasa_themes::{render_blocks, sanitize_comment_html, tokens_to_css, TokenSet};

/// A document of `n` paragraphs, matching what the seeder produces.
fn document(n: usize) -> Vec<Block> {
    (0..n)
        .map(|i| Block {
            kind: BlockKind::Paragraph,
            plugin_kind: None,
            attrs: serde_json::json!({
                "text": format!("Paragraph {i} with enough words to be worth measuring rather than \
                                 a single token that fits in a cache line."),
            }),
            children: Vec::new(),
        })
        .collect()
}

fn bench_render_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_blocks");
    // Three sizes: a short post, a typical one, and a long-form article.
    for size in [3usize, 20, 100] {
        let blocks = document(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &blocks, |b, blocks| {
            b.iter(|| render_blocks(black_box(blocks)).unwrap());
        });
    }
    group.finish();
}

fn bench_tokens_to_css(c: &mut Criterion) {
    let tokens = TokenSet::default();
    c.bench_function("tokens_to_css", |b| {
        b.iter(|| tokens_to_css(black_box(&tokens)));
    });
}

fn bench_sanitize(c: &mut Criterion) {
    // Deliberately hostile input: sanitization cost is what an attacker
    // controls, so the benchmark should reflect the worst realistic case.
    let html = "<p>Hello <strong>world</strong> \
                <script>alert(1)</script><a href=\"javascript:alert(1)\">x</a> \
                <img src=x onerror=alert(1)></p>"
        .repeat(20);
    c.bench_function("sanitize_comment_html", |b| {
        b.iter(|| sanitize_comment_html(black_box(&html)));
    });
}

fn bench_block_serialization(c: &mut Criterion) {
    let doc = vyasa_core::block::doc::BlockDocument::new(document(20));
    let json = serde_json::to_value(&doc).unwrap();
    c.bench_function("block_document_to_json", |b| {
        b.iter(|| serde_json::to_value(black_box(&doc)).unwrap());
    });
    c.bench_function("block_document_from_json", |b| {
        b.iter(|| {
            vyasa_core::block::doc::BlockDocument::from_json(black_box(json.clone())).unwrap()
        });
    });
}

criterion_group!(
    benches,
    bench_render_blocks,
    bench_tokens_to_css,
    bench_sanitize,
    bench_block_serialization,
);
criterion_main!(benches);
