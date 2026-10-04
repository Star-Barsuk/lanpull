//! Shared manifest regeneration used after policy and account changes.

use lanpull_core::config::Config;
use lanpull_core::error::Result;
use lanpull_store::manifest;

use crate::cli::Outcome;

/// Regenerate manifests and append a one-line summary per share and account.
pub fn report_regeneration(config: &Config, outcome: &mut Outcome) -> Result<()> {
    let report = manifest::regenerate(config)?;
    for (share, count) in &report.share_files {
        outcome
            .lines
            .push(format!("manifest {share}: {count} files"));
    }
    for (account, count) in &report.account_files {
        outcome
            .lines
            .push(format!("access {account}: {count} files visible"));
    }
    outcome.warnings.extend(report.warnings);
    Ok(())
}
