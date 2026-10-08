//! `lanpull status`, `report`, `audit`, and `clean`.

use std::path::Path;

use lanpull_core::arm::ArmState;
use lanpull_core::audit;
use lanpull_core::clients::Clients;
use lanpull_core::config::Config;
use lanpull_core::error::Result;
use lanpull_core::human;
use lanpull_core::timeutil;
use lanpull_store::status;
use serde::Serialize;

use crate::cli::output::format_rows;
use crate::cli::Outcome;

/// Arguments for `status`.
#[derive(Debug, clap::Args)]
pub struct StatusArgs {
    /// Show only the armed accounts and their remaining windows.
    #[arg(long)]
    pub arm: bool,
}

/// Arguments for `report`.
#[derive(Debug, clap::Args)]
pub struct ReportArgs {
    /// Only show one account.
    #[arg(long, alias = "account")]
    pub user: Option<String>,
    /// Only show requests newer than this duration, for example `7d`.
    #[arg(long)]
    pub since: Option<String>,
    /// Break down rejected requests by reason.
    #[arg(long)]
    pub reasons: bool,
    /// Only show accounts with rejected requests.
    #[arg(long)]
    pub rejected: bool,
    /// Show only the most recent records instead of the summary.
    #[arg(long, value_name = "N", conflicts_with_all = ["reasons", "rejected"])]
    pub tail: Option<usize>,
}

/// Arguments for `clean`.
#[derive(Debug, clap::Args)]
pub struct CleanArgs {
    /// Show what would be removed without removing it.
    #[arg(long)]
    pub dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// Show operator status.
pub fn status(config_path: &Path, args: StatusArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    if args.arm {
        return status_arm(&config);
    }
    let report = status::report(&config)?;
    let mut lines = report.manifests.clone();
    for account in &report.armed {
        lines.push(format!(
            "armed: {} ({} left)",
            account.account,
            status::format_duration(account.remaining_secs)
        ));
    }
    if lines.is_empty() {
        lines.push("status is clean".to_string());
    }
    let data = serde_json::json!({ "manifests": report.manifests, "armed": report.armed });
    Ok(Outcome {
        lines,
        data,
        warnings: report.warnings,
    })
}

/// Show only the armed accounts and their remaining windows.
///
/// This is the cheap path: it reads the arm state and the account list, and
/// never walks a share directory or a manifest.
fn status_arm(config: &Config) -> Result<Outcome> {
    let now = timeutil::now_unix();
    let arm = ArmState::load(&config.arm_path())?;
    // Arm entries outlive a removed account; only list accounts that still exist.
    let known = Clients::load(&config.clients_path).ok();
    let armed: Vec<(String, i64)> = arm
        .armed_entries(now)
        .into_iter()
        .filter(|(name, _)| {
            known
                .as_ref()
                .is_none_or(|clients| clients.get(name).is_some())
        })
        .collect();

    if armed.is_empty() {
        return Ok(Outcome::new().line("armed: none").with_data(
            &serde_json::json!({ "scope": "arm", "arm": Vec::<serde_json::Value>::new() }),
        ));
    }

    let mut outcome = Outcome::new();
    let mut data = Vec::new();
    for (name, remaining) in &armed {
        let expires = now.saturating_add(*remaining);
        let at = timeutil::iso8601(expires);
        outcome = outcome.line(format!(
            "armed {name} {} left (until {at})",
            status::format_duration(*remaining)
        ));
        data.push(serde_json::json!({
            "account": name,
            "expires_at": at,
            "remaining_secs": remaining,
        }));
    }
    outcome.data = serde_json::json!({ "scope": "arm", "arm": data });
    Ok(outcome)
}

/// Summarize the audit log.
pub fn report(config_path: &Path, args: ReportArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let cutoff = match args.since.as_deref() {
        Some(value) => {
            let seconds = timeutil::parse_duration_secs(value)?;
            Some(timeutil::now_unix().saturating_sub(seconds))
        }
        None => None,
    };

    if let Some(tail) = args.tail {
        return report_tail(&config.audit_log, args.user.as_deref(), cutoff, tail);
    }

    let summary = audit::summarize(&config.audit_log, args.user.as_deref(), cutoff)?;
    let accounts: Vec<&audit::AccountSummary> = summary
        .accounts
        .iter()
        .filter(|account| !args.rejected || account.rejected > 0)
        .collect();
    if accounts.is_empty() {
        return Ok(Outcome::text(vec!["no requests recorded".to_string()]).with_data(&summary));
    }

    let mut rows = vec![vec![
        "ACCOUNT".to_string(),
        "LAST SEEN".to_string(),
        "HOST".to_string(),
        "FILES".to_string(),
        "SIZE".to_string(),
        "REJECTED".to_string(),
    ]];
    for account in &accounts {
        rows.push(vec![
            account.user.clone(),
            account.last_seen.clone(),
            if account.last_host.is_empty() {
                "<unknown>".to_string()
            } else {
                account.last_host.clone()
            },
            account.files.to_string(),
            human::format_bytes(account.bytes),
            account.rejected.to_string(),
        ]);
    }
    let mut lines = format_rows(&rows);

    if args.reasons {
        let mut totals: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for account in &accounts {
            for (reason, count) in &account.by_reason {
                let total = totals.entry(reason.clone()).or_insert(0);
                *total = total.saturating_add(*count);
            }
        }
        lines.push(String::new());
        if totals.is_empty() {
            lines.push("no rejections recorded".to_string());
        } else {
            lines.push("rejections by reason:".to_string());
            let reason_rows: Vec<Vec<String>> = totals
                .iter()
                .map(|(reason, count)| vec![reason.clone(), count.to_string()])
                .collect();
            lines.extend(format_rows(&reason_rows));
        }
    }

    Ok(Outcome::text(lines).with_data(&summary))
}

/// Show the most recent `tail` audit records instead of the summary.
fn report_tail(
    log: &Path,
    user: Option<&str>,
    cutoff: Option<i64>,
    tail: usize,
) -> Result<Outcome> {
    let records = audit::read_records(log, user, cutoff)?;
    let start = records.len().saturating_sub(tail);
    let shown: &[audit::Record] = records.get(start..).unwrap_or(&[]);
    if shown.is_empty() {
        return Ok(Outcome::new()
            .line("no requests recorded")
            .with_data(&serde_json::json!({ "records": Vec::<audit::Record>::new() })));
    }
    let mut rows = vec![vec![
        "TS".to_string(),
        "USER".to_string(),
        "IP".to_string(),
        "METHOD".to_string(),
        "PATH".to_string(),
        "STATUS".to_string(),
        "REASON".to_string(),
    ]];
    for record in shown {
        rows.push(vec![
            record.ts.clone(),
            record.user.clone(),
            record.ip.clone(),
            record.method.clone(),
            record.path.clone(),
            record.status.to_string(),
            record.reason.clone().unwrap_or_default(),
        ]);
    }
    Ok(Outcome::text(format_rows(&rows)).with_data(&serde_json::json!({ "records": shown })))
}

/// One discovered artifact.
#[derive(Debug, Serialize)]
struct Artifact {
    kind: String,
    path: String,
    present: bool,
}

/// Audit the artifacts lanpull created on this host.
pub fn audit(config_path: &Path) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut artifacts: Vec<Artifact> = Vec::new();

    let mut push = |kind: &str, path: std::path::PathBuf| {
        artifacts.push(Artifact {
            kind: kind.to_string(),
            present: path.exists(),
            path: path.display().to_string(),
        });
    };
    push("config", config_path.to_path_buf());
    push("clients", config.clients_path.clone());
    push("access", config.access_path.clone());
    push("networks", config.networks_path.clone());
    push("cert", config.cert_path.clone());
    push("key", config.key_path.clone());
    push("state", config.state_dir.clone());
    push("manifest", config.manifest_dir());
    push("arm", config.arm_path());
    push("audit-log", config.audit_log.clone());
    push("bundle", config.bundle_dir());
    for (name, dir) in &config.shares {
        push(&format!("share:{name}"), dir.clone());
    }

    let mut rows = vec![vec![
        "KIND".to_string(),
        "STATE".to_string(),
        "PATH".to_string(),
    ]];
    for artifact in &artifacts {
        rows.push(vec![
            artifact.kind.clone(),
            if artifact.present {
                "present".to_string()
            } else {
                "missing".to_string()
            },
            artifact.path.clone(),
        ]);
    }
    let lines = format_rows(&rows);
    Ok(Outcome::text(lines).with_data(&artifacts))
}

/// Remove runtime leftovers that no other teardown step covers.
pub fn clean(config_path: &Path, args: CleanArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut targets: Vec<std::path::PathBuf> = Vec::new();

    // Orphaned client-ready folders for accounts that no longer exist.
    let accounts = Clients::load(&config.clients_path)?;
    let ready = config.state_dir.join("client-ready");
    if let Ok(entries) = std::fs::read_dir(&ready) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if accounts.get(&name).is_none() {
                targets.push(entry.path());
            }
        }
    }

    if targets.is_empty() {
        return Ok(Outcome::new()
            .line("nothing to clean")
            .with_data(&serde_json::json!({ "removed": [], "dry_run": args.dry_run })));
    }
    if args.dry_run {
        let lines: Vec<String> = targets
            .iter()
            .map(|path| format!("would remove {}", path.display()))
            .collect();
        return Ok(Outcome::text(lines).with_data(&serde_json::json!({
            "removed": targets,
            "dry_run": true,
        })));
    }
    crate::confirm::require(
        args.yes,
        &format!("remove {} leftover path(s)", targets.len()),
    )?;
    for path in &targets {
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    let lines: Vec<String> = targets
        .iter()
        .map(|path| format!("removed {}", path.display()))
        .collect();
    Ok(Outcome::text(lines).with_data(&serde_json::json!({
        "removed": targets,
        "dry_run": false,
    })))
}
