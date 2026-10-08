//! The `lanpull` command-line interface.
//!
//! A thin shell over the `lanpull` crates. Diagnostics go to stderr and command
//! data to stdout; with `--json` stdout carries one envelope document instead
//! of text. The process exit code follows the contract in
//! `lanpull_core::error::exit`. The binary owns its standard streams directly,
//! so the workspace-wide `print_stdout`/`print_stderr` denies are relaxed here.
#![allow(clippy::print_stdout, clippy::print_stderr)]
// A binary has no external consumers, so "unreachable pub" is meaningless here.
#![allow(unreachable_pub)]
// clap derive structs are consumed by value; that is idiomatic for clap.
#![allow(clippy::needless_pass_by_value)]
// The CLI's job includes spawning `systemctl` and `ip`; the server crates never
// spawn, and `clippy.toml`'s `disallowed_methods` still guards them.
#![allow(clippy::disallowed_methods)]

mod cli;
mod config_edit;
mod confirm;
mod init;
mod manifest_helpers;
mod verbosity;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use lanpull_core::config as core_config;
use lanpull_core::error::{Error, Result};

use crate::cli::{emit, emit_error, Cli, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let parsed = parse_cli();
    let json = parsed.json;
    let command = parsed.command.path();
    if let Err(e) = init_tracing(parsed.verbose, parsed.quiet) {
        emit_error(command, &e, json);
        return ExitCode::from(e.exit_code());
    }
    match run(parsed).await {
        Ok(()) => ExitCode::from(lanpull_core::error::exit::OK),
        Err(e) => {
            emit_error(command, &e, json);
            ExitCode::from(e.exit_code())
        }
    }
}

/// Parse the command line, rendering clap errors through the output layer.
///
/// `--help`/`--version` are printed as usual and exit `0`; any other parse
/// failure becomes a usage error, wrapped in the JSON envelope when `--json`
/// was requested (clap has not parsed it yet, so the raw arguments are
/// scanned).
fn parse_cli() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let _ = e.print();
                std::process::exit(0);
            }
            let json = std::env::args_os().any(|arg| arg == "--json");
            // clap's message already begins with "error: "; drop it so the
            // output layer adds exactly one prefix.
            let rendered = e.to_string();
            let message = rendered.strip_prefix("error: ").unwrap_or(&rendered);
            let error = Error::Usage(message.trim_end().to_string());
            emit_error("", &error, json);
            std::process::exit(i32::from(lanpull_core::error::exit::USAGE));
        }
    }
}

/// Install a tracing subscriber: diagnostics to stderr, data to stdout.
fn init_tracing(verbose: u8, quiet: bool) -> Result<()> {
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
        .with_writer(std::io::stderr)
        .with_env_filter(filter);
    tracing::subscriber::set_global_default(subscriber.finish())
        .map_err(|e| Error::Server(format!("could not install the logger: {e}")))
}

/// Dispatch a parsed command.
async fn run(cli: Cli) -> Result<()> {
    let json = cli.json;
    let command = cli.command.path();
    verbosity::set(cli.verbose > 0);

    // `init` creates the configuration; every other command needs it. `init`
    // resolves the target the same way as the rest (`--config`, then
    // `$LANPULL_CONFIG`, then the canonical path), but never requires the file
    // to exist yet.
    if let Command::Init(args) = &cli.command {
        let config_path = resolve_init_path(cli.config.clone());
        return emit("init", init::run(&config_path, args)?, json);
    }

    let config_path = core_config::resolve_path(cli.config.as_deref())?;
    match cli.command {
        Command::Init(_) => unreachable!("handled above"),
        Command::Serve => {
            let config = lanpull_core::config::Config::load(&config_path)?;
            lanpull_http::http::serve(config).await
        }
        Command::Status(args) => emit(command, cli::misc::status(&config_path, args)?, json),
        Command::Account { command: sub } => {
            emit(command, cli::account::run(&config_path, sub)?, json)
        }
        Command::Access { command: sub } => {
            emit(command, cli::access::run(&config_path, sub)?, json)
        }
        Command::Share { command: sub } => emit(command, cli::share::run(&config_path, sub)?, json),
        Command::Service { command: sub } => emit(command, cli::service::run(sub)?, json),
        Command::Config { command: sub } => {
            emit(command, cli::config::run(&config_path, sub)?, json)
        }
        Command::Network { command: sub } => {
            emit(command, cli::network::run(&config_path, sub)?, json)
        }
        Command::Cert { force } => emit(command, cert(&config_path, force)?, json),
        Command::Audit => emit(command, cli::misc::audit(&config_path)?, json),
        Command::Report(args) => emit(command, cli::misc::report(&config_path, args)?, json),
        Command::Clean(args) => emit(command, cli::misc::clean(&config_path, args)?, json),
    }
}

/// Resolve the target path for `init`: explicit flag, then `$LANPULL_CONFIG`,
/// then the canonical `/etc/lanpull/lanpull.conf`. Unlike `resolve_path`, the
/// file need not exist yet.
fn resolve_init_path(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os(core_config::CONFIG_ENV).map(Into::into))
        .unwrap_or_else(|| PathBuf::from(core_config::DEFAULT_CONFIG_PATH))
}

/// Generate the TLS certificate.
fn cert(config_path: &std::path::Path, force: bool) -> Result<cli::Outcome> {
    let config = lanpull_core::config::Config::load(config_path)?;
    if !force && (config.cert_path.exists() || config.key_path.exists()) {
        return Err(Error::Usage(format!(
            "certificate already exists at {}; pass --force to regenerate",
            config.cert_path.display()
        )));
    }
    let (cert_path, key_path) =
        lanpull_core::cert::generate(&config.cert_path, &config.key_path, config.server_ip)?;
    Ok(cli::Outcome::new()
        .line(format!("certificate written to {}", cert_path.display()))
        .line(format!("key written to {} (mode 600)", key_path.display()))
        .line("copy server.crt to every client")
        .with_data(&serde_json::json!({
            "cert_path": cert_path,
            "key_path": key_path,
        })))
}
