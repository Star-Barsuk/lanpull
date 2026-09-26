//! The `lanpull` command-line interface.
//!
//! A thin CLI over the `lanpull` library: `serve` runs the HTTPS server, the
//! remaining subcommands manage the manifest, accounts, the arm window, the
//! audit report, the certificate, and operator status.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use lanpull::arm::ArmState;
use lanpull::clients::{self, Account, Clients};
use lanpull::config::Config;
use lanpull::error::{Error, Result};
use lanpull::timeutil;
use lanpull::{audit, cert, http, manifest, status};

/// lanpull server: manual file distribution over a local network.
#[derive(Debug, Parser)]
#[command(name = "lanpull", version, about)]
struct Cli {
    /// Path to the server configuration file.
    #[arg(long, global = true, default_value = "config/lanpull.conf")]
    config: PathBuf,
    /// Subcommand to run.
    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the HTTPS server.
    Serve,
    /// Regenerate the data manifest.
    Manifest,
    /// Create an account and stage a ready client folder.
    AddClient {
        /// Account (machine) name.
        name: String,
        /// Optional source IP the account is bound to.
        #[arg(long)]
        ip: Option<IpAddr>,
        /// The client's local mirror directory, written to `OUTPUT`.
        #[arg(long)]
        output: PathBuf,
    },
    /// Revoke an account.
    RemoveClient {
        /// Account name.
        name: String,
    },
    /// List accounts.
    ListClients,
    /// Rotate one account password.
    Passwd {
        /// Account name.
        name: String,
    },
    /// Authorize an account for a short window.
    Arm {
        /// Account name (omit with `--all`).
        name: Option<String>,
        /// Arm every account.
        #[arg(long)]
        all: bool,
        /// Window length, for example `15m` or `30m`.
        #[arg(long, default_value = "15m")]
        ttl: String,
    },
    /// Clear an arm window.
    Disarm {
        /// Account name.
        name: String,
    },
    /// Summarize the audit log.
    Report {
        /// Only show one account.
        #[arg(long)]
        user: Option<String>,
        /// Only show requests newer than this duration, for example `7d`.
        #[arg(long)]
        since: Option<String>,
    },
    /// Generate the self-signed TLS certificate.
    Cert,
    /// Show manifest freshness, warnings, and armed accounts.
    Status,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Install a stdout tracing subscriber.
fn init_tracing() {
    let subscriber = tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .with_writer(std::io::stdout);
    let _ = tracing::subscriber::set_global_default(subscriber.finish());
}

/// Dispatch a parsed command.
async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Serve => {
            let config = Config::load(&cli.config)?;
            http::serve(config).await
        }
        Command::Manifest => manifest_command(&cli.config),
        Command::AddClient { name, ip, output } => add_client(&cli.config, &name, ip, &output),
        Command::RemoveClient { name } => remove_client(&cli.config, &name),
        Command::ListClients => list_clients(&cli.config),
        Command::Passwd { name } => passwd(&cli.config, &name),
        Command::Arm { name, all, ttl } => arm(&cli.config, name.as_deref(), all, &ttl),
        Command::Disarm { name } => disarm(&cli.config, &name),
        Command::Report { user, since } => report(&cli.config, user.as_deref(), since.as_deref()),
        Command::Cert => cert_command(&cli.config),
        Command::Status => status_command(&cli.config),
    }
}

/// Regenerate the manifest.
fn manifest_command(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let (manifest, warnings) = manifest::generate(
        &config.share_dir,
        &config.manifest_path(),
        &config.cache_path(),
    )?;
    let total: u64 = manifest
        .files
        .iter()
        .fold(0_u64, |acc, file| acc.saturating_add(file.size));
    tracing::info!(
        "manifest: {} files, {} bytes, generated_at {}",
        manifest.files.len(),
        total,
        manifest.generated_at
    );
    emit_warnings(&warnings);
    Ok(())
}

/// Print manifest warnings.
fn emit_warnings(warnings: &manifest::Warnings) {
    for path in &warnings.symlinks {
        tracing::warn!("symlink skipped: {path}");
    }
    for path in &warnings.reserved {
        tracing::warn!("reserved-prefix entry ignored: {path}");
    }
    for path in &warnings.modified_during_walk {
        tracing::warn!("file modified during walk: {path}");
    }
}

/// Create an account and stage a client folder.
fn add_client(config_path: &Path, name: &str, ip: Option<IpAddr>, output: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut accounts = Clients::load(&config.clients_path)?;
    if accounts.get(name).is_some() {
        return Err(Error::Account(format!("account {name} already exists")));
    }

    let password = clients::generate_password();
    let hash = clients::hash_password(&password)?;
    accounts.insert(Account {
        name: name.to_string(),
        hash,
        allowed_ip: ip,
    });
    accounts.save(&config.clients_path)?;

    let staging = config.state_dir.join("client-ready").join(name);
    std::fs::create_dir_all(&staging)?;

    let pull_source = config.bundle_dir().join("pull.py");
    if !pull_source.is_file() {
        return Err(Error::Config(
            "client bundle not staged; run make install or make client-bundle".to_string(),
        ));
    }
    std::fs::copy(&pull_source, staging.join("pull.py"))?;
    set_executable(&staging.join("pull.py"))?;

    let client_conf = format!(
        "SERVER_URL=https://{}:{}\nOUTPUT={}\n",
        config.server_ip,
        config.port,
        output.display()
    );
    std::fs::write(staging.join("lanpull.conf"), client_conf)?;

    let auth = format!("{name}:{password}\n");
    lanpull::atomic::write_private(&staging.join("auth"), auth.as_bytes())?;

    std::fs::copy(&config.cert_path, staging.join("server.crt"))?;

    tracing::info!("account {name} created");
    tracing::info!("client folder staged at {}", staging.display());
    tracing::info!("copy that folder to the machine, then run pull.py there");
    Ok(())
}

/// Mark a staged file executable.
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Revoke an account.
fn remove_client(config_path: &Path, name: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut accounts = Clients::load(&config.clients_path)?;
    if !accounts.remove(name) {
        return Err(Error::Account(format!("no such account: {name}")));
    }
    accounts.save(&config.clients_path)?;
    tracing::info!("account {name} removed");
    Ok(())
}

/// List accounts.
fn list_clients(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    if accounts.is_empty() {
        tracing::info!("no accounts");
        return Ok(());
    }
    for account in accounts.iter() {
        let ip = account
            .allowed_ip
            .map_or_else(|| "any".to_string(), |ip| ip.to_string());
        tracing::info!("{} {}", account.name, ip);
    }
    Ok(())
}

/// Rotate an account password and refresh its staged auth file.
fn passwd(config_path: &Path, name: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut accounts = Clients::load(&config.clients_path)?;
    let password = clients::generate_password();
    let hash = clients::hash_password(&password)?;
    if !accounts.set_hash(name, hash) {
        return Err(Error::Account(format!("no such account: {name}")));
    }
    accounts.save(&config.clients_path)?;

    let staged = config
        .state_dir
        .join("client-ready")
        .join(name)
        .join("auth");
    if staged.is_file() {
        let auth = format!("{name}:{password}\n");
        lanpull::atomic::write_private(&staged, auth.as_bytes())?;
        tracing::info!("staged auth updated at {}", staged.display());
    }

    tracing::info!("password for {name}: {password}");
    tracing::info!("copy the new auth file to that machine");
    Ok(())
}

/// Arm one account or all accounts.
fn arm(config_path: &Path, name: Option<&str>, all: bool, ttl: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    let seconds = timeutil::parse_duration_secs(ttl)?;
    let now = timeutil::now_unix();
    let expires = now.saturating_add(seconds);

    let mut state = ArmState::load(&config.arm_path())?;
    if all {
        if accounts.is_empty() {
            return Err(Error::Account("no accounts to arm".to_string()));
        }
        for account in accounts.iter() {
            state.arm(&account.name, expires);
        }
    } else {
        let name = name.ok_or_else(|| Error::Account("provide a name or --all".to_string()))?;
        if accounts.get(name).is_none() {
            return Err(Error::Account(format!("no such account: {name}")));
        }
        state.arm(name, expires);
    }
    state.save(&config.arm_path())?;

    for (account, remaining) in state.armed_entries(now) {
        tracing::info!("armed {account} for {}", status::format_duration(remaining));
    }
    Ok(())
}

/// Clear an arm window.
fn disarm(config_path: &Path, name: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut state = ArmState::load(&config.arm_path())?;
    if state.disarm(name) {
        state.save(&config.arm_path())?;
        tracing::info!("disarmed {name}");
    } else {
        tracing::info!("{name} was not armed");
    }
    Ok(())
}

/// Summarize the audit log.
fn report(config_path: &Path, user: Option<&str>, since: Option<&str>) -> Result<()> {
    let config = Config::load(config_path)?;
    let cutoff = match since {
        Some(value) => {
            let seconds = timeutil::parse_duration_secs(value)?;
            Some(timeutil::now_unix().saturating_sub(seconds))
        }
        None => None,
    };
    let summary = audit::summarize(&config.audit_log, user, cutoff)?;
    for line in summary.lines() {
        tracing::info!("{line}");
    }
    Ok(())
}

/// Generate the TLS certificate.
fn cert_command(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let (cert_path, key_path) = cert::generate(&config.state_dir, config.server_ip)?;
    tracing::info!("certificate written to {}", cert_path.display());
    tracing::info!("key written to {} (mode 600)", key_path.display());
    tracing::info!("copy server.crt to every client");
    Ok(())
}

/// Show operator status.
fn status_command(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    for line in status::run(&config)? {
        tracing::info!("{line}");
    }
    Ok(())
}
