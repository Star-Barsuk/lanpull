//! Benchmark the parsing and matching hot paths.
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

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use lanpull_core::access::{Access, Rule};
use lanpull_core::config::parse_kv;
use lanpull_core::glob::Glob;

fn glob_match(c: &mut Criterion) {
    let globs: Vec<Glob> = ["**/*.pdf", "media/**", "*.txt", "a/*/b"]
        .iter()
        .map(|pattern| Glob::parse(pattern).unwrap())
        .collect();
    let paths = ["a/b/c.pdf", "media/x/y.mp3", "notes.txt", "a/x/b"];
    c.bench_function("glob_match_4x4", |b| {
        b.iter(|| {
            for glob in &globs {
                for path in paths {
                    black_box(glob.covers(black_box(path)));
                }
            }
        });
    });
}

fn glob_parse(c: &mut Criterion) {
    c.bench_function("glob_parse", |b| {
        b.iter(|| black_box(Glob::parse(black_box("media/**/2026/*.pdf"))));
    });
}

fn access_allows(c: &mut Criterion) {
    let mut access = Access::new();
    access.add_rule("alpha", Rule::parse("reports:**/*.pdf").unwrap());
    access.add_rule("alpha", Rule::parse("media:music/**").unwrap());
    c.bench_function("access_allows", |b| {
        b.iter(|| black_box(access.allows("alpha", "reports", "sub/deck.pdf")));
    });
}

fn config_parse(c: &mut Criterion) {
    let mut text = String::new();
    for index in 0..50 {
        let _ = writeln!(text, "SHARE_s{index}=/srv/s{index}");
    }
    text.push_str("SERVER_IP=192.168.0.1\n");
    c.bench_function("config_parse_50_shares", |b| {
        b.iter(|| black_box(parse_kv(black_box(&text))));
    });
}

criterion_group!(benches, glob_match, glob_parse, access_allows, config_parse);
criterion_main!(benches);
