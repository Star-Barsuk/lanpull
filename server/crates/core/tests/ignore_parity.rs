//! The server's ignore decisions must match the shared cases that the client
//! suite also checks (`fixtures/ignore_cases.tsv`).
//!
//! Tests may unwrap and use bare asserts for brevity; production code may not.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_assert_message,
    clippy::indexing_slicing
)]

use lanpull_core::ignore::IgnoreRules;

const CASES: &str = include_str!("fixtures/ignore_cases.tsv");

#[test]
fn shared_ignore_cases_match_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let (rules, warnings) = IgnoreRules::load(dir.path());
    assert!(
        warnings.is_empty(),
        "defaults produced warnings: {warnings:?}"
    );

    let mut checked = 0_usize;
    for line in CASES.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path, expected) = line.split_once('\t').unwrap();
        let want_ignored = match expected {
            "ignored" => true,
            "kept" => false,
            other => panic!("bad expectation {other:?} for {path:?}"),
        };
        assert_eq!(rules.is_ignored(path), want_ignored, "{path}");
        checked += 1;
    }
    assert!(checked > 40, "fixture shrank unexpectedly: {checked} cases");
}
