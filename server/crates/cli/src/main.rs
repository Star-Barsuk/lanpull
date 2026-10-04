//! The `lanpull` command-line interface.
//!
//! A thin CLI over the `lanpull` library: `serve` runs the HTTPS server, the
//! remaining subcommands manage the manifest, the JSON access policy, accounts,
//! the arm window, the audit report, the certificate, and operator status.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use lanpull_core::access::Rule;
use lanpull_core::arm::ArmState;
use lanpull_core::clients::{self, Clients};
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::policy::{self, Policy};
use lanpull_core::timeutil;
use lanpull_core::{audit, cert};
use lanpull_http::http;
use lanpull_store::{manifest, status};

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
        /// The client's local mirror base directory; each share maps to a
        /// subdirectory of it.
        #[arg(long)]
        output: PathBuf,
        /// Loopback (self-share) account: exempt from the arm window.
        #[arg(long)]
        local: bool,
    },
    /// Revoke an account.
    RemoveClient {
        /// Account name.
        name: String,
    },
    /// List accounts, their IPs, and their effective access rules.
    ListClients,
    /// Manage the JSON access policy.
    Access {
        /// Policy operation.
        #[command(subcommand)]
        command: AccessCommand,
    },
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

/// Access-policy operations.
#[derive(Debug, Subcommand)]
enum AccessCommand {
    /// Manage the files visible to every account.
    Public {
        /// Public-set operation.
        #[command(subcommand)]
        command: PublicCommand,
    },
    /// Manage per-client additions and removals.
    Client {
        /// Per-client operation.
        #[command(subcommand)]
        command: ClientCommand,
    },
    /// Import an old flat access file into the policy.
    Import {
        /// Path to the old `lanpull.access` file.
        #[arg(long)]
        from: PathBuf,
    },
    /// Check the policy for unknown shares, missing files, and empty accounts.
    Doctor,
    /// Regenerate the per-account manifests after a manual edit.
    Apply,
}

/// Public-set operations.
#[derive(Debug, Subcommand)]
enum PublicCommand {
    /// Add files to the public set.
    Add {
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
    },
    /// Remove files from the public set.
    Remove {
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
    },
    /// List the public set.
    List {
        /// Only show this share.
        #[arg(long)]
        share: Option<String>,
    },
}

/// Per-client operations.
#[derive(Debug, Subcommand)]
enum ClientCommand {
    /// Add personal files for a client.
    Add {
        /// Account name.
        name: String,
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
    },
    /// Remove files (including public ones) for one client.
    Remove {
        /// Account name.
        name: String,
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
    },
    /// Show the effective set for one or every client.
    List {
        /// Account name (default: every account).
        name: Option<String>,
    },
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
        Command::AddClient {
            name,
            ip,
            output,
            local,
        } => add_client(&cli.config, &name, ip, &output, local),
        Command::RemoveClient { name } => remove_client(&cli.config, &name),
        Command::ListClients => list_clients(&cli.config),
        Command::Access { command } => access_command(&cli.config, command),
        Command::Passwd { name } => passwd(&cli.config, &name),
        Command::Arm { name, all, ttl } => arm(&cli.config, name.as_deref(), all, &ttl),
        Command::Disarm { name } => disarm(&cli.config, &name),
        Command::Report { user, since } => report(&cli.config, user.as_deref(), since.as_deref()),
        Command::Cert => cert_command(&cli.config),
        Command::Status => status_command(&cli.config),
    }
}

/// Regenerate the per-share and per-account manifests.
fn manifest_command(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    report_regeneration(&config)
}

/// Regenerate manifests and log a one-line summary.
fn report_regeneration(config: &Config) -> Result<()> {
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

/// Create an account and stage a client folder.
///
/// Delegates to [`lanpull_core::account::create`], whose pre-checks run before the
/// accounts file changes and whose rollback keeps a failed request from
/// leaving a half-created account.
fn add_client(
    config_path: &Path,
    name: &str,
    ip: Option<IpAddr>,
    output: &Path,
    local: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;
    let created = lanpull_core::account::create(&config, name, ip, output, local)?;
    report_regeneration(&config)?;

    tracing::info!("account {name} created");
    tracing::info!("client folder staged at {}", created.staging.display());
    tracing::info!("export it with 'lanpull account export {name} --to <dir>'");
    Ok(())
}

/// Dispatch an access-policy operation.
fn access_command(config_path: &Path, command: AccessCommand) -> Result<()> {
    match command {
        AccessCommand::Public { command } => match command {
            PublicCommand::Add { rules } => access_public_add(config_path, &rules),
            PublicCommand::Remove { rules } => access_public_remove(config_path, &rules),
            PublicCommand::List { share } => access_public_list(config_path, share.as_deref()),
        },
        AccessCommand::Client { command } => match command {
            ClientCommand::Add { name, rules } => access_client_add(config_path, &name, &rules),
            ClientCommand::Remove { name, rules } => {
                access_client_remove(config_path, &name, &rules)
            }
            ClientCommand::List { name } => access_client_list(config_path, name.as_deref()),
        },
        AccessCommand::Import { from } => access_import(config_path, &from),
        AccessCommand::Doctor => access_doctor(config_path),
        AccessCommand::Apply => access_apply(config_path),
    }
}

/// Refuse a rule that names a share the server does not serve.
fn ensure_share(config: &Config, share: &str) -> Result<()> {
    if config.shares.contains_key(share) {
        Ok(())
    } else {
        Err(Error::Config(format!("unknown share {share}")))
    }
}

/// Add rules to the public set.
fn access_public_add(config_path: &Path, specs: &[String]) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.public.insert(path);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config)?;
    tracing::info!("public set updated");
    Ok(())
}

/// Remove rules from the public set.
fn access_public_remove(config_path: &Path, specs: &[String]) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.public.remove(&path);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config)?;
    tracing::info!("public set updated");
    Ok(())
}

/// List the public set.
fn access_public_list(config_path: &Path, share: Option<&str>) -> Result<()> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;
    let mut any = false;
    for (name, policy) in &policy.shares {
        if share.is_some_and(|filter| filter != name) {
            continue;
        }
        for path in &policy.public {
            tracing::info!("{name}:{path}");
            any = true;
        }
    }
    if !any {
        tracing::info!("public set is empty");
    }
    Ok(())
}

/// Add personal files for one client.
fn access_client_add(config_path: &Path, name: &str, specs: &[String]) -> Result<()> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    if accounts.get(name).is_none() {
        tracing::warn!("account {name} does not exist yet; the rule applies once it is created");
    }
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.add_for(name, path);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config)?;
    tracing::info!("granted {name} {}", specs.join(", "));
    Ok(())
}

/// Remove files (including public ones) for one client.
fn access_client_remove(config_path: &Path, name: &str, specs: &[String]) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.remove_for(name, &path);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config)?;
    tracing::info!("revoked {name} {}", specs.join(", "));
    Ok(())
}

/// Show the effective set for one or every client.
fn access_client_list(config_path: &Path, name: Option<&str>) -> Result<()> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;
    let accounts = Clients::load(&config.clients_path)?;

    let names: Vec<String> = if let Some(name) = name {
        vec![name.to_string()]
    } else {
        let mut names: Vec<String> = accounts.iter().map(|a| a.name.clone()).collect();
        for policy in policy.shares.values() {
            for account in policy.clients.keys() {
                if !names.iter().any(|known| known == account) {
                    names.push(account.clone());
                }
            }
        }
        names.sort();
        names.dedup();
        names
    };

    if names.is_empty() {
        tracing::info!("no accounts");
        return Ok(());
    }

    for account in names {
        let mut any = false;
        for (share, policy) in &policy.shares {
            for path in policy.effective(&account) {
                tracing::info!("{account} {share}:{path}");
                any = true;
            }
        }
        if !any {
            tracing::info!("{account} (nothing)");
        }
    }
    Ok(())
}

/// Import an old flat access file into the policy.
fn access_import(config_path: &Path, from: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let text = std::fs::read_to_string(from)?;
    let mut policy = Policy::load(&config.access_path)?;
    let mut imported = 0_usize;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(account), Some(spec)) = (fields.next(), fields.next()) else {
            continue;
        };
        let rule = Rule::parse(spec)?;
        let shares: Vec<String> = match &rule.share {
            Some(name) => vec![name.clone()],
            None => config.shares.keys().cloned().collect(),
        };
        let path = rule
            .glob
            .as_ref()
            .map_or_else(|| "**".to_string(), |glob| glob.as_str().to_string());
        for share in shares {
            ensure_share(&config, &share)?;
            policy.share_mut(&share)?.add_for(account, path.clone());
        }
        imported = imported.saturating_add(1);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config)?;
    tracing::info!(
        "imported {imported} rules into {}",
        config.access_path.display()
    );
    Ok(())
}

/// Check the policy for unknown shares, missing files, and empty accounts.
fn access_doctor(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;
    let clients = Clients::load(&config.clients_path)?;
    let mut warnings: Vec<String> = Vec::new();

    for share in policy.shares.keys() {
        if !config.shares.contains_key(share) {
            warnings.push(format!("policy references unknown share {share}"));
        }
    }

    for (share, policy) in &policy.shares {
        let Some(root) = config.shares.get(share) else {
            continue;
        };
        for path in &policy.public {
            check_exists(root, share, path, &mut warnings);
        }
        for (account, deltas) in &policy.clients {
            if clients.get(account).is_none() {
                warnings.push(format!("policy references unknown account {account}"));
            }
            for path in &deltas.add {
                check_exists(root, share, path, &mut warnings);
            }
            for path in &deltas.remove {
                if !policy.public.contains(path) {
                    warnings.push(format!(
                        "{account}: removes {share}:{path} which is not in the public set"
                    ));
                }
            }
        }
    }

    let access = policy.expand(&clients)?;
    for account in clients.iter() {
        if !access.has_rules(&account.name) {
            warnings.push(format!("account {} can pull nothing", account.name));
        }
    }

    if warnings.is_empty() {
        tracing::info!("access policy looks consistent");
    } else {
        for warning in &warnings {
            tracing::warn!("{warning}");
        }
    }
    Ok(())
}

/// Check that a listed path exists under the share.
fn check_exists(root: &Path, share: &str, path: &str, warnings: &mut Vec<String>) {
    if path.contains('*') {
        return;
    }
    match policy::resolve(root, path) {
        Some(full) if full.is_file() => {}
        _ => warnings.push(format!("{share}: listed path does not exist: {path}")),
    }
}

/// Regenerate manifests after a manual policy edit.
fn access_apply(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    report_regeneration(&config)
}

/// Revoke an account and drop its policy deltas.
fn remove_client(config_path: &Path, name: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut accounts = Clients::load(&config.clients_path)?;
    if !accounts.remove(name) {
        return Err(Error::Account(format!("no such account: {name}")));
    }
    accounts.save(&config.clients_path)?;

    let mut policy = Policy::load(&config.access_path)?;
    policy.remove_account(name);
    policy.save(&config.access_path)?;

    let dir = config.access_dir().join(name);
    if dir.is_dir() {
        std::fs::remove_dir_all(dir)?;
    }
    report_regeneration(&config)?;

    tracing::info!("account {name} removed");
    Ok(())
}

/// List accounts with their IPs and effective access rules.
fn list_clients(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    if accounts.is_empty() {
        tracing::info!("no accounts");
        return Ok(());
    }
    let access = policy::load_access(&config)?;
    for account in accounts.iter() {
        let ip = account
            .allowed_ip
            .map_or_else(|| "any".to_string(), |ip| ip.to_string());
        let scope = if account.local { " local" } else { "" };
        let rules: Vec<String> = access.rules(&account.name).iter().map(Rule::spec).collect();
        tracing::info!("{} {}{} [{}]", account.name, ip, scope, rules.join(", "));
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
        lanpull_core::atomic::write_private(&staged, auth.as_bytes())?;
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
            if !account.local {
                state.arm(&account.name, expires);
            }
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
