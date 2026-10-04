//! Operator status checks and safety nets.
//!
//! `lanpull status` and server startup compare the newest share file against
//! the manifest and report symlinks, reserved-prefix entries, and armed
//! accounts (`docs/SPEC.md` section 11).

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use walkdir::WalkDir;

use lanpull_core::arm::ArmState;
use lanpull_core::clients::Clients;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::policy::Policy;
use lanpull_core::relpath;
use lanpull_core::timeutil;

use crate::manifest::Manifest;

/// Neutral starting point for the newest-mtime scan (any real mtime is above it).
const MIN_MTIME: i64 = i64::MIN;

/// Warnings suitable for server startup.
pub fn startup_warnings(config: &Config) -> Vec<String> {
    let mut warnings = Vec::new();

    for (share, dir) in &config.shares {
        match Manifest::load(&config.manifest_path(share)) {
            Ok(manifest) => {
                warnings.extend(
                    freshness_warnings(dir, &manifest)
                        .into_iter()
                        .map(|warning| format!("{share}: {warning}")),
                );
            }
            Err(_) => warnings.push(format!(
                "{share}: no manifest yet; run make rescan before the first pull"
            )),
        }
        match walk_warnings(dir) {
            Ok(more) => warnings.extend(
                more.into_iter()
                    .map(|warning| format!("{share}: {warning}")),
            ),
            Err(e) => warnings.push(format!("{share}: cannot inspect share directory: {e}")),
        }
    }

    warnings.extend(access_warnings(config));
    warnings
}

/// Full status output for the operator.
pub fn run(config: &Config) -> Result<Vec<String>> {
    let mut lines = Vec::new();

    for (share, dir) in &config.shares {
        match Manifest::load(&config.manifest_path(share)) {
            Ok(manifest) => {
                lines.push(format!(
                    "manifest {share}: {} files, generated_at {}",
                    manifest.files.len(),
                    manifest.generated_at
                ));
                lines.extend(
                    freshness_warnings(dir, &manifest)
                        .into_iter()
                        .map(|warning| format!("{share}: {warning}")),
                );
            }
            Err(_) => lines.push(format!("manifest {share}: MISSING (run make rescan)")),
        }
        lines.extend(
            walk_warnings(dir)?
                .into_iter()
                .map(|warning| format!("{share}: {warning}")),
        );
    }

    lines.extend(access_warnings(config));

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
        lines.push("armed: none".to_string());
    } else {
        for (name, remaining) in armed {
            lines.push(format!(
                "armed: {name} ({} left)",
                format_duration(remaining)
            ));
        }
    }

    Ok(lines)
}

/// Warn when files in the share are newer than the manifest.
pub fn freshness_warnings(share: &Path, manifest: &Manifest) -> Vec<String> {
    let mut warnings = Vec::new();
    match newest_mtime(share) {
        Ok(Some(newest)) => {
            if let Ok(generated) = timeutil::parse_iso8601(&manifest.generated_at) {
                if newest > generated {
                    warnings.push(
                        "share has files newer than the manifest; run make rescan".to_string(),
                    );
                }
            }
        }
        Ok(None) => {}
        Err(e) => warnings.push(format!("cannot inspect share directory: {e}")),
    }
    warnings
}

/// Warn about accounts without rules and policy entries naming unknown shares.
pub fn access_warnings(config: &Config) -> Vec<String> {
    let mut warnings = Vec::new();
    let policy = match Policy::load(&config.access_path) {
        Ok(policy) => policy,
        Err(e) => {
            warnings.push(format!("cannot read access policy: {e}"));
            return warnings;
        }
    };

    for share in policy.shares.keys() {
        if !config.shares.contains_key(share) {
            warnings.push(format!("access: policy references unknown share {share}"));
        }
    }

    match Clients::load(&config.clients_path) {
        Ok(clients) => {
            let access = match policy.expand(&clients) {
                Ok(access) => access,
                Err(e) => {
                    warnings.push(format!("cannot expand access policy: {e}"));
                    return warnings;
                }
            };
            if !clients.is_empty() && access.is_empty() {
                warnings.push("no access rules; every account can pull nothing".to_string());
            }
            for account in clients.iter() {
                if !access.has_rules(&account.name) {
                    warnings.push(format!("account {} has no access rules", account.name));
                }
            }
        }
        Err(e) => warnings.push(format!("cannot read accounts: {e}")),
    }

    warnings
}

/// Report symlinks and reserved-prefix entries under the share.
pub fn walk_warnings(share: &Path) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for entry in WalkDir::new(share).follow_links(false) {
        let entry = entry.map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        if entry.file_type().is_dir() {
            continue;
        }
        let Ok(rel) = relpath::to_relative_string(share, entry.path()) else {
            continue;
        };
        if entry.file_type().is_symlink() {
            warnings.push(format!("symlink skipped: {rel}"));
        } else if relpath::is_reserved(&rel) {
            warnings.push(format!("reserved-prefix entry ignored: {rel}"));
        }
    }
    Ok(warnings)
}

/// Newest file modification time under `share`, if any.
pub fn newest_mtime(share: &Path) -> Result<Option<i64>> {
    let mut newest = MIN_MTIME;
    let mut found = false;
    for entry in WalkDir::new(share).follow_links(false) {
        let entry = entry.map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let mtime = entry
            .metadata()
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?
            .mtime();
        if mtime > newest {
            newest = mtime;
        }
        found = true;
    }
    Ok(if found { Some(newest) } else { None })
}

/// Format a remaining duration compactly.
pub fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60)
    }
}
