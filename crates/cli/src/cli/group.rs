//! `lanpull group` — named groups of accounts.

use std::path::Path;

use clap::Subcommand;
use lanpull_core::config::Config;
use lanpull_core::error::{Error, Result};
use lanpull_core::policy::Policy;
use serde::Serialize;

use crate::cli::output::format_rows;
use crate::cli::Outcome;
use crate::manifest_helpers::report_regeneration;

/// Group operations.
#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Create an empty group.
    Create {
        /// Group name.
        name: String,
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a group and its access rules.
    Remove {
        /// Group name.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage group membership.
    Member {
        /// Membership operation.
        #[command(subcommand)]
        command: MemberCommand,
    },
    /// List groups and their members.
    List(GroupListArgs),
}

impl GroupCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create { .. } => "group create",
            Self::Remove { .. } => "group remove",
            Self::Member { command } => command.path(),
            Self::List(_) => "group list",
        }
    }
}

/// Membership operations.
#[derive(Debug, Subcommand)]
pub enum MemberCommand {
    /// Add members to a group.
    Add {
        /// Group name.
        group: String,
        /// Account or group names to add.
        members: Vec<String>,
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove members from a group.
    Remove {
        /// Group name.
        group: String,
        /// Account or group names to remove.
        members: Vec<String>,
        /// Show what would change without writing the policy.
        #[arg(long)]
        dry_run: bool,
    },
}

impl MemberCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Add { .. } => "group member add",
            Self::Remove { .. } => "group member remove",
        }
    }
}

/// Arguments for `group list`.
#[derive(Debug, clap::Args)]
pub struct GroupListArgs {
    /// Group name (default: every group).
    pub name: Option<String>,
}

/// `group create` payload.
#[derive(Debug, Serialize)]
struct GroupCreated {
    group: String,
    dry_run: bool,
}

/// `group remove` payload.
#[derive(Debug, Serialize)]
struct GroupRemoved {
    group: String,
    dry_run: bool,
}

/// `group member add/remove` payload.
#[derive(Debug, Serialize)]
struct GroupMemberChange {
    group: String,
    members: Vec<String>,
    dry_run: bool,
}

/// Dispatch a group operation.
pub fn run(config_path: &Path, command: GroupCommand) -> Result<Outcome> {
    match command {
        GroupCommand::Create { name, dry_run } => create(config_path, &name, dry_run),
        GroupCommand::Remove { name, yes, dry_run } => remove(config_path, &name, yes, dry_run),
        GroupCommand::Member { command } => match command {
            MemberCommand::Add {
                group,
                members,
                dry_run,
            } => member_add(config_path, &group, &members, dry_run),
            MemberCommand::Remove {
                group,
                members,
                dry_run,
            } => member_remove(config_path, &group, &members, dry_run),
        },
        GroupCommand::List(args) => list(config_path, args),
    }
}

fn create(config_path: &Path, name: &str, dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    if policy.groups.contains_key(name) {
        return Err(Error::Config(format!("group {name} already exists")));
    }
    if dry_run {
        return Ok(Outcome::new()
            .line(format!("would create group {name}"))
            .with_data(&GroupCreated {
                group: name.to_string(),
                dry_run: true,
            }));
    }
    policy.group_mut(name)?;
    policy.save(&config.access_path)?;
    let mut outcome = Outcome::new().line(format!("created group {name}"));
    report_regeneration(&config, &mut outcome)?;
    outcome.set_data(&GroupCreated {
        group: name.to_string(),
        dry_run: false,
    })?;
    Ok(outcome)
}

fn remove(config_path: &Path, name: &str, yes: bool, dry_run: bool) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    if !policy.groups.contains_key(name) {
        return Err(Error::Config(format!("no such group: {name}")));
    }
    if dry_run {
        return Ok(Outcome::new()
            .line(format!("would remove group {name}"))
            .with_data(&GroupRemoved {
                group: name.to_string(),
                dry_run: true,
            }));
    }
    crate::confirm::require(yes, &format!("remove group {name}"))?;
    policy.remove_group(name);
    policy.save(&config.access_path)?;
    let mut outcome = Outcome::new().line(format!("removed group {name}"));
    report_regeneration(&config, &mut outcome)?;
    outcome.set_data(&GroupRemoved {
        group: name.to_string(),
        dry_run: false,
    })?;
    Ok(outcome)
}

fn member_add(
    config_path: &Path,
    group: &str,
    members: &[String],
    dry_run: bool,
) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    if !policy.groups.contains_key(group) {
        return Err(Error::Config(format!("no such group: {group}")));
    }
    if dry_run {
        return Ok(Outcome::new()
            .line(format!("would add {} to {group}", members.join(", ")))
            .with_data(&GroupMemberChange {
                group: group.to_string(),
                members: members.to_vec(),
                dry_run: true,
            }));
    }
    let def = policy.group_mut(group)?;
    for member in members {
        def.members.insert(member.clone());
    }
    policy.save(&config.access_path)?;
    let mut outcome = Outcome::new().line(format!("added {} to {group}", members.join(", ")));
    report_regeneration(&config, &mut outcome)?;
    outcome.set_data(&GroupMemberChange {
        group: group.to_string(),
        members: members.to_vec(),
        dry_run: false,
    })?;
    Ok(outcome)
}

fn member_remove(
    config_path: &Path,
    group: &str,
    members: &[String],
    dry_run: bool,
) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let mut policy = Policy::load(&config.access_path)?;
    if !policy.groups.contains_key(group) {
        return Err(Error::Config(format!("no such group: {group}")));
    }
    if dry_run {
        return Ok(Outcome::new()
            .line(format!("would remove {} from {group}", members.join(", ")))
            .with_data(&GroupMemberChange {
                group: group.to_string(),
                members: members.to_vec(),
                dry_run: true,
            }));
    }
    let def = policy.group_mut(group)?;
    for member in members {
        def.members.remove(member);
    }
    policy.save(&config.access_path)?;
    let mut outcome = Outcome::new().line(format!("removed {} from {group}", members.join(", ")));
    report_regeneration(&config, &mut outcome)?;
    outcome.set_data(&GroupMemberChange {
        group: group.to_string(),
        members: members.to_vec(),
        dry_run: false,
    })?;
    Ok(outcome)
}

fn list(config_path: &Path, args: GroupListArgs) -> Result<Outcome> {
    let config = Config::load(config_path)?;
    let policy = Policy::load(&config.access_path)?;

    let groups: Vec<String> = if let Some(name) = args.name {
        vec![name]
    } else {
        let mut names: Vec<String> = policy.groups.keys().cloned().collect();
        names.sort();
        names
    };

    if groups.is_empty() {
        return Ok(Outcome::text(vec!["no groups".to_string()]));
    }

    let mut table = vec![vec!["GROUP".to_string(), "MEMBERS".to_string()]];
    for group in &groups {
        let members = policy
            .groups
            .get(group)
            .map(|def| def.members.iter().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        table.push(vec![
            group.clone(),
            if members.is_empty() {
                "(none)".to_string()
            } else {
                members
            },
        ]);
    }
    let lines = format_rows(&table);
    Ok(Outcome::text(lines))
}
