//! `lanpull account` — accounts, passwords, arm window, and export.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use lanpull_core::access::Rule;
use lanpull_core::arm::ArmState;
use lanpull_core::clients::{self, Clients};
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::policy;
use lanpull_core::timeutil;
use lanpull_store::status;
use serde::Serialize;

use crate::cli::{print_json, JsonFlag};
use crate::manifest_helpers::report_regeneration;

/// Account operations.
#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Create an account and stage its client folder.
    Add(AddArgs),
    /// Revoke an account and drop its policy deltas.
    Remove {
        /// Account name.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// List accounts with their IPs and effective access rules.
    List(JsonFlag),
    /// Rotate one account password and refresh its staged auth file.
    Passwd {
        /// Account name.
        name: String,
    },
    /// Export a staged client folder for transfer to the client.
    Export(ExportArgs),
    /// Authorize one account (or all with `--all`) for a short window.
    Arm(ArmArgs),
    /// Clear an account's arm window.
    Disarm {
        /// Account name.
        name: String,
    },
}

/// Arguments for `account add`.
#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// Account (machine) name.
    pub name: String,
    /// Optional source IP the account is bound to.
    #[arg(long)]
    pub ip: Option<IpAddr>,
    /// The client's local mirror base directory; each share maps to a
    /// subdirectory of it.
    #[arg(long)]
    pub output: PathBuf,
    /// Loopback (self-share) account: exempt from the arm window.
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `account export`.
#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Account name.
    pub name: String,
    /// Destination directory (the folder is copied inside it) or exact path.
    #[arg(long, value_name = "DIR")]
    pub to: PathBuf,
    /// Move the staging folder instead of copying it.
    #[arg(long)]
    pub move_staging: bool,
    /// Overwrite an existing destination.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `account arm`.
#[derive(Debug, clap::Args)]
pub struct ArmArgs {
    /// Account name (omit with `--all`).
    pub name: Option<String>,
    /// Arm every account.
    #[arg(long)]
    pub all: bool,
    /// Window length, for example `15m` or `30m`.
    #[arg(long, default_value = "15m")]
    pub ttl: String,
}

/// One row of `account list` output.
#[derive(Debug, Serialize)]
struct AccountRow {
    name: String,
    allowed_ip: Option<String>,
    local: bool,
    rules: Vec<String>,
}

/// Dispatch an account operation.
pub fn run(config_path: &Path, command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::Add(args) => add(config_path, args),
        AccountCommand::Remove { name, yes } => remove(config_path, &name, yes),
        AccountCommand::List(json) => list(config_path, json),
        AccountCommand::Passwd { name } => passwd(config_path, &name),
        AccountCommand::Export(args) => export(config_path, args),
        AccountCommand::Arm(args) => arm(config_path, args),
        AccountCommand::Disarm { name } => disarm(config_path, &name),
    }
}

/// Create an account and stage a client folder.
fn add(config_path: &Path, args: AddArgs) -> Result<()> {
    let config = Config::load(config_path)?;
    let created =
        lanpull_core::account::create(&config, &args.name, args.ip, &args.output, args.local)?;
    report_regeneration(&config)?;

    tracing::info!("account {} created", args.name);
    tracing::info!("client folder staged at {}", created.staging.display());
    tracing::info!(
        "export it with 'lanpull account export {} --to <dir>'",
        args.name
    );
    Ok(())
}

/// Revoke an account and drop its policy deltas.
fn remove(config_path: &Path, name: &str, yes: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    crate::confirm::require(yes, &format!("remove account {name}"))?;

    let mut accounts = Clients::load(&config.clients_path)?;
    if !accounts.remove(name) {
        return Err(Error::Account(format!("no such account: {name}")));
    }
    accounts.save(&config.clients_path)?;

    let mut policy = policy::Policy::load(&config.access_path)?;
    policy.remove_account(name);
    policy.save(&config.access_path)?;

    let dir = config.access_dir().join(name);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir)?;
    }
    report_regeneration(&config)?;

    tracing::info!("account {name} removed");
    Ok(())
}

/// List accounts with their IPs and effective access rules.
fn list(config_path: &Path, json: JsonFlag) -> Result<()> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    let access = policy::load_access(&config)?;

    let rows: Vec<AccountRow> = accounts
        .iter()
        .map(|account| AccountRow {
            name: account.name.clone(),
            allowed_ip: account.allowed_ip.map(|ip| ip.to_string()),
            local: account.local,
            rules: access.rules(&account.name).iter().map(Rule::spec).collect(),
        })
        .collect();

    if json.json {
        return print_json(&rows);
    }
    if rows.is_empty() {
        println!("no accounts");
        return Ok(());
    }
    for row in rows {
        let scope = if row.local { " local" } else { "" };
        let ip = row.allowed_ip.as_deref().unwrap_or("any");
        println!("{} {}{} [{}]", row.name, ip, scope, row.rules.join(", "));
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

    println!("password for {name}: {password}");
    tracing::info!(
        "export it with 'lanpull account export {name} --to <dir>' and copy to that machine"
    );
    Ok(())
}

/// Export a staged client folder.
fn export(config_path: &Path, args: ExportArgs) -> Result<()> {
    let config = Config::load(config_path)?;
    let exported = lanpull_core::account::export(
        &config,
        &args.name,
        &args.to,
        args.move_staging,
        args.force,
    )?;
    if exported.cross_device {
        tracing::warn!(
            "destination is on another filesystem; auth and server.crt are now on that device"
        );
    }
    tracing::info!(
        "exported client folder to {}",
        exported.destination.display()
    );
    tracing::info!("copy it to the client machine, then run pull.py there");
    Ok(())
}

/// Arm one account or all accounts.
fn arm(config_path: &Path, args: ArmArgs) -> Result<()> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    let seconds = timeutil::parse_duration_secs(&args.ttl)?;
    let now = timeutil::now_unix();
    let expires = now.saturating_add(seconds);

    let mut state = ArmState::load(&config.arm_path())?;
    if args.all {
        if accounts.is_empty() {
            return Err(Error::Account("no accounts to arm".to_string()));
        }
        for account in accounts.iter() {
            if !account.local {
                state.arm(&account.name, expires);
            }
        }
    } else {
        let name = args
            .name
            .ok_or_else(|| Error::Account("provide a name or --all".to_string()))?;
        if accounts.get(&name).is_none() {
            return Err(Error::Account(format!("no such account: {name}")));
        }
        state.arm(&name, expires);
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
