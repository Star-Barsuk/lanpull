//! `lanpull access` — the JSON access policy.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use lanpull_core::access::Rule;
use lanpull_core::clients::Clients;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::policy::{self, Policy};
use serde::Serialize;

use crate::cli::print_json;
use crate::manifest_helpers::report_regeneration;

/// Access-policy operations.
#[derive(Debug, Subcommand)]
pub enum AccessCommand {
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
pub enum PublicCommand {
    /// Add files to the public set.
    Add {
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
    },
    /// Remove files from the public set.
    Remove {
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// List the public set.
    List(PublicListArgs),
}

/// Arguments for `access public list`.
#[derive(Debug, clap::Args)]
pub struct PublicListArgs {
    /// Only show this share.
    #[arg(long)]
    pub share: Option<String>,
    /// Print machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// Per-client operations.
#[derive(Debug, Subcommand)]
pub enum ClientCommand {
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
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Show the effective set for one or every client.
    List(ClientListArgs),
}

/// Arguments for `access client list`.
#[derive(Debug, clap::Args)]
pub struct ClientListArgs {
    /// Account name (default: every account).
    pub name: Option<String>,
    /// Print machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// One effective rule row, used for JSON output.
#[derive(Debug, Serialize)]
struct RuleRow {
    account: String,
    share: String,
    path: String,
}

/// Dispatch an access-policy operation.
pub fn run(config_path: &Path, command: AccessCommand) -> Result<()> {
    match command {
        AccessCommand::Public { command } => match command {
            PublicCommand::Add { rules } => public_add(config_path, &rules),
            PublicCommand::Remove { rules, yes } => public_remove(config_path, &rules, yes),
            PublicCommand::List(args) => public_list(config_path, args),
        },
        AccessCommand::Client { command } => match command {
            ClientCommand::Add { name, rules } => client_add(config_path, &name, &rules),
            ClientCommand::Remove { name, rules, yes } => {
                client_remove(config_path, &name, &rules, yes)
            }
            ClientCommand::List(args) => client_list(config_path, args),
        },
        AccessCommand::Import { from } => import(config_path, &from),
        AccessCommand::Doctor => doctor(config_path),
        AccessCommand::Apply => apply(config_path),
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
fn public_add(config_path: &Path, specs: &[String]) -> Result<()> {
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
fn public_remove(config_path: &Path, specs: &[String], yes: bool) -> Result<()> {
    crate::confirm::require(yes, "remove files from the public set")?;
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
fn public_list(config_path: &Path, args: PublicListArgs) -> Result<()> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;
    let mut rows: Vec<RuleRow> = Vec::new();
    for (name, share_policy) in &policy.shares {
        if args.share.as_ref().is_some_and(|filter| filter != name) {
            continue;
        }
        for path in &share_policy.public {
            rows.push(RuleRow {
                account: "*".to_string(),
                share: name.clone(),
                path: path.clone(),
            });
        }
    }
    if args.json {
        return print_json(&rows);
    }
    if rows.is_empty() {
        println!("public set is empty");
        return Ok(());
    }
    for row in rows {
        println!("{}:{}", row.share, row.path);
    }
    Ok(())
}

/// Add personal files for one client.
fn client_add(config_path: &Path, name: &str, specs: &[String]) -> Result<()> {
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
fn client_remove(config_path: &Path, name: &str, specs: &[String], yes: bool) -> Result<()> {
    crate::confirm::require(yes, &format!("change access for {name}"))?;
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
fn client_list(config_path: &Path, args: ClientListArgs) -> Result<()> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;
    let accounts = Clients::load(&config.clients_path)?;

    let names: Vec<String> = if let Some(name) = args.name {
        vec![name]
    } else {
        let mut names: Vec<String> = accounts.iter().map(|a| a.name.clone()).collect();
        for share_policy in policy.shares.values() {
            for account in share_policy.clients.keys() {
                if !names.iter().any(|known| known == account) {
                    names.push(account.clone());
                }
            }
        }
        names.sort();
        names.dedup();
        names
    };

    if args.json {
        let mut rows: Vec<RuleRow> = Vec::new();
        for account in &names {
            for (share, share_policy) in &policy.shares {
                for path in share_policy.effective(account) {
                    rows.push(RuleRow {
                        account: account.clone(),
                        share: share.clone(),
                        path,
                    });
                }
            }
        }
        return print_json(&rows);
    }

    if names.is_empty() {
        println!("no accounts");
        return Ok(());
    }
    for account in names {
        let mut any = false;
        for (share, share_policy) in &policy.shares {
            for path in share_policy.effective(&account) {
                println!("{account} {share}:{path}");
                any = true;
            }
        }
        if !any {
            println!("{account} (nothing)");
        }
    }
    Ok(())
}

/// Import an old flat access file into the policy.
fn import(config_path: &Path, from: &Path) -> Result<()> {
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
fn doctor(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;
    let clients = Clients::load(&config.clients_path)?;
    let mut warnings: Vec<String> = Vec::new();

    for share in policy.shares.keys() {
        if !config.shares.contains_key(share) {
            warnings.push(format!("policy references unknown share {share}"));
        }
    }

    for (share, share_policy) in &policy.shares {
        let Some(root) = config.shares.get(share) else {
            continue;
        };
        for path in &share_policy.public {
            check_exists(root, share, path, &mut warnings);
        }
        for (account, deltas) in &share_policy.clients {
            if clients.get(account).is_none() {
                warnings.push(format!("policy references unknown account {account}"));
            }
            for path in &deltas.add {
                check_exists(root, share, path, &mut warnings);
            }
            for path in &deltas.remove {
                if !share_policy.public.contains(path) {
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
fn apply(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    report_regeneration(&config)
}
