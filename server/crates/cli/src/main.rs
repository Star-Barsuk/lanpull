//! The `lanpull` command-line interface.
//!
//! A thin shell over the `lanpull` crates. Diagnostics go to stderr and command
//! data to stdout; the process exit code follows the contract in
//! `lanpull_core::error::exit`. The binary owns its standard streams directly,
//! so the workspace-wide `print_stdout`/`print_stderr` denies are relaxed here.
#![allow(clippy::print_stdout, clippy::print_stderr)]
// A binary has no external consumers, so "unreachable pub" is meaningless here.
#![allow(unreachable_pub)]
// clap derive structs are consumed by value; that is idiomatic for clap.
#![allow(clippy::needless_pass_by_value)]
// The CLI's job includes spawning systemctl and `ip`; the server crates never
// spawn, and `clippy.toml`'s `disallowed_methods` still guards them.
#![allow(clippy::disallowed_methods)]

mod cli;
mod config_edit;
mod confirm;
mod init;
mod manifest_helpers;

use std::process::ExitCode;

use clap::Parser;
use lanpull_core::config as core_config;
use lanpull_core::error::Result;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let parsed = Cli::parse();
    if let Err(e) = init_tracing(parsed.verbose, parsed.quiet) {
        eprintln!("error: {e}");
        return ExitCode::from(lanpull_core::error::exit::FAILURE);
    }
    match run(parsed).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            if let Some(hint) = e.hint() {
                eprintln!("hint: {hint}");
            }
            ExitCode::from(e.exit_code())
        }
    }
}

/// Install a tracing subscriber: diagnostics to stderr, data to stdout.
fn init_tracing(verbose: u8, quiet: bool) -> std::result::Result<(), String> {
    use tracing_subscriber::EnvFilter;
    let filter = if quiet || verbose > 0 {
        let level = match (quiet, verbose) {
            (true, _) => "error",
            (_, 1) => "debug",
            (_, _) => "trace",
        };
        EnvFilter::new(level)
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };
    let subscriber = tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .with_env_filter(filter);
    tracing::subscriber::set_global_default(subscriber.finish())
        .map_err(|e| format!("could not install the logger: {e}"))
}

/// Dispatch a parsed command.
async fn run(cli: Cli) -> Result<()> {
    // `init` creates the configuration; every other command needs it.
    if let Command::Init(args) = &cli.command {
        let config_path = cli
            .config
            .clone()
            .or_else(|| std::env::var_os(core_config::CONFIG_ENV).map(Into::into))
            .unwrap_or_else(|| std::path::PathBuf::from(core_config::DEFAULT_CONFIG_PATH));
        return init::run(&config_path, args);
    }

    let config_path = core_config::resolve_path(cli.config.as_deref())?;
    match cli.command {
        Command::Init(_) => unreachable!("handled above"),
        Command::Serve => {
            let config = lanpull_core::config::Config::load(&config_path)?;
            lanpull_http::http::serve(config).await
        }
        Command::Status(args) => cli::misc::status(&config_path, args),
        Command::Account { command } => cli::account::run(&config_path, command),
        Command::Access { command } => cli::access::run(&config_path, command),
        Command::Share { command } => cli::share::run(&config_path, command),
        Command::Service { command } => cli::service::run(command),
        Command::Config { command } => cli::config::run(&config_path, command),
        Command::Cert => cert(&config_path),
        Command::Audit(args) => cli::misc::audit(&config_path, args),
        Command::Report(args) => cli::misc::report(&config_path, args),
        Command::Clean(args) => cli::misc::clean(&config_path, args),
    }
}

/// Generate the TLS certificate.
fn cert(config_path: &std::path::Path) -> Result<()> {
    let config = lanpull_core::config::Config::load(config_path)?;
    let (cert_path, key_path) = lanpull_core::cert::generate(&config.state_dir, config.server_ip)?;
    tracing::info!("certificate written to {}", cert_path.display());
    tracing::info!("key written to {} (mode 600)", key_path.display());
    tracing::info!("copy server.crt to every client");
    Ok(())
}
