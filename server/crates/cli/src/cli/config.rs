//! `lanpull config` — read or change the configuration.

use std::path::Path;

use clap::Subcommand;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use serde::Serialize;

use crate::cli::Outcome;

/// Configuration operations.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the resolved configuration and its paths.
    Show,
    /// Print one configuration value.
    Get {
        /// Key, for example `STATE_DIR` or `PORT`.
        key: String,
    },
    /// Set one configuration value.
    Set {
        /// Key, for example `PORT` or `BIND`.
        key: String,
        /// New value.
        value: String,
        /// Show what would change without writing the configuration.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the resolved configuration file path.
    Path,
}

impl ConfigCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Show => "config show",
            Self::Get { .. } => "config get",
            Self::Set { .. } => "config set",
            Self::Path => "config path",
        }
    }
}

/// The full resolved configuration, for `config show`.
#[derive(Debug, Serialize)]
struct ConfigView {
    path: String,
    shares: Vec<(String, String)>,
    state_dir: String,
    bind: String,
    port: u16,
    server_ip: String,
    cert_path: String,
    key_path: String,
    clients_path: String,
    access_path: String,
    audit_log: String,
}

/// Dispatch a configuration operation.
pub fn run(config_path: &Path, command: ConfigCommand) -> Result<Outcome> {
    match command {
        ConfigCommand::Show => show(config_path),
        ConfigCommand::Get { key } => get(config_path, &key),
        ConfigCommand::Set {
            key,
            value,
            dry_run,
        } => set(config_path, &key, &value, dry_run),
        ConfigCommand::Path => Ok(Outcome::new()
            .line(config_path.display().to_string())
            .with_data(&serde_json::json!({ "path": config_path }))),
    }
}

/// Print the resolved configuration.
fn show(config_path: &Path) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let view = ConfigView {
        path: config_path.display().to_string(),
        shares: config
            .shares
            .iter()
            .map(|(name, dir)| (name.clone(), dir.display().to_string()))
            .collect(),
        state_dir: config.state_dir.display().to_string(),
        bind: config.bind.to_string(),
        port: config.port,
        server_ip: config.server_ip.to_string(),
        cert_path: config.cert_path.display().to_string(),
        key_path: config.key_path.display().to_string(),
        clients_path: config.clients_path.display().to_string(),
        access_path: config.access_path.display().to_string(),
        audit_log: config.audit_log.display().to_string(),
    };
    let port = view.port.to_string();
    let mut lines = vec![format!("{:<14}{}", "config:", view.path)];
    for (name, dir) in &view.shares {
        lines.push(format!("{name}: {dir}"));
    }
    for (key, value) in [
        ("STATE_DIR:", view.state_dir.as_str()),
        ("BIND:", view.bind.as_str()),
        ("PORT:", port.as_str()),
        ("SERVER_IP:", view.server_ip.as_str()),
        ("CERT_PATH:", view.cert_path.as_str()),
        ("KEY_PATH:", view.key_path.as_str()),
        ("CLIENTS_PATH:", view.clients_path.as_str()),
        ("ACCESS_PATH:", view.access_path.as_str()),
        ("AUDIT_LOG:", view.audit_log.as_str()),
    ] {
        lines.push(format!("{key:<14}{value}"));
    }
    Ok(Outcome::text(lines).with_data(&view))
}

/// Print one configuration value.
fn get(config_path: &Path, key: &str) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let value = if let Some(value) = resolved_value(&config, key) {
        value
    } else {
        crate::config_edit::get(config_path, key)?
            .ok_or_else(|| Error::Config(format!("unknown key: {key}")))?
    };
    Ok(Outcome::new()
        .line(value.clone())
        .with_data(&serde_json::json!({ "key": key, "value": value })))
}

/// Set one configuration value.
fn set(config_path: &Path, key: &str, value: &str, dry_run: bool) -> Result<Outcome> {
    if key.starts_with("SHARE_") {
        return Err(Error::Config(
            "use 'lanpull share add' to add a share".to_string(),
        ));
    }
    if !settable().contains(&key) {
        return Err(Error::Config(format!("unknown key: {key}")));
    }
    if value.is_empty() {
        return Err(Error::Usage(format!("{key} must not be empty")));
    }
    let summary = if dry_run {
        format!("would set {key}={value}")
    } else {
        crate::config_edit::set(config_path, key, value)?;
        format!("set {key}={value}")
    };
    Ok(Outcome::new()
        .line(summary)
        .with_data(&serde_json::json!({ "key": key, "value": value, "dry_run": dry_run })))
}

/// The keys accepted by `config get`/`config set` and resolved by the loader.
const fn settable() -> [&'static str; 9] {
    [
        "STATE_DIR",
        "BIND",
        "PORT",
        "SERVER_IP",
        "CERT_PATH",
        "KEY_PATH",
        "CLIENTS_PATH",
        "ACCESS_PATH",
        "AUDIT_LOG",
    ]
}

/// Resolve a key from the loaded configuration, or `None` for raw keys.
fn resolved_value(config: &Config, key: &str) -> Option<String> {
    match key {
        "STATE_DIR" => Some(config.state_dir.display().to_string()),
        "BIND" => Some(config.bind.to_string()),
        "PORT" => Some(config.port.to_string()),
        "SERVER_IP" => Some(config.server_ip.to_string()),
        "CERT_PATH" => Some(config.cert_path.display().to_string()),
        "KEY_PATH" => Some(config.key_path.display().to_string()),
        "CLIENTS_PATH" => Some(config.clients_path.display().to_string()),
        "ACCESS_PATH" => Some(config.access_path.display().to_string()),
        "AUDIT_LOG" => Some(config.audit_log.display().to_string()),
        _ => None,
    }
}
