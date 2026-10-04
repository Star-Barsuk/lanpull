//! Shared manifest regeneration used after policy and account changes.

use lanpull_core::config::Config;
use lanpull_core::error::Result;
use lanpull_store::manifest;

/// Regenerate manifests and log a one-line summary per share and account.
pub fn report_regeneration(config: &Config) -> Result<()> {
    let report = manifest::regenerate(config)?;
    for (share, count) in &report.share_files {
        tracing::info!("manifest {share}: {count} files");
    }
    for (account, count) in &report.account_files {
        tracing::info!("access {account}: {count} files visible");
    }
    for warning in &report.warnings {
        tracing::warn!("{warning}");
    }
    Ok(())
}
