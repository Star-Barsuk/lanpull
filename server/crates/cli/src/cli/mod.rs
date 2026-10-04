//! Command-line surface: enums, dispatch, and shared output helpers.
//!
//! The taxonomy groups operations by object:
//! `init`, `serve`, `status`, `account`, `access`, `share`, `service`,
//! `config`, `cert`, `audit`, `report`, `clean`.
//!
//! Every command writes its result to stdout and its diagnostics to stderr.
//! With `--json` stdout carries one envelope document instead of text; the
//! process exit code is unchanged.

pub mod access;
pub mod account;
pub mod config;
pub mod misc;
pub mod output;
pub mod service;
pub mod share;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::cli::access::AccessCommand;
use crate::cli::account::AccountCommand;
use crate::cli::config::ConfigCommand;
pub use crate::cli::output::{emit, emit_error, Outcome};
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
    /// Print machine-readable JSON on stdout instead of text.
    #[arg(long, global = true)]
    pub json: bool,
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
    Status,
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
    Cert {
        /// Overwrite an existing certificate and key.
        #[arg(long)]
        force: bool,
    },
    /// Audit the artifacts lanpull created on this host.
    Audit,
    /// Summarize the audit log.
    Report(misc::ReportArgs),
    /// Remove runtime leftovers.
    Clean(misc::CleanArgs),
}

impl Command {
    /// The canonical dotted path of this command, used in the JSON envelope.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Init(_) => "init",
            Self::Serve => "serve",
            Self::Status => "status",
            Self::Account { command } => command.path(),
            Self::Access { command } => command.path(),
            Self::Share { command } => command.path(),
            Self::Service { command } => command.path(),
            Self::Config { command } => command.path(),
            Self::Cert { .. } => "cert",
            Self::Audit => "audit",
            Self::Report(_) => "report",
            Self::Clean(_) => "clean",
        }
    }
}
