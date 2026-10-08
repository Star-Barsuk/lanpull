//! Router and authentication tests through `tower::ServiceExt::oneshot`.
//!
//! Tests may unwrap and use bare asserts for brevity; production code may not.
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
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use http_body_util::BodyExt as _;
use lanpull_core::access::Rule;
use lanpull_core::arm::ArmState;
use lanpull_core::clients::{self, Account, Clients};
use lanpull_core::config::Config;
use lanpull_core::policy::Policy;
use lanpull_http::{router, AppState};
use tower::ServiceExt as _;

const REASON_HEADER: &str = "x-lanpull-reason";

fn account(name: &str, password: &str, ip: Option<&str>) -> Account {
    Account {
        name: name.to_string(),
        hash: clients::hash_password(password).unwrap(),
        allowed_ip: ip.map(|value| value.parse().unwrap()),
        local: false,
    }
}

/// Convert legacy `<account> <share>[:<glob>]` lines into a JSON policy.
///
/// A `*` share expands to every configured share; a rule without a glob grants
/// the whole share. This keeps the test call sites readable.
fn policy_from_flat(shares: &BTreeMap<String, PathBuf>, text: &str) -> Policy {
    let mut policy = Policy::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let account = fields.next().unwrap();
        let spec = fields.next().unwrap();
        let rule = Rule::parse(spec).unwrap();
        let names: Vec<String> = rule.share.as_ref().map_or_else(
            || shares.keys().cloned().collect(),
            |name| vec![name.clone()],
        );
        let path = rule
            .glob
            .as_ref()
            .map_or_else(|| "**".to_string(), |glob| glob.as_str().to_string());
        for name in names {
            policy
                .shares
                .entry(name)
                .or_default()
                .clients
                .entry(account.to_string())
                .or_default()
                .add
                .insert(path.clone());
        }
    }
    policy
}

/// Build shared server state with one `reports` share and a matching manifest.
fn build(accounts: Vec<Account>, access_text: &str) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let share = dir.path().join("share");
    let state_dir = dir.path().join("state");
    fs::create_dir_all(share.join("sub")).unwrap();
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(share.join("a.txt"), b"hello world").unwrap();
    fs::write(share.join("sub/b.txt"), b"nested").unwrap();

    let clients_path = dir.path().join("lanpull.clients");
    let mut clients = Clients::new();
    for entry in accounts {
        clients.insert(entry);
    }
    clients.save(&clients_path).unwrap();

    let mut shares = BTreeMap::new();
    shares.insert("reports".to_string(), share);

    let access_path = dir.path().join("lanpull.access.json");
    policy_from_flat(&shares, access_text)
        .save(&access_path)
        .unwrap();

    let config = Config {
        shares,
        state_dir: state_dir.clone(),
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        server_ip: "127.0.0.1".parse().unwrap(),
        cert_path: state_dir.join("server.crt"),
        key_path: state_dir.join("server.key"),
        clients_path,
        access_path,
        networks_path: state_dir.join("lanpull.networks.json"),
        audit_log: state_dir.join("access.log"),
    };

    lanpull_store::manifest::regenerate(&config).unwrap();

    let state = AppState::new(config);
    (state, dir)
}

fn arm(state: &AppState, names: &[&str]) {
    let mut arm_state = ArmState::default();
    for name in names {
        arm_state.arm(name, i64::MAX);
    }
    arm_state.save(&state.config.arm_path()).unwrap();
}

fn stage_bundle(state: &AppState) {
    let dir = state.config.bundle_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("VERSION"), b"9.9.9\n").unwrap();
    fs::write(dir.join("pull.py"), b"print('hi')\n").unwrap();
}

fn basic(credentials: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(credentials)
    )
}

fn request(method: &str, uri: &str, auth: Option<&str>, ip: [u8; 4]) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(credentials) = auth {
        builder = builder.header(header::AUTHORIZATION, basic(credentials));
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((ip, 5000))));
    request
}

async fn call(state: &AppState, request: Request<Body>) -> axum::response::Response {
    router(state.clone()).oneshot(request).await.unwrap()
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

const fn manifest_uri() -> &'static str {
    "/_lanpull/share/reports/manifest.json"
}

#[tokio::test]
async fn missing_credentials_are_rejected() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(&state, request("GET", manifest_uri(), None, [127, 0, 0, 1])).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_password_reports_bad_password() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:wrong"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(REASON_HEADER).unwrap(),
        "bad_password"
    );
}

#[tokio::test]
async fn repeated_failures_are_rate_limited() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    for _ in 0..10 {
        let response = call(
            &state,
            request("GET", manifest_uri(), Some("alpha:wrong"), [127, 0, 0, 1]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(REASON_HEADER).unwrap(),
        "rate_limited"
    );
}

#[tokio::test]
async fn unarmed_account_is_rejected() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers().get(REASON_HEADER).unwrap(), "not_armed");
}

#[tokio::test]
async fn local_account_skips_arm_window() {
    let mut alpha = account("alpha", "secret", None);
    alpha.local = true;
    let (state, _dir) = build(vec![alpha], "alpha reports\n");
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn armed_account_reads_manifest() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["scheme"], "whole-file-v1");
}

#[tokio::test]
async fn filtered_manifest_omits_disallowed_paths() {
    let (state, _dir) = build(
        vec![account("alpha", "secret", None)],
        "alpha reports:a.txt\n",
    );
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let files = parsed["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "a.txt");
}

#[tokio::test]
async fn disallowed_share_is_forbidden() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha *\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/share/other/manifest.json",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn account_without_rules_is_forbidden() {
    let (state, _dir) = build(
        vec![account("alpha", "secret", None)],
        "someoneelse reports\n",
    );
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers().get(REASON_HEADER).unwrap(), "forbidden");
}

#[tokio::test]
async fn disallowed_file_path_is_forbidden() {
    let (state, _dir) = build(
        vec![account("alpha", "secret", None)],
        "alpha reports:a.txt\n",
    );
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/share/reports/file/sub/b.txt",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn range_request_is_partial() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let mut request = request(
        "GET",
        "/_lanpull/share/reports/file/a.txt",
        Some("alpha:secret"),
        [127, 0, 0, 1],
    );
    request
        .headers_mut()
        .insert(header::RANGE, "bytes=0-4".parse().unwrap());
    let response = call(&state, request).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_bytes(response).await, b"hello");
}

#[tokio::test]
async fn symlink_file_is_not_served() {
    let (state, dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let outside = dir.path().join("secret.txt");
    fs::write(&outside, b"secret").unwrap();
    let link = state.config.shares.get("reports").unwrap().join("link.txt");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/share/reports/file/link.txt",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bundle_manifest_and_missing_bundle() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);

    let missing = call(
        &state,
        request(
            "GET",
            "/_lanpull/client/manifest.json",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::SERVICE_UNAVAILABLE);

    stage_bundle(&state);
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/client/manifest.json",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["version"], "9.9.9");
}

#[tokio::test]
async fn bundle_file_is_served() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    stage_bundle(&state);

    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/client/pull.py",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"print('hi')\n");

    let unknown = call(
        &state,
        request(
            "GET",
            "/_lanpull/client/nope",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_path_is_not_found() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/share/reports/file/missing.txt",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn legacy_manifest_route_is_gone() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/manifest.json",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_is_method_not_allowed() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request("POST", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn missing_manifest_is_unavailable() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let path: PathBuf = state.config.access_manifest_path("alpha", "reports");
    fs::remove_file(path).unwrap();
    let response = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn ip_bound_account_rejects_foreign_address() {
    let (state, _dir) = build(
        vec![account("alpha", "secret", Some("10.0.0.5"))],
        "alpha reports\n",
    );
    arm(&state, &["alpha"]);
    let foreign = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [10, 0, 0, 9]),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(foreign.headers().get(REASON_HEADER).unwrap(), "foreign_ip");

    let allowed = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [10, 0, 0, 5]),
    )
    .await;
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn unbound_account_accepts_any_address() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request(
            "GET",
            manifest_uri(),
            Some("alpha:secret"),
            [192, 168, 1, 9],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn head_manifest_has_length() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request("HEAD", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key(header::CONTENT_LENGTH));
}

#[tokio::test]
async fn head_file_has_length() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let response = call(
        &state,
        request(
            "HEAD",
            "/_lanpull/share/reports/file/a.txt",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key(header::CONTENT_LENGTH));
}

#[tokio::test]
async fn unsatisfiable_range_is_416() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let mut request = request(
        "GET",
        "/_lanpull/share/reports/file/a.txt",
        Some("alpha:secret"),
        [127, 0, 0, 1],
    );
    request
        .headers_mut()
        .insert(header::RANGE, "bytes=1000-2000".parse().unwrap());
    let response = call(&state, request).await;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test]
async fn bundle_symlink_is_not_served() {
    let (state, dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    stage_bundle(&state);
    let outside = dir.path().join("secret.txt");
    fs::write(&outside, b"secret").unwrap();
    std::os::unix::fs::symlink(&outside, state.config.bundle_dir().join("link.py")).unwrap();
    let response = call(
        &state,
        request(
            "GET",
            "/_lanpull/client/link.py",
            Some("alpha:secret"),
            [127, 0, 0, 1],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn requests_are_audited() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let _ = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    let log = fs::read_to_string(&state.config.audit_log).unwrap();
    assert!(log.contains("\"user\":\"alpha\""));
    assert!(log.contains("\"status\":200"));
}

#[tokio::test]
async fn forbidden_requests_are_audited_with_reason() {
    let (state, _dir) = build(
        vec![account("alpha", "secret", None)],
        "someoneelse reports\n",
    );
    arm(&state, &["alpha"]);
    let _ = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    let log = fs::read_to_string(&state.config.audit_log).unwrap();
    assert!(log.contains("\"status\":403"));
    assert!(log.contains("\"reason\":\"forbidden\""));
}

#[tokio::test]
async fn remove_client_revokes_immediately() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let first = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    let mut clients = Clients::load(&state.config.clients_path).unwrap();
    assert!(clients.remove("alpha"));
    clients.save(&state.config.clients_path).unwrap();

    let second = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn disarm_takes_effect_immediately() {
    let (state, _dir) = build(vec![account("alpha", "secret", None)], "alpha reports\n");
    arm(&state, &["alpha"]);
    let first = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    ArmState::default().save(&state.config.arm_path()).unwrap();

    let second = call(
        &state,
        request("GET", manifest_uri(), Some("alpha:secret"), [127, 0, 0, 1]),
    )
    .await;
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(second.headers().get(REASON_HEADER).unwrap(), "not_armed");
}
