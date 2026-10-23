//! Benchmark manifest filtering, the per-account view served to a client.
//!
//! Benches may unwrap; production code may not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_assert_message
)]
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use lanpull_core::access::{Access, Rule};
use lanpull_store::manifest::{filter, Entry, Manifest};

fn filter_entries(c: &mut Criterion) {
    let files: Vec<Entry> = (0..2000)
        .map(|index| Entry {
            path: format!("dir{}/file{index}.pdf", index % 20),
            size: 12_345,
            mtime: 1_700_000_000,
            sha256: "0".repeat(64),
        })
        .collect();
    let manifest = Manifest {
        scheme: "whole-file-v1".to_string(),
        generated_at: "2026-10-24T00:00:00Z".to_string(),
        files,
    };
    let mut access = Access::new();
    access.add_rule("alpha", Rule::parse("reports:**/*.pdf").unwrap());

    c.bench_function("filter_2000_entries", |b| {
        b.iter(|| black_box(filter(black_box(&manifest), &access, "alpha", "reports")));
    });
}

criterion_group!(benches, filter_entries);
criterion_main!(benches);
