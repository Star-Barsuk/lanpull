//! `lanpull status`, `report`, `audit`, and `clean`.

use std::path::Path;

use lanpull_core::audit;
use lanpull_core::config::Config;
use lanpull_core::error::Result;
use lanpull_core::timeutil;
use lanpull_store::status;
use serde::Serialize;

use crate::cli::output::format_rows;
use crate::cli::Outcome;

/// Arguments for `report`.
#[derive(Debug, clap::Args)]
pub struct ReportArgs {
    /// Only show one account.
    #[arg(long)]
    pub user: Option<String>,
    /// Only show requests newer than this duration, for example `7d`.
    #[arg(long)]
    pub since: Option<String>,
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
pub fn status(config_path: &Path) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let lines = status::run(&config)?;
    let text = if lines.is_empty() {
        vec!["status is clean".to_string()]
    } else {
        lines.clone()
    };
    Ok(Outcome::text(text).with_data(&serde_json::json!({ "lines": lines })))
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
    let summary = audit::summarize(&config.audit_log, args.user.as_deref(), cutoff)?;
    let lines = summary.lines();
    Ok(Outcome::text(lines).with_data(&summary))
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

    let lines = format_rows(
        &artifacts
            .iter()
            .map(|artifact| {
                let mark = if artifact.present {
                    "present"
                } else {
                    "missing"
                };
                vec![
                    artifact.kind.clone(),
                    mark.to_string(),
                    artifact.path.clone(),
                ]
            })
            .collect::<Vec<_>>(),
    );
    Ok(Outcome::text(lines).with_data(&artifacts))
}

/// Remove runtime leftovers that no other teardown step covers.
pub fn clean(config_path: &Path, args: CleanArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut targets: Vec<std::path::PathBuf> = Vec::new();

    // Orphaned share-list temp file written by an aborted teardown.
    let shares_file = config_path.with_extension("conf.shares");
    if shares_file.exists() {
        targets.push(shares_file);
    }
    // Orphaned client-ready folders for accounts that no longer exist.
    let accounts = lanpull_core::clients::Clients::load(&config.clients_path)?;
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
