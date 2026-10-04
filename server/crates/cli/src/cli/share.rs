//! `lanpull share` — share listing and manifest regeneration.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use serde::Serialize;

use crate::cli::output::format_rows;
use crate::cli::Outcome;
use crate::manifest_helpers::report_regeneration;

/// Share operations.
#[derive(Debug, Subcommand)]
pub enum ShareCommand {
    /// List the declared shares and their directories.
    List,
    /// Regenerate the per-share and per-account manifests.
    Rescan,
    /// Add a share directory to the configuration.
    Add {
        /// Share name (`^[a-z0-9][a-z0-9_-]*$`).
        name: String,
        /// Directory to distribute.
        dir: PathBuf,
        /// Show what would change without writing the configuration.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a share from the configuration.
    Remove {
        /// Share name.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Show what would change without writing the configuration.
        #[arg(long)]
        dry_run: bool,
    },
}

impl ShareCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::List => "share list",
            Self::Rescan => "share rescan",
            Self::Add { .. } => "share add",
            Self::Remove { .. } => "share remove",
        }
    }
}

/// One share row.
#[derive(Debug, Serialize)]
struct ShareRow {
    name: String,
    dir: String,
}

/// Arguments for `init` (share creation).
#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Directory to distribute; repeat for several shares as `name=path`.
    #[arg(long = "share", value_name = "NAME=PATH")]
    pub shares: Vec<String>,
    /// Internal state directory.
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    /// Listen address.
    #[arg(long)]
    pub bind: Option<String>,
    /// Listen port.
    #[arg(long)]
    pub port: Option<u16>,
    /// Address clients use (embedded in the certificate).
    #[arg(long)]
    pub server_ip: Option<String>,
    /// Accept the detected values without prompting.
    #[arg(long)]
    pub yes: bool,
    /// Overwrite an existing configuration file.
    #[arg(long)]
    pub force: bool,
}

/// Dispatch a share operation.
pub fn run(config_path: &Path, command: ShareCommand) -> Result<Outcome> {
    match command {
        ShareCommand::List => list(config_path),
        ShareCommand::Rescan => rescan(config_path),
        ShareCommand::Add { name, dir, dry_run } => add(config_path, &name, &dir, dry_run),
        ShareCommand::Remove { name, yes, dry_run } => remove(config_path, &name, yes, dry_run),
    }
}

/// List the declared shares.
fn list(config_path: &Path) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let rows: Vec<ShareRow> = config
        .shares
        .iter()
        .map(|(name, dir)| ShareRow {
            name: name.clone(),
            dir: dir.display().to_string(),
        })
        .collect();
    let table = format_rows(
        &rows
            .iter()
            .map(|row| vec![row.name.clone(), row.dir.clone()])
            .collect::<Vec<_>>(),
    );
    let lines = if rows.is_empty() {
        vec!["no shares".to_string()]
    } else {
        table
    };
    Ok(Outcome::text(lines).with_data(&rows))
}

/// Regenerate the manifests.
fn rescan(config_path: &Path) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut outcome = Outcome::new();
    report_regeneration(&config, &mut outcome)?;
    if outcome.lines.is_empty() {
        outcome.lines.push("manifest regenerated".to_string());
    }
    Ok(outcome)
}

/// Add a share to the configuration, creating the directory if absent.
fn add(config_path: &Path, name: &str, dir: &Path, dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    if config.shares.contains_key(name) {
        return Err(Error::Config(format!("share {name} already exists")));
    }
    if !lanpull_core::config::valid_share_name(name) {
        return Err(Error::Config(format!(
            "invalid share name {name}: must match ^[a-z0-9][a-z0-9_-]*$"
        )));
    }
    if !dir.is_dir() {
        return Err(Error::Config(format!(
            "share directory does not exist: {}",
            dir.display()
        )));
    }
    let summary = if dry_run {
        format!("would add share {name} = {}", dir.display())
    } else {
        crate::config_edit::append_share(config_path, name, dir)?;
        format!("added share {name}")
    };
    Ok(Outcome::new()
        .line(summary)
        .with_data(&serde_json::json!({ "name": name, "dir": dir, "dry_run": dry_run })))
}

/// Remove a share from the configuration.
fn remove(config_path: &Path, name: &str, yes: bool, dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    if !config.shares.contains_key(name) {
        return Err(Error::Config(format!("no such share: {name}")));
    }
    if !dry_run {
        crate::confirm::require(yes, &format!("remove share {name}"))?;
    }
    let summary = if dry_run {
        format!("would remove share {name}")
    } else {
        crate::config_edit::remove_share(config_path, name)?;
        format!("removed share {name}")
    };
    Ok(Outcome::new()
        .line(summary)
        .with_data(&serde_json::json!({ "name": name, "dry_run": dry_run })))
}
