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

use crate::cli::output::format_rows;
use crate::cli::Outcome;
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
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// List accounts with their IPs and effective access rules.
    List,
    /// Rotate one account password and refresh its staged auth file.
    Passwd {
        /// Account name.
        name: String,
        /// Rebuild the staging folder with this mirror base if it is missing.
        #[arg(long)]
        output: Option<PathBuf>,
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

impl AccountCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Add(_) => "account add",
            Self::Remove { .. } => "account remove",
            Self::List => "account list",
            Self::Passwd { .. } => "account passwd",
            Self::Export(_) => "account export",
            Self::Arm(_) => "account arm",
            Self::Disarm { .. } => "account disarm",
        }
    }
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
    /// Show what would change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
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
    pub r#move: bool,
    /// Overwrite an existing destination.
    #[arg(long)]
    pub force: bool,
    /// Show what would change without copying anything.
    #[arg(long)]
    pub dry_run: bool,
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
pub fn run(config_path: &Path, command: AccountCommand) -> Result<Outcome> {
    match command {
        AccountCommand::Add(args) => add(config_path, args),
        AccountCommand::Remove { name, yes, dry_run } => remove(config_path, &name, yes, dry_run),
        AccountCommand::List => list(config_path),
        AccountCommand::Passwd { name, output } => passwd(config_path, &name, output.as_deref()),
        AccountCommand::Export(args) => export(config_path, args),
        AccountCommand::Arm(args) => arm(config_path, args),
        AccountCommand::Disarm { name } => disarm(config_path, &name),
    }
}

/// Create an account and stage a client folder.
fn add(config_path: &Path, args: AddArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    if args.dry_run {
        let accounts = Clients::load(&config.clients_path)?;
        if accounts.get(&args.name).is_some() {
            return Err(Error::Account(format!(
                "account {} already exists",
                args.name
            )));
        }
        let staged = config.state_dir.join("client-ready").join(&args.name);
        return Ok(Outcome::new()
            .line(format!(
                "would create account {} (staging {})",
                args.name,
                staged.display()
            ))
            .with_data(&serde_json::json!({
                "account": args.name,
                "staging": staged,
                "dry_run": true,
            })));
    }
    let created =
        lanpull_core::account::create(&config, &args.name, args.ip, &args.output, args.local)?;
    let mut outcome = Outcome::new().line(format!(
        "created account {} (staging {})",
        args.name,
        created.staging.display()
    ));
    report_regeneration(&config, &mut outcome)?;
    outcome.data = serde_json::json!({
        "account": args.name,
        "staging": created.staging,
        "dry_run": false,
    });
    Ok(outcome)
}

/// Revoke an account and drop its policy deltas.
fn remove(config_path: &Path, name: &str, yes: bool, dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut accounts = Clients::load(&config.clients_path)?;
    if accounts.get(name).is_none() {
        return Err(Error::Account(format!("no such account: {name}")));
    }
    if dry_run {
        return Ok(Outcome::new()
            .line(format!("would remove account {name}"))
            .with_data(&serde_json::json!({ "account": name, "dry_run": true })));
    }
    crate::confirm::require(yes, &format!("remove account {name}"))?;
    accounts.remove(name);
    accounts.save(&config.clients_path)?;

    let mut policy = policy::Policy::load(&config.access_path)?;
    policy.remove_account(name);
    policy.save(&config.access_path)?;

    let dir = config.access_dir().join(name);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir)?;
    }
    let mut outcome = Outcome::new().line(format!("removed account {name}"));
    report_regeneration(&config, &mut outcome)?;
    outcome.data = serde_json::json!({ "account": name, "dry_run": false });
    Ok(outcome)
}

/// List accounts with their IPs and effective access rules.
fn list(config_path: &Path) -> Result<Outcome> {
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

    let lines = if rows.is_empty() {
        vec!["no accounts".to_string()]
    } else {
        format_rows(
            &rows
                .iter()
                .map(|row| {
                    let scope = if row.local { " local" } else { "" };
                    let ip = row.allowed_ip.as_deref().unwrap_or("any");
                    vec![
                        row.name.clone(),
                        ip.to_string(),
                        scope.to_string(),
                        format!("[{}]", row.rules.join(", ")),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    };
    Ok(Outcome::text(lines).with_data(&rows))
}

/// Rotate an account password and refresh its staged auth file.
///
/// When the staging folder is missing and `--output <dir>` is given, the whole
/// folder is rebuilt from the current configuration; otherwise the rotated
/// password is only written when the folder already exists.
fn passwd(config_path: &Path, name: &str, output: Option<&Path>) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut accounts = Clients::load(&config.clients_path)?;
    let password = clients::generate_password();
    let hash = clients::hash_password(&password)?;
    if !accounts.set_hash(name, hash) {
        return Err(Error::Account(format!("no such account: {name}")));
    }
    accounts.save(&config.clients_path)?;

    let staging = config.state_dir.join("client-ready").join(name);
    let staged = staging.join("auth");
    if staged.is_file() {
        lanpull_core::atomic::write_private(&staged, format!("{name}:{password}\n").as_bytes())?;
        lanpull_core::account::refresh_staging(&config, name)?;
    } else if let Some(output) = output {
        let auth = format!("{name}:{password}\n");
        lanpull_core::account::restage(&config, name, output, &auth)?;
    }
    let staged_present = staged.is_file();
    let mut outcome = Outcome::new().line(format!("password for {name}: {password}"));
    if !staged_present {
        outcome = outcome.warn(format!(
            "no staged folder for {name}; pass --output <dir> to rebuild it"
        ));
    }
    outcome.data = serde_json::json!({
        "account": name,
        "password": password,
        "staged": if staged_present { Some(staged) } else { None },
    });
    Ok(outcome)
}

/// Export a staged client folder.
fn export(config_path: &Path, args: ExportArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    if args.dry_run {
        let accounts = Clients::load(&config.clients_path)?;
        if accounts.get(&args.name).is_none() {
            return Err(Error::Account(format!("no such account: {}", args.name)));
        }
        let target = if args.to.is_dir() {
            args.to.join(&args.name)
        } else {
            args.to.clone()
        };
        return Ok(Outcome::new()
            .line(format!(
                "would export {} to {}",
                args.name,
                target.display()
            ))
            .with_data(&serde_json::json!({
                "account": args.name,
                "destination": target,
                "dry_run": true,
            })));
    }
    let exported =
        lanpull_core::account::export(&config, &args.name, &args.to, args.r#move, args.force)?;
    let mut outcome = Outcome::new().line(format!(
        "exported {} to {}",
        args.name,
        exported.destination.display()
    ));
    if exported.cross_device {
        outcome = outcome.warn(
            "destination is on another filesystem; auth and server.crt are now on that device",
        );
    }
    outcome.data = serde_json::json!({
        "account": args.name,
        "destination": exported.destination,
        "cross_device": exported.cross_device,
        "dry_run": false,
    });
    Ok(outcome)
}

/// Arm one account or all accounts.
fn arm(config_path: &Path, args: ArmArgs) -> Result<Outcome> {
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
            .ok_or_else(|| Error::Usage("provide a name or --all".to_string()))?;
        if accounts.get(&name).is_none() {
            return Err(Error::Account(format!("no such account: {name}")));
        }
        state.arm(&name, expires);
    }
    state.save(&config.arm_path())?;

    let armed: Vec<serde_json::Value> = state
        .armed_entries(now)
        .into_iter()
        .map(|(account, remaining)| serde_json::json!({ "account": account, "remaining_secs": remaining }))
        .collect();
    let mut outcome = Outcome::new();
    for (account, remaining) in state.armed_entries(now) {
        outcome = outcome.line(format!(
            "armed {account} for {}",
            status::format_duration(remaining)
        ));
    }
    outcome.data = serde_json::json!({ "accounts": armed, "ttl": args.ttl });
    Ok(outcome)
}

/// Clear an arm window.
fn disarm(config_path: &Path, name: &str) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut state = ArmState::load(&config.arm_path())?;
    let was_armed = state.disarm(name);
    if was_armed {
        state.save(&config.arm_path())?;
    }
    let line = if was_armed {
        format!("disarmed {name}")
    } else {
        format!("{name} was not armed")
    };
    Ok(Outcome::new()
        .line(line)
        .with_data(&serde_json::json!({ "account": name, "was_armed": was_armed })))
}
