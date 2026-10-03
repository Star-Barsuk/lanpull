//! Unit and property tests for the lanpull library.
//!
//! Tests may unwrap, index, and use bare asserts for brevity; production code
//! may not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::missing_assert_message
)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use lanpull::access::{Access, Rule};
use lanpull::{arm::ArmState, audit, bundle, clients, config, manifest, status, timeutil};

#[test]
fn ignore_patterns_match() {
    use lanpull::ignore::is_ignored;
    for ignored in ["~lock.a", ".~lock.b", "a.tmp", "dir/a.part"] {
        assert!(is_ignored(ignored), "{ignored} should be ignored");
    }
    for kept in ["a.txt", "part", "archive.tar.gz"] {
        assert!(!is_ignored(kept), "{kept} should not be ignored");
    }
}

#[test]
fn duration_parsing() {
    assert_eq!(timeutil::parse_duration_secs("15m").unwrap(), 900);
    assert_eq!(timeutil::parse_duration_secs("1h").unwrap(), 3600);
    assert!(timeutil::parse_duration_secs("nope").is_err());
}

#[test]
fn iso8601_roundtrip() {
    let text = timeutil::iso8601(1_700_000_000);
    assert_eq!(timeutil::parse_iso8601(&text).unwrap(), 1_700_000_000);
}

#[test]
fn config_parses_quotes_and_defaults() {
    let text = "# comment\nSHARE_reports=\"/srv/share\"\nSERVER_IP=10.0.0.1\nPORT=9000\n";
    let map = config::parse_kv(text);
    let cfg = config::Config::from_map(&map).unwrap();
    assert_eq!(cfg.shares.get("reports").unwrap(), Path::new("/srv/share"));
    assert_eq!(cfg.port, 9000);
    assert_eq!(cfg.server_ip.to_string(), "10.0.0.1");
    assert_eq!(cfg.cert_path, Path::new("/var/lib/lanpull/server.crt"));
}

#[test]
fn config_expands_missing_env_verbatim() {
    let map = config::parse_kv("SHARE_reports=/srv/${LANPULL_NO_SUCH_VAR}\n");
    assert_eq!(
        map.get("SHARE_reports").unwrap(),
        "/srv/${LANPULL_NO_SUCH_VAR}"
    );
}

#[test]
fn config_requires_share_and_server_ip() {
    let mut map = BTreeMap::new();
    map.insert("SERVER_IP".to_string(), "10.0.0.1".to_string());
    assert!(config::Config::from_map(&map).is_err());
}

#[test]
fn password_hash_roundtrip() {
    let password = clients::generate_password();
    assert_eq!(password.len(), 24);
    let hash = clients::hash_password(&password).unwrap();
    assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(clients::verify_password(&hash, &password));
    assert!(!clients::verify_password(&hash, "wrong"));
}

#[test]
fn clients_roundtrip_with_optional_ip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lanpull.clients");

    let mut accounts = clients::Clients::new();
    accounts.insert(clients::Account {
        name: "alpha".to_string(),
        hash: clients::hash_password("secret").unwrap(),
        allowed_ip: Some("10.0.0.5".parse().unwrap()),
        local: false,
    });
    accounts.insert(clients::Account {
        name: "beta".to_string(),
        hash: clients::hash_password("secret").unwrap(),
        allowed_ip: None,
        local: true,
    });
    accounts.save(&path).unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let loaded = clients::Clients::load(&path).unwrap();
    assert_eq!(loaded.iter().count(), 2);
    assert_eq!(
        loaded.get("alpha").unwrap().allowed_ip.unwrap().to_string(),
        "10.0.0.5"
    );
    assert!(!loaded.get("alpha").unwrap().local);
    assert!(loaded.get("beta").unwrap().allowed_ip.is_none());
    assert!(loaded.get("beta").unwrap().local);
}

#[test]
fn clients_parse_ignores_malformed_lines() {
    let text = "# c\nalpha:HASH:10.0.0.5\nbroken\nbeta:HASH\n";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clients");
    fs::write(&path, text).unwrap();
    let loaded = clients::Clients::load(&path).unwrap();
    assert_eq!(loaded.iter().count(), 2);
}

#[test]
fn access_rules_scope_paths() {
    let mut access = Access::new();
    access.add_rule("laptop", Rule::parse("reports").unwrap());
    access.add_rule("laptop", Rule::parse("media:music/**").unwrap());
    access.add_rule("desktop", Rule::parse("*").unwrap());

    assert!(access.allows("laptop", "reports", "a/b.pdf"));
    assert!(access.allows("laptop", "media", "music/2026/track.flac"));
    assert!(!access.allows("laptop", "media", "video/clip.mp4"));
    assert!(access.allows("desktop", "media", "anything"));
    assert!(!access.allows("unknown", "reports", "a"));
    assert_eq!(access.rules("laptop").len(), 2);
}

#[test]
fn manifest_filter_scopes_entries() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    fs::create_dir_all(share.join("music")).unwrap();
    fs::create_dir_all(share.join("video")).unwrap();
    fs::write(share.join("music/song.mp3"), b"a").unwrap();
    fs::write(share.join("video/clip.mp4"), b"b").unwrap();
    fs::create_dir_all(dir.path().join("state")).unwrap();

    let (manifest, _) = manifest::generate(
        &share,
        &dir.path().join("state/manifest/media.json"),
        &dir.path().join("state/manifest/media.cache.json"),
    )
    .unwrap();

    let mut access = Access::new();
    access.add_rule("laptop", Rule::parse("media:music/**").unwrap());
    let filtered = manifest::filter(&manifest, &access, "laptop", "media");
    let paths: Vec<&str> = filtered.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["music/song.mp3"]);
}

#[test]
fn arm_window_expires() {
    let mut state = ArmState::default();
    state.arm("alpha", 1000);
    assert!(state.is_armed("alpha", 999));
    assert!(!state.is_armed("alpha", 1000));
    assert_eq!(state.armed_entries(940), vec![("alpha".to_string(), 60)]);
    assert!(state.armed_entries(1000).is_empty());
}

#[test]
fn manifest_skips_ignored_reserved_and_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    fs::create_dir_all(&share).unwrap();
    fs::write(share.join("a.txt"), b"hello").unwrap();
    fs::create_dir_all(share.join("sub")).unwrap();
    fs::write(share.join("sub/b.bin"), b"data").unwrap();
    fs::write(share.join("skip.tmp"), b"x").unwrap();
    fs::create_dir_all(share.join("_lanpull")).unwrap();
    fs::write(share.join("_lanpull/secret"), b"x").unwrap();
    std::os::unix::fs::symlink(share.join("a.txt"), share.join("link")).unwrap();

    let state = dir.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let manifest_path = state.join("manifest.json");
    let cache_path = state.join("manifest.cache.json");

    let (manifest, warnings) = manifest::generate(&share, &manifest_path, &cache_path).unwrap();

    let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt", "sub/b.bin"]);
    assert_eq!(manifest.scheme, manifest::SCHEME);
    assert!(warnings.reserved.iter().any(|p| p == "_lanpull/secret"));
    assert!(warnings.symlinks.iter().any(|p| p == "link"));
    assert!(manifest_path.is_file());
    assert!(cache_path.is_file());
    assert!(!state.join("manifest.json.tmp").exists());
}

#[test]
fn manifest_cache_reuses_digest() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    fs::create_dir_all(&share).unwrap();
    fs::write(share.join("a.txt"), b"hello").unwrap();

    let state = dir.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let manifest_path = state.join("manifest.json");
    let cache_path = state.join("manifest.cache.json");

    let (first, _) = manifest::generate(&share, &manifest_path, &cache_path).unwrap();
    let (second, _) = manifest::generate(&share, &manifest_path, &cache_path).unwrap();
    assert_eq!(first.files[0].sha256, second.files[0].sha256);

    let cache = lanpull::cache::Cache::load(&cache_path).unwrap();
    assert_eq!(
        cache.entries.get("a.txt").unwrap().sha256,
        first.files[0].sha256
    );
}

#[test]
fn bundle_builds_and_finds_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), b"1.2.3\n").unwrap();
    fs::write(dir.path().join("pull.py"), b"print('hi')\n").unwrap();

    let manifest = bundle::build(dir.path()).unwrap();
    assert_eq!(manifest.version, "1.2.3");
    assert!(bundle::contains(&manifest, "pull.py"));
    assert!(!bundle::contains(&manifest, "auth"));
    let pull = manifest.files.iter().find(|f| f.path == "pull.py").unwrap();
    assert_eq!(pull.size, 12);
}

#[test]
fn example_config_declares_a_named_share() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/lanpull.conf.example");
    let text = fs::read_to_string(&path).unwrap();
    let map = config::parse_kv(&text);
    assert!(
        map.keys().any(|key| key.starts_with("SHARE_")),
        "the example must declare at least one SHARE_<name>"
    );
    assert!(
        !map.contains_key("SHARE_DIR"),
        "the removed SHARE_DIR key must not appear"
    );
}

#[test]
fn access_warnings_report_unknown_share() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    fs::create_dir_all(&share).unwrap();
    let conf = dir.path().join("lanpull.conf");
    fs::write(
        &conf,
        format!(
            "SHARE_reports={}\nSTATE_DIR={}\nSERVER_IP=127.0.0.1\n",
            share.display(),
            dir.path().join("state").display()
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("lanpull.access.json"),
        br#"{"version":1,"shares":{"ghost":{"public":["**"]}}}"#,
    )
    .unwrap();
    let config = config::Config::load(&conf).unwrap();
    let warnings = status::access_warnings(&config);
    assert!(
        warnings.iter().any(|w| w.contains("unknown share ghost")),
        "expected a warning about the unknown share, got {warnings:?}"
    );
}

#[test]
fn bundle_missing_version_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("pull.py"), b"x").unwrap();
    assert!(bundle::build(dir.path()).is_err());
}

#[test]
fn audit_report_aggregates() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("access.log");
    let now = timeutil::iso8601(timeutil::now_unix());

    audit::append(
        &log,
        &audit::Record {
            ts: now.clone(),
            user: "alpha".to_string(),
            ip: "10.0.0.5".to_string(),
            host: "client-a".to_string(),
            method: "GET".to_string(),
            path: "/_lanpull/share/reports/file/a.txt".to_string(),
            status: 200,
            bytes: 100,
            reason: None,
        },
    )
    .unwrap();
    audit::append(
        &log,
        &audit::Record {
            ts: now,
            user: "alpha".to_string(),
            ip: "10.0.0.5".to_string(),
            host: "client-a".to_string(),
            method: "GET".to_string(),
            path: "/_lanpull/share/reports/file/a.txt".to_string(),
            status: 401,
            bytes: 0,
            reason: Some("not_armed".to_string()),
        },
    )
    .unwrap();

    let report = audit::summarize(&log, None, None).unwrap();
    let alpha = report.accounts.iter().find(|a| a.user == "alpha").unwrap();
    assert_eq!(alpha.files, 1);
    assert_eq!(alpha.bytes, 100);
    assert_eq!(alpha.rejected, 1);
    assert_eq!(alpha.last_host, "client-a");
}

#[test]
fn audit_report_missing_log_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let report = audit::summarize(&dir.path().join("nope.log"), None, None).unwrap();
    assert!(report.accounts.is_empty());
}

#[test]
fn manifest_rehashes_replaced_file_with_same_size_and_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    fs::create_dir_all(&share).unwrap();
    let file = share.join("a.txt");
    fs::write(&file, b"AAAAA").unwrap();

    let state = dir.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let manifest_path = state.join("manifest.json");
    let cache_path = state.join("manifest.cache.json");

    let (first, _) = manifest::generate(&share, &manifest_path, &cache_path).unwrap();

    // Replace the file atomically (a new inode), keeping the same size, and
    // restore the original mtime. The cache key must still force a re-hash.
    let mtime = fs::metadata(&file).unwrap().modified().unwrap();
    let replacement = share.join("replacement.tmp");
    fs::write(&replacement, b"BBBBB").unwrap();
    fs::rename(&replacement, &file).unwrap();
    let handle = fs::File::options().write(true).open(&file).unwrap();
    handle
        .set_times(fs::FileTimes::new().set_modified(mtime))
        .unwrap();

    let (second, _) = manifest::generate(&share, &manifest_path, &cache_path).unwrap();
    assert_eq!(first.files[0].size, second.files[0].size);
    assert_ne!(first.files[0].sha256, second.files[0].sha256);
}
