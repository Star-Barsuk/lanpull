//! `lanpull access` — the JSON access policy.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use lanpull_core::access::Rule;
use lanpull_core::clients::Clients;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::policy::{self, Policy};
use serde::Serialize;

use crate::cli::output::format_rows;
use crate::cli::Outcome;
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
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check the policy for unknown shares, missing files, and empty accounts.
    Doctor,
    /// Regenerate the per-account manifests after a manual edit.
    Apply,
}

impl AccessCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Public { command } => command.path(),
            Self::Client { command } => command.path(),
            Self::Import { .. } => "access import",
            Self::Doctor => "access doctor",
            Self::Apply => "access apply",
        }
    }
}

/// Public-set operations.
#[derive(Debug, Subcommand)]
pub enum PublicCommand {
    /// Add files to the public set.
    Add {
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove files from the public set.
    Remove {
        /// Rules `<share>:<path>` (repeatable).
        rules: Vec<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
    /// List the public set.
    List(PublicListArgs),
}

impl PublicCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Add { .. } => "access public add",
            Self::Remove { .. } => "access public remove",
            Self::List(_) => "access public list",
        }
    }
}

/// Arguments for `access public list`.
#[derive(Debug, clap::Args)]
pub struct PublicListArgs {
    /// Only show this share.
    #[arg(long)]
    pub share: Option<String>,
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
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
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
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the effective set for one or every client.
    List(ClientListArgs),
}

impl ClientCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Add { .. } => "access client add",
            Self::Remove { .. } => "access client remove",
            Self::List(_) => "access client list",
        }
    }
}

/// Arguments for `access client list`.
#[derive(Debug, clap::Args)]
pub struct ClientListArgs {
    /// Account name (default: every account).
    pub name: Option<String>,
}

/// One effective rule row, used for JSON output.
#[derive(Debug, Serialize)]
struct RuleRow {
    account: String,
    share: String,
    path: String,
}

/// Dispatch an access-policy operation.
pub fn run(config_path: &Path, command: AccessCommand) -> Result<Outcome> {
    match command {
        AccessCommand::Public { command } => match command {
            PublicCommand::Add { rules, dry_run } => public_add(config_path, &rules, dry_run),
            PublicCommand::Remove {
                rules,
                yes,
                dry_run,
            } => public_remove(config_path, &rules, yes, dry_run),
            PublicCommand::List(args) => public_list(config_path, args),
        },
        AccessCommand::Client { command } => match command {
            ClientCommand::Add {
                name,
                rules,
                dry_run,
            } => client_add(config_path, &name, &rules, dry_run),
            ClientCommand::Remove {
                name,
                rules,
                yes,
                dry_run,
            } => client_remove(config_path, &name, &rules, yes, dry_run),
            ClientCommand::List(args) => client_list(config_path, args),
        },
        AccessCommand::Import { from, dry_run } => import(config_path, &from, dry_run),
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

/// The text/data result of a rule mutation.
fn rule_outcome(base: &str, past: &str, rules: &[String], dry_run: bool) -> Outcome {
    let summary = if dry_run {
        format!("would {base} {}", rules.join(", "))
    } else {
        format!("{past} {}", rules.join(", "))
    };
    Outcome::new()
        .line(summary)
        .with_data(&serde_json::json!({ "rules": rules, "dry_run": dry_run }))
}

/// Add rules to the public set.
fn public_add(config_path: &Path, specs: &[String], dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.public.insert(path);
    }
    if dry_run {
        return Ok(rule_outcome(
            "add to the public set",
            "added to the public set",
            specs,
            true,
        ));
    }
    policy.save(&config.access_path)?;
    let mut outcome = rule_outcome(
        "add to the public set",
        "added to the public set",
        specs,
        false,
    );
    report_regeneration(&config, &mut outcome)?;
    Ok(outcome)
}

/// Remove rules from the public set.
fn public_remove(
    config_path: &Path,
    specs: &[String],
    yes: bool,
    dry_run: bool,
) -> Result<Outcome> {
    if !dry_run {
        crate::confirm::require(yes, "remove files from the public set")?;
    }
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.public.remove(&path);
    }
    if dry_run {
        return Ok(rule_outcome(
            "remove from the public set",
            "removed from the public set",
            specs,
            true,
        ));
    }
    policy.save(&config.access_path)?;
    let mut outcome = rule_outcome(
        "remove from the public set",
        "removed from the public set",
        specs,
        false,
    );
    report_regeneration(&config, &mut outcome)?;
    Ok(outcome)
}

/// List the public set.
fn public_list(config_path: &Path, args: PublicListArgs) -> Result<Outcome> {
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
    let lines = if rows.is_empty() {
        vec!["public set is empty".to_string()]
    } else {
        format_rows(
            &rows
                .iter()
                .map(|row| vec![format!("{}:{}", row.share, row.path)])
                .collect::<Vec<_>>(),
        )
    };
    Ok(Outcome::text(lines).with_data(&rows))
}

/// Add personal files for one client.
fn client_add(config_path: &Path, name: &str, specs: &[String], dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let accounts = Clients::load(&config.clients_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.add_for(name, path);
    }
    let mut outcome = rule_outcome("grant", "granted", specs, dry_run);
    if accounts.get(name).is_none() {
        outcome = outcome.warn(format!(
            "account {name} does not exist yet; the rule applies once it is created"
        ));
    }
    if dry_run {
        return Ok(outcome);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config, &mut outcome)?;
    Ok(outcome)
}

/// Remove files (including public ones) for one client.
fn client_remove(
    config_path: &Path,
    name: &str,
    specs: &[String],
    yes: bool,
    dry_run: bool,
) -> Result<Outcome> {
    if !dry_run {
        crate::confirm::require(yes, &format!("change access for {name}"))?;
    }
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    for spec in specs {
        let (share, path) = policy::split_rule(spec)?;
        ensure_share(&config, &share)?;
        policy.share_mut(&share)?.remove_for(name, &path);
    }
    let mut outcome = rule_outcome("revoke", "revoked", specs, dry_run);
    if dry_run {
        return Ok(outcome);
    }
    policy.save(&config.access_path)?;
    report_regeneration(&config, &mut outcome)?;
    Ok(outcome)
}

/// Show the effective set for one or every client.
fn client_list(config_path: &Path, args: ClientListArgs) -> Result<Outcome> {
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

    if rows.is_empty() {
        let lines = if names.is_empty() {
            vec!["no accounts".to_string()]
        } else {
            names
                .iter()
                .map(|account| format!("{account} (nothing)"))
                .collect()
        };
        return Ok(Outcome::text(lines).with_data(&rows));
    }

    // Group the effective set by account: one line per account, specs comma-separated.
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for row in &rows {
        let spec = format!("{}:{}", row.share, row.path);
        match grouped.last_mut() {
            Some((account, specs)) if *account == row.account => specs.push(spec),
            _ => grouped.push((row.account.clone(), vec![spec])),
        }
    }
    let lines: Vec<String> = grouped
        .iter()
        .map(|(account, specs)| format!("{account}: {}", specs.join(", ")))
        .collect();
    Ok(Outcome::text(lines).with_data(&rows))
}

/// Import an old flat access file into the policy.
fn import(config_path: &Path, from: &Path, dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let text = std::fs::read_to_string(from)
        .map_err(|e| Error::Config(format!("cannot read {}: {e}", from.display())))?;
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
    let data = serde_json::json!({ "imported": imported, "dry_run": dry_run });
    if dry_run {
        return Ok(Outcome::new()
            .line(format!("would import {imported} rules"))
            .with_data(&data));
    }
    policy.save(&config.access_path)?;
    let mut outcome = Outcome::new().line(format!("imported {imported} rules"));
    report_regeneration(&config, &mut outcome)?;
    outcome.data = data;
    Ok(outcome)
}

/// Check the policy for unknown shares, missing files, and empty accounts.
fn doctor(config_path: &Path) -> Result<Outcome> {
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

    let summary = if warnings.is_empty() {
        "access policy looks consistent".to_string()
    } else {
        format!("{} warning(s)", warnings.len())
    };
    let mut outcome = Outcome::new().line(summary);
    outcome.data = serde_json::json!({ "warnings": &warnings });
    outcome.warnings = warnings;
    Ok(outcome)
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
fn apply(config_path: &Path) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut outcome = Outcome::new();
    report_regeneration(&config, &mut outcome)?;
    if outcome.lines.is_empty() {
        outcome.lines.push("manifests regenerated".to_string());
    }
    Ok(outcome)
}
