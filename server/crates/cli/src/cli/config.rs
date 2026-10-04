//! `lanpull config` — read or change the configuration.

use std::path::Path;

use clap::Subcommand;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use serde::Serialize;

use crate::cli::{print_json, JsonFlag};

/// Configuration operations.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the resolved configuration and its paths.
    Show(JsonFlag),
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
    },
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
pub fn run(config_path: &Path, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Show(json) => show(config_path, json),
        ConfigCommand::Get { key } => get(config_path, &key),
        ConfigCommand::Set { key, value } => set(config_path, &key, &value),
    }
}

/// Print the resolved configuration.
fn show(config_path: &Path, json: JsonFlag) -> Result<()> {
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
    if json.json {
        return print_json(&view);
    }
    println!("config:      {}", view.path);
    for (name, dir) in &view.shares {
        println!("SHARE_{name:<10} {dir}");
    }
    println!("STATE_DIR:   {}", view.state_dir);
    println!("BIND:        {}", view.bind);
    println!("PORT:        {}", view.port);
    println!("SERVER_IP:   {}", view.server_ip);
    println!("CERT_PATH:   {}", view.cert_path);
    println!("KEY_PATH:    {}", view.key_path);
    println!("CLIENTS_PATH:{}", view.clients_path);
    println!("ACCESS_PATH: {}", view.access_path);
    println!("AUDIT_LOG:   {}", view.audit_log);
    Ok(())
}

/// Print one configuration value.
fn get(config_path: &Path, key: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    let value = match key {
        "STATE_DIR" => config.state_dir.display().to_string(),
        "BIND" => config.bind.to_string(),
        "PORT" => config.port.to_string(),
        "SERVER_IP" => config.server_ip.to_string(),
        "CERT_PATH" => config.cert_path.display().to_string(),
        "KEY_PATH" => config.key_path.display().to_string(),
        "CLIENTS_PATH" => config.clients_path.display().to_string(),
        "ACCESS_PATH" => config.access_path.display().to_string(),
        "AUDIT_LOG" => config.audit_log.display().to_string(),
        other => crate::config_edit::get(config_path, other)?
            .ok_or_else(|| Error::Config(format!("unknown key: {other}")))?,
    };
    println!("{value}");
    Ok(())
}

/// Set one configuration value.
fn set(config_path: &Path, key: &str, value: &str) -> Result<()> {
    let settable = [
        "STATE_DIR",
        "BIND",
        "PORT",
        "SERVER_IP",
        "CERT_PATH",
        "KEY_PATH",
        "CLIENTS_PATH",
        "ACCESS_PATH",
        "AUDIT_LOG",
    ];
    if key.starts_with("SHARE_") {
        return Err(Error::Config(
            "use 'lanpull share add' to add a share".to_string(),
        ));
    }
    if !settable.contains(&key) {
        return Err(Error::Config(format!("unknown key: {key}")));
    }
    crate::config_edit::set(config_path, key, value)?;
    tracing::info!("{key} updated");
    Ok(())
}
