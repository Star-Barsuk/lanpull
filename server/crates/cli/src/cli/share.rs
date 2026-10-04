//! `lanpull share` — share listing and manifest regeneration.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use serde::Serialize;

use crate::cli::{print_json, JsonFlag};
use crate::manifest_helpers::report_regeneration;

/// Share operations.
#[derive(Debug, Subcommand)]
pub enum ShareCommand {
    /// List the declared shares and their directories.
    List(JsonFlag),
    /// Regenerate the per-share and per-account manifests.
    Rescan,
    /// Add a share directory to the configuration.
    Add {
        /// Share name (`^[a-z0-9][a-z0-9_-]*$`).
        name: String,
        /// Directory to distribute.
        dir: PathBuf,
    },
    /// Remove a share from the configuration.
    Remove {
        /// Share name.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
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
    /// Accept defaults without prompting.
    #[arg(long)]
    pub non_interactive: bool,
}

/// Dispatch a share operation.
pub fn run(config_path: &Path, command: ShareCommand) -> Result<()> {
    match command {
        ShareCommand::List(json) => list(config_path, json),
        ShareCommand::Rescan => rescan(config_path),
        ShareCommand::Add { name, dir } => add(config_path, &name, &dir),
        ShareCommand::Remove { name, yes } => remove(config_path, &name, yes),
    }
}

/// List the declared shares.
fn list(config_path: &Path, json: JsonFlag) -> Result<()> {
    let config = Config::load(config_path)?;
    let rows: Vec<ShareRow> = config
        .shares
        .iter()
        .map(|(name, dir)| ShareRow {
            name: name.clone(),
            dir: dir.display().to_string(),
        })
        .collect();
    if json.json {
        return print_json(&rows);
    }
    for row in rows {
        println!("{:<16} {}", row.name, row.dir);
    }
    Ok(())
}

/// Regenerate the manifests.
fn rescan(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    report_regeneration(&config)
}

/// Add a share to the configuration, creating the directory if absent.
fn add(config_path: &Path, name: &str, dir: &Path) -> Result<()> {
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
    crate::config_edit::append_share(config_path, name, dir)?;
    tracing::info!("share {name} added");
    Ok(())
}

/// Remove a share from the configuration.
fn remove(config_path: &Path, name: &str, yes: bool) -> Result<()> {
    crate::confirm::require(yes, &format!("remove share {name}"))?;
    crate::config_edit::remove_share(config_path, name)?;
    tracing::info!("share {name} removed");
    Ok(())
}
