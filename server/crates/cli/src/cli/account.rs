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
    /// Authorize one or more accounts (or all with `--all`) for a short window.
    Arm(ArmArgs),
    /// Clear one or more accounts' arm windows (or all with `--all`).
    Disarm(DisarmArgs),
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
            Self::Disarm(_) => "account disarm",
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
    /// Account names (omit with `--all`).
    pub names: Vec<String>,
    /// Arm every non-local account.
    #[arg(long, conflicts_with = "names")]
    pub all: bool,
    /// Window length, for example `15m` or `30m`.
    #[arg(long, default_value = "15m")]
    pub ttl: String,
}

/// Arguments for `account disarm`.
#[derive(Debug, clap::Args)]
pub struct DisarmArgs {
    /// Account names (omit with `--all`).
    pub names: Vec<String>,
    /// Clear every account's arm window.
    #[arg(long, conflicts_with = "names")]
    pub all: bool,
    /// Do not ask for confirmation.
    #[arg(long)]
    pub yes: bool,
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
        AccountCommand::Disarm(args) => disarm(config_path, args),
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
    let mut arm = ArmState::load(&config.arm_path())?;
    let was_armed = arm.disarm(name);
    if was_armed {
        arm.save(&config.arm_path())?;
    }
    let mut outcome = Outcome::new().line(format!("removed account {name}"));
    if was_armed {
        outcome = outcome.warn(format!("{name} was armed; its arm window was cleared"));
    }
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
        let mut table = vec![vec![
            "NAME".to_string(),
            "IP".to_string(),
            "SCOPE".to_string(),
            "RULES".to_string(),
        ]];
        for row in &rows {
            table.push(vec![
                row.name.clone(),
                row.allowed_ip.as_deref().unwrap_or("any").to_string(),
                if row.local {
                    "local".to_string()
                } else {
                    String::new()
                },
                format!("[{}]", row.rules.join(", ")),
            ]);
        }
        format_rows(&table)
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
    // The password is a secret: it goes to stderr (diagnostics) so it never
    // lands in stdout data or the JSON envelope.
    eprintln!("password for {name}: {password}");
    let mut outcome = Outcome::new().line(if staged_present {
        format!("rotated password for {name} (auth file updated)")
    } else {
        format!("rotated password for {name}")
    });
    if !staged_present {
        outcome = outcome.warn(format!(
            "no staged folder for {name}; pass --output <dir> to rebuild it"
        ));
    }
    outcome.data = serde_json::json!({
        "account": name,
        "staged": staged_present,
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

/// Arm one or more accounts, or every non-local account.
fn arm(config_path: &Path, args: ArmArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    let seconds = timeutil::parse_duration_secs(&args.ttl)?;
    let now = timeutil::now_unix();
    let expires = now.saturating_add(seconds);

    let targets: Vec<String> = if args.all {
        if accounts.is_empty() {
            return Err(Error::Account("no accounts to arm".to_string()));
        }
        accounts
            .iter()
            .filter(|account| !account.local)
            .map(|account| account.name.clone())
            .collect()
    } else {
        if args.names.is_empty() {
            return Err(Error::Usage(
                "provide one or more names or --all".to_string(),
            ));
        }
        ensure_known(&accounts, &args.names)?;
        args.names.clone()
    };

    let mut state = ArmState::load(&config.arm_path())?;
    let mut outcome = Outcome::new();
    let mut data = Vec::new();
    for name in &targets {
        if accounts.get(name).is_some_and(|account| account.local) {
            outcome = outcome.warn(format!("local account {name} needs no arming; skipped"));
            continue;
        }
        state.arm(name, expires);
        outcome = outcome.line(format!(
            "armed {name} for {}",
            status::format_duration(seconds)
        ));
        data.push(serde_json::json!({
            "account": name,
            "expires_at": timeutil::iso8601(expires),
            "remaining_secs": seconds,
        }));
    }
    state.save(&config.arm_path())?;
    outcome.data = serde_json::json!({ "accounts": data, "ttl": args.ttl });
    Ok(outcome)
}

/// Clear one or more accounts' arm windows, or every entry with `--all`.
fn disarm(config_path: &Path, args: DisarmArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    let mut state = ArmState::load(&config.arm_path())?;

    if args.all {
        if state.armed.is_empty() {
            return Ok(Outcome::new()
                .line("no accounts were armed")
                .with_data(&serde_json::json!({ "accounts": [], "all": true })));
        }
        crate::confirm::require(args.yes, "disarm all accounts")?;
        let names: Vec<String> = state.armed.keys().cloned().collect();
        for name in &names {
            state.disarm(name);
        }
        state.save(&config.arm_path())?;
        let mut outcome = Outcome::new();
        for name in &names {
            outcome = outcome.line(format!("disarmed {name}"));
        }
        let data: Vec<serde_json::Value> = names
            .iter()
            .map(|name| serde_json::json!({ "account": name, "was_armed": true }))
            .collect();
        outcome.data = serde_json::json!({ "accounts": data, "all": true });
        return Ok(outcome);
    }

    if args.names.is_empty() {
        return Err(Error::Usage(
            "provide one or more names or --all".to_string(),
        ));
    }
    ensure_known(&accounts, &args.names)?;

    let mut outcome = Outcome::new();
    let mut data = Vec::new();
    let mut changed = false;
    for name in &args.names {
        let was_armed = state.disarm(name);
        changed |= was_armed;
        let line = if was_armed {
            format!("disarmed {name}")
        } else {
            format!("{name} was not armed")
        };
        outcome = outcome.line(line);
        data.push(serde_json::json!({ "account": name, "was_armed": was_armed }));
    }
    if changed {
        state.save(&config.arm_path())?;
    }
    outcome.data = serde_json::json!({ "accounts": data, "all": false });
    Ok(outcome)
}

/// Reject the whole request when any named account does not exist.
///
/// The check runs before any mutation, so a batch is all-or-nothing.
fn ensure_known(accounts: &Clients, names: &[String]) -> Result<()> {
    let unknown: Vec<String> = names
        .iter()
        .filter(|name| accounts.get(name).is_none())
        .cloned()
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(Error::Account(format!(
            "no such account(s): {}",
            unknown.join(", ")
        )))
    }
}
