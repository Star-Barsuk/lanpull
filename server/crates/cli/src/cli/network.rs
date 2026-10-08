//! `lanpull network` — named per-network server address and TLS profiles.
//!
//! A profile records the address and certificate a server uses on one LAN.
//! `lanpull network use <name>` applies a profile to the configuration and
//! generates its certificate once; switching between profiles never overwrites
//! an existing pinned certificate, so the clients of the other networks keep
//! working without any change on their side.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use lanpull_core::cert;
use lanpull_core::config::{valid_network_name, Config};
use lanpull_core::error::{Error, Result};
use lanpull_core::network::{Network, Networks};
use serde::Serialize;

use crate::cli::output::format_rows;
use crate::cli::service::{self, ServiceCommand};
use crate::cli::Outcome;

/// Network-profile operations.
#[derive(Debug, Subcommand)]
pub enum NetworkCommand {
    /// Register a network profile.
    Add(AddArgs),
    /// List the registered network profiles.
    List,
    /// Show one profile (the active one by default).
    Show {
        /// Profile name; defaults to the active profile.
        name: Option<String>,
    },
    /// Apply a profile to the configuration and ensure its certificate.
    Use(UseArgs),
    /// Remove a network profile.
    Remove(RemoveArgs),
}

impl NetworkCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Add(_) => "network add",
            Self::List => "network list",
            Self::Show { .. } => "network show",
            Self::Use(_) => "network use",
            Self::Remove(_) => "network remove",
        }
    }
}

/// Arguments for `network add`.
#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// Profile name.
    pub name: String,
    /// Server address clients use on this network.
    #[arg(long)]
    pub ip: IpAddr,
    /// PEM certificate path (default `<STATE_DIR>/net-<name>.crt`).
    #[arg(long)]
    pub cert: Option<PathBuf>,
    /// PEM private-key path (default `<STATE_DIR>/net-<name>.key`).
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Generate the certificate now when it does not exist yet.
    #[arg(long)]
    pub generate: bool,
    /// Replace an existing profile.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `network use`.
#[derive(Debug, clap::Args)]
pub struct UseArgs {
    /// Profile name.
    pub name: String,
    /// Show what would change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Regenerate the certificate even if it exists; this network's clients
    /// must then receive the new server.crt.
    #[arg(long)]
    pub regenerate_cert: bool,
    /// Restart the systemd service after switching.
    #[arg(long)]
    pub restart: bool,
}

/// Arguments for `network remove`.
#[derive(Debug, clap::Args)]
pub struct RemoveArgs {
    /// Profile name.
    pub name: String,
    /// Also delete the profile's certificate and key files.
    #[arg(long)]
    pub delete_cert: bool,
    /// Do not ask for confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// One row of `network list` output.
#[derive(Debug, Serialize)]
struct NetworkRow {
    name: String,
    ip: String,
    active: bool,
    cert_path: String,
    key_path: String,
    cert_present: bool,
}

/// Dispatch a network-profile operation.
pub fn run(config_path: &Path, command: NetworkCommand) -> Result<Outcome> {
    match command {
        NetworkCommand::Add(args) => add(config_path, &args),
        NetworkCommand::List => list(config_path),
        NetworkCommand::Show { name } => show(config_path, name.as_deref()),
        NetworkCommand::Use(args) => use_profile(config_path, &args),
        NetworkCommand::Remove(args) => remove(config_path, &args),
    }
}

/// Register a network profile.
fn add(config_path: &Path, args: &AddArgs) -> Result<Outcome> {
    let (config, mut networks) = load(config_path)?;
    if !valid_network_name(&args.name) {
        return Err(Error::Usage(format!(
            "invalid network name '{}': use [a-z0-9][a-z0-9_-]*",
            args.name
        )));
    }
    if networks.get(&args.name).is_some() && !args.force {
        return Err(Error::Network(format!(
            "network '{}' already exists; pass --force to replace it",
            args.name
        )));
    }
    let network = Network {
        name: args.name.clone(),
        ip: args.ip,
        cert_path: args
            .cert
            .clone()
            .unwrap_or_else(|| default_cert_path(&config, &args.name)),
        key_path: args
            .key
            .clone()
            .unwrap_or_else(|| default_key_path(&config, &args.name)),
    };

    let mut lines = vec![format!("added network {} ({})", network.name, network.ip)];
    if args.generate {
        match ensure_cert(&network, false, false)? {
            CertAction::Created | CertAction::Generated => {
                lines.push(format!("generated certificate for {}", network.name));
            }
            CertAction::Exists => {
                lines.push(format!(
                    "certificate for {} already exists; left unchanged",
                    network.name
                ));
            }
            CertAction::WouldGenerate => {
                lines.push(format!("would generate certificate for {}", network.name));
            }
        }
    }

    networks.insert(network.clone());
    networks.save(&config.networks_path)?;
    Ok(Outcome::text(lines).with_data(&network))
}

/// List the registered profiles, marking the active one.
fn list(config_path: &Path) -> Result<Outcome> {
    let (config, networks) = load(config_path)?;
    if networks.is_empty() {
        return Ok(Outcome::text(vec!["no networks configured".to_string()])
            .with_data(&serde_json::json!({ "networks": [] })));
    }
    let rows: Vec<NetworkRow> = networks
        .iter()
        .map(|network| NetworkRow {
            name: network.name.clone(),
            ip: network.ip.to_string(),
            active: is_active(&config, network),
            cert_path: network.cert_path.display().to_string(),
            key_path: network.key_path.display().to_string(),
            cert_present: network.cert_path.is_file() && network.key_path.is_file(),
        })
        .collect();

    let mut table = vec![vec![
        "NAME".to_string(),
        "IP".to_string(),
        "ACTIVE".to_string(),
        "CERT".to_string(),
        "KEY".to_string(),
    ]];
    for row in &rows {
        table.push(vec![
            if row.active {
                format!("{} *", row.name)
            } else {
                row.name.clone()
            },
            row.ip.clone(),
            if row.active {
                "yes".to_string()
            } else {
                "no".to_string()
            },
            if row.cert_present {
                row.cert_path.clone()
            } else {
                format!("{} (missing)", row.cert_path)
            },
            row.key_path.clone(),
        ]);
    }
    Ok(Outcome::text(format_rows(&table)).with_data(&serde_json::json!({ "networks": rows })))
}

/// Show one profile, or the active profile when no name is given.
fn show(config_path: &Path, name: Option<&str>) -> Result<Outcome> {
    let (config, networks) = load(config_path)?;
    let network = match name {
        Some(name) => networks.get(name).ok_or_else(|| unknown(name))?,
        None => networks
            .iter()
            .find(|network| is_active(&config, network))
            .ok_or_else(|| {
                Error::Network("no active network profile; run 'lanpull network list'".to_string())
            })?,
    };
    let active = is_active(&config, network);
    let lines = vec![
        format!("name:      {}", network.name),
        format!("ip:        {}", network.ip),
        format!("cert_path: {}", network.cert_path.display()),
        format!("key_path:  {}", network.key_path.display()),
        format!("active:    {active}"),
    ];
    Ok(Outcome::text(lines).with_data(&serde_json::json!({
        "name": network.name,
        "ip": network.ip.to_string(),
        "cert_path": network.cert_path,
        "key_path": network.key_path,
        "active": active,
    })))
}

/// Apply a profile to the configuration and ensure its certificate exists.
fn use_profile(config_path: &Path, args: &UseArgs) -> Result<Outcome> {
    let (config, networks) = load(config_path)?;
    let network = networks
        .get(&args.name)
        .ok_or_else(|| unknown(&args.name))?
        .clone();

    let mut lines = Vec::new();
    let mut warnings = Vec::new();

    let active = is_active(&config, &network);
    let server_ip = network.ip.to_string();
    let cert_path = network.cert_path.display().to_string();
    let key_path = network.key_path.display().to_string();

    // Validate the prospective configuration before generating anything, so a
    // certificate is never written for a configuration that would be rejected
    // (for example a certificate path inside a share root).
    if !active {
        crate::config_edit::set_many(
            config_path,
            &[
                ("SERVER_IP", &server_ip),
                ("CERT_PATH", &cert_path),
                ("KEY_PATH", &key_path),
            ],
            true,
        )?;
    }

    match ensure_cert(&network, args.regenerate_cert, args.dry_run)? {
        CertAction::Generated => {
            lines.push(format!("regenerated certificate for {}", network.name));
        }
        CertAction::Created => lines.push(format!("generated certificate for {}", network.name)),
        CertAction::Exists => {}
        CertAction::WouldGenerate => {
            lines.push(format!("would generate certificate for {}", network.name));
        }
    }

    if active {
        lines.push(format!("network {} is already active", network.name));
    } else if args.dry_run {
        lines.push(format!(
            "would set SERVER_IP={server_ip}, CERT_PATH={cert_path}, KEY_PATH={key_path}"
        ));
    } else {
        crate::config_edit::set_many(
            config_path,
            &[
                ("SERVER_IP", &server_ip),
                ("CERT_PATH", &cert_path),
                ("KEY_PATH", &key_path),
            ],
            false,
        )?;
        lines.push(format!("activated network {}", network.name));
    }

    if args.regenerate_cert && !args.dry_run {
        warnings.push(format!(
            "copy the new {cert_path} to every client of this network"
        ));
    }
    if !active && !args.dry_run {
        lines.push(
            "next: export this network's accounts, then run 'lanpull account arm --all'"
                .to_string(),
        );
    }

    if args.restart {
        let restart = service::run(ServiceCommand::Restart)?;
        lines.extend(restart.lines);
    }

    let mut outcome = Outcome::text(lines).with_data(&serde_json::json!({
        "name": network.name,
        "ip": network.ip.to_string(),
        "cert_path": network.cert_path,
        "key_path": network.key_path,
        "dry_run": args.dry_run,
        "restart": args.restart,
    }));
    for warning in warnings {
        outcome = outcome.warn(warning);
    }
    Ok(outcome)
}

/// Remove a network profile, optionally deleting its TLS material.
fn remove(config_path: &Path, args: &RemoveArgs) -> Result<Outcome> {
    let (config, mut networks) = load(config_path)?;
    let network = networks
        .get(&args.name)
        .ok_or_else(|| unknown(&args.name))?
        .clone();

    if args.delete_cert {
        crate::confirm::require(
            args.yes,
            &format!("delete the certificate and key of network '{}'", args.name),
        )?;
    }

    let mut lines = vec![format!("removed network {}", network.name)];
    networks.remove(&args.name);
    networks.save(&config.networks_path)?;

    if args.delete_cert {
        remove_if_present(&network.cert_path, &mut lines);
        remove_if_present(&network.key_path, &mut lines);
    }
    Ok(Outcome::text(lines).with_data(&serde_json::json!({
        "name": network.name,
        "deleted_cert": args.delete_cert,
    })))
}

/// The result of ensuring a profile's certificate.
enum CertAction {
    /// The certificate already existed and was left alone.
    Exists,
    /// A missing certificate was created.
    Created,
    /// An existing certificate was regenerated.
    Generated,
    /// A missing certificate would be created (dry run).
    WouldGenerate,
}

/// Generate the profile certificate when missing (or when `regenerate` is set).
fn ensure_cert(network: &Network, regenerate: bool, dry_run: bool) -> Result<CertAction> {
    let present = network.cert_path.is_file() && network.key_path.is_file();
    if present && !regenerate {
        return Ok(CertAction::Exists);
    }
    if dry_run {
        return Ok(CertAction::WouldGenerate);
    }
    cert::generate(&network.cert_path, &network.key_path, network.ip)?;
    Ok(if present {
        CertAction::Generated
    } else {
        CertAction::Created
    })
}

/// Remove a file, ignoring a missing one, and note it on `lines`.
fn remove_if_present(path: &Path, lines: &mut Vec<String>) {
    match std::fs::remove_file(path) {
        Ok(()) => lines.push(format!("deleted {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => lines.push(format!("warning: could not delete {}: {e}", path.display())),
    }
}

/// Load the configuration and its profile set.
fn load(config_path: &Path) -> Result<(Config, Networks)> {
    let config = Config::load(config_path)?;
    let networks = Networks::load(&config.networks_path)?;
    Ok((config, networks))
}

/// Whether the configuration currently uses `network`.
fn is_active(config: &Config, network: &Network) -> bool {
    let address_matches = config.server_ip == network.ip;
    let cert_matches = config.cert_path == network.cert_path;
    let key_matches = config.key_path == network.key_path;
    address_matches && cert_matches && key_matches
}

/// The default certificate path for a profile.
fn default_cert_path(config: &Config, name: &str) -> PathBuf {
    config.state_dir.join(format!("net-{name}.crt"))
}

/// The default private-key path for a profile.
fn default_key_path(config: &Config, name: &str) -> PathBuf {
    config.state_dir.join(format!("net-{name}.key"))
}

/// The error for an unknown profile name.
fn unknown(name: &str) -> Error {
    Error::Network(format!(
        "unknown network '{name}'; run 'lanpull network list'"
    ))
}
