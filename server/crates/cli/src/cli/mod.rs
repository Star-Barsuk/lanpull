//! Command-line surface: enums, dispatch, and shared output helpers.
//!
//! The taxonomy groups operations by object:
//! `init`, `serve`, `status`, `account`, `access`, `share`, `service`,
//! `config`, `cert`, `audit`, `report`, `clean`.

pub mod access;
pub mod account;
pub mod config;
pub mod misc;
pub mod service;
pub mod share;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use lanpull_core::error::Result;

use crate::cli::access::AccessCommand;
use crate::cli::account::AccountCommand;
use crate::cli::config::ConfigCommand;
use crate::cli::service::ServiceCommand;
use crate::cli::share::ShareCommand;

/// lanpull server: manual file distribution over a local network.
#[derive(Debug, Parser)]
#[command(name = "lanpull", version, about)]
pub struct Cli {
    /// Path to the server configuration file. Overrides `$LANPULL_CONFIG` and
    /// the canonical `/etc/lanpull/lanpull.conf`.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Increase verbosity (repeatable). Overrides `$RUST_LOG`.
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Suppress everything but errors. Overrides `$RUST_LOG`.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the configuration and state for this machine.
    Init(share::InitArgs),
    /// Run the HTTPS server (production uses the systemd unit).
    Serve,
    /// Warn on a stale manifest and show armed accounts.
    Status(misc::StatusArgs),
    /// Manage accounts, passwords, and the arm window.
    Account {
        /// Account operation.
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Manage the JSON access policy.
    Access {
        /// Policy operation.
        #[command(subcommand)]
        command: AccessCommand,
    },
    /// Manage shares and their manifests.
    Share {
        /// Share operation.
        #[command(subcommand)]
        command: ShareCommand,
    },
    /// Control the systemd service.
    Service {
        /// Service operation.
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Read or change the configuration.
    Config {
        /// Configuration operation.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Generate the self-signed TLS certificate.
    Cert,
    /// Audit the artifacts lanpull created on this host.
    Audit(misc::AuditArgs),
    /// Summarize the audit log.
    Report(misc::ReportArgs),
    /// Remove runtime leftovers.
    Clean(misc::CleanArgs),
}

/// Whether output should be machine-readable JSON.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
pub struct JsonFlag {
    /// Print machine-readable JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// Print a value as pretty JSON to stdout.
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| lanpull_core::error::Error::Config(format!("JSON: {e}")))?;
    println!("{text}");
    Ok(())
}
