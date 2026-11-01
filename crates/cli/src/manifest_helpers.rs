//! Shared manifest regeneration used after policy and account changes.

use lanpull_core::config::Config;
use lanpull_core::error::Result;
use lanpull_store::manifest;

use crate::cli::Outcome;

/// Regenerate manifests and append a one-line summary per share.
///
/// The per-account breakdown is verbose-only: with many accounts it would drown
/// the one useful line per share.
pub fn report_regeneration(config: &Config, outcome: &mut Outcome) -> Result<()> {
    let report = manifest::regenerate(config)?;
    for (share, count) in &report.share_files {
        outcome
            .lines
            .push(format!("manifest {share}: {count} files"));
    }
    if crate::verbosity::enabled() {
        for (account, count) in &report.account_files {
            outcome
                .lines
                .push(format!("access {account}: {count} files visible"));
        }
    } else if !report.account_files.is_empty() {
        outcome.lines.push(format!(
            "access: {} account(s) regenerated",
            report.account_files.len()
        ));
    }
    outcome.warnings.extend(report.warnings);
    Ok(())
}
