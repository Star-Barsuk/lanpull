//! Account creation and client-folder staging.
//!
//! Creating an account has two side effects that must stay consistent: a line
//! in the accounts file (`CLIENTS_PATH`) and a staged client folder under
//! `$STATE_DIR/client-ready/<name>`. Every check that can fail runs before the
//! accounts file changes, and a later failure rolls both back best-effort, so
//! `create` never leaves a half-created account behind.

pub mod clients;

use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::access::Access;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::policy;

use self::clients::{Account, Clients};

/// The result of a successful account creation.
#[derive(Debug)]
pub struct Created {
    /// Directory holding the staged client folder.
    pub staging: PathBuf,
}

/// Create an account and stage its client folder.
///
/// The account name, the access policy, the staged client bundle, and the TLS
/// certificate are validated before the accounts file is written. If staging
/// fails afterwards, the account line and the staging directory are removed.
pub fn create(
    config: &Config,
    name: &str,
    ip: Option<IpAddr>,
    output: &Path,
    local: bool,
) -> Result<Created> {
    let mut accounts = Clients::load(&config.clients_path)?;
    if accounts.get(name).is_some() {
        return Err(Error::Account(format!("account {name} already exists")));
    }

    let pull_source = config.bundle_dir().join("pull.py");
    if !pull_source.is_file() {
        return Err(Error::Config(
            "client bundle not staged; run 'make install' (or 'make client-bundle')".to_string(),
        ));
    }

    let policy = policy::Policy::load(&config.access_path)?;
    if !policy.has_effective_rules(name) {
        return Err(Error::Account(format!(
            "account {name} has no access; grant it with \
             'lanpull access public add <share>:**' first"
        )));
    }
    if !config.cert_path.is_file() {
        return Err(Error::Certificate(format!(
            "no certificate at {}; run 'lanpull cert' first",
            config.cert_path.display()
        )));
    }

    let password = clients::generate_password();
    let hash = clients::hash_password(&password)?;
    accounts.insert(Account {
        name: name.to_string(),
        hash,
        allowed_ip: ip,
        local,
    });
    accounts.save(&config.clients_path)?;

    // The account now exists, so the policy expands with its `public` rules.
    let access = policy::load_access(config)?;

    let staging = config.state_dir.join("client-ready").join(name);
    if let Err(e) = stage(
        config,
        name,
        output,
        &access,
        &password,
        &pull_source,
        &staging,
    ) {
        rollback(config, name, &staging);
        return Err(e);
    }

    Ok(Created { staging })
}

/// The result of a successful export.
#[derive(Debug)]
pub struct Exported {
    /// Directory the client folder was copied to.
    pub destination: PathBuf,
    /// Whether the destination is on a different filesystem than the staging.
    pub cross_device: bool,
}

/// Copy an account's staged client folder to `destination` (or
/// `destination/<name>` when `destination` is an existing directory).
///
/// The staging directory stays in place unless `move_staging` is set, so the
/// operator can export the same account again. An existing non-empty target is
/// refused unless `force` is set.
pub fn export(
    config: &Config,
    name: &str,
    destination: &Path,
    move_staging: bool,
    force: bool,
) -> Result<Exported> {
    let accounts = Clients::load(&config.clients_path)?;
    if accounts.get(name).is_none() {
        return Err(Error::Account(format!("no such account: {name}")));
    }

    let staging = config.state_dir.join("client-ready").join(name);
    if !staging.is_dir() {
        return Err(Error::Account(format!(
            "no staged folder for {name}; re-create the account with 'lanpull account add {name} --output <dir>'"
        )));
    }
    if !staging.join("auth").is_file() {
        return Err(Error::Account(format!(
            "staged folder for {name} has no auth file; run 'lanpull account passwd {name} --output <dir>'"
        )));
    }

    // Refresh the generated files from the current configuration so a rotated
    // certificate, a rebuilt pull.py, or a changed policy is reflected.
    refresh_staging(config, name)?;

    // A path that is an existing directory means "put the folder inside it".
    let target = if destination.is_dir() {
        destination.join(name)
    } else {
        destination.to_path_buf()
    };

    if target.exists() && !force {
        let empty = target.is_dir()
            && std::fs::read_dir(&target).is_ok_and(|mut entries| entries.next().is_none());
        if !empty {
            return Err(Error::Account(format!(
                "destination {} exists; pass --force to overwrite",
                target.display()
            )));
        }
    }

    let cross_device = crosses_device(&staging, &target);

    copy_tree(&staging, &target, force)?;
    apply_modes(&target)?;

    if move_staging {
        std::fs::remove_dir_all(&staging)?;
    }

    Ok(Exported {
        destination: target,
        cross_device,
    })
}

/// Rewrite the generated files in an existing staging folder from the current
/// configuration, preserving `auth`.
///
/// Used before an export so a rotated certificate, a rebuilt `pull.py`, or a
/// changed policy/share set is reflected in the delivered folder. The mirror
/// base is recovered from the staged `MIRROR_<share>` lines.
pub fn refresh_staging(config: &Config, name: &str) -> Result<PathBuf> {
    let staging = config.state_dir.join("client-ready").join(name);
    if !staging.is_dir() {
        return Err(Error::Account(format!(
            "no staged folder for {name}; re-create the account with 'lanpull account add {name} --output <dir>'"
        )));
    }
    let output = mirror_base(&staging.join("lanpull.conf")).unwrap_or_else(|| PathBuf::from("."));

    let pull_source = config.bundle_dir().join("pull.py");
    if pull_source.is_file() {
        std::fs::copy(&pull_source, staging.join("pull.py"))?;
        set_executable(&staging.join("pull.py"))?;
    }

    let access = policy::load_access(config)?;
    let conf = client_conf(config, name, &output, &access)?;
    crate::atomic::write(&staging.join("lanpull.conf"), conf.as_bytes())?;

    if config.cert_path.is_file() {
        std::fs::copy(&config.cert_path, staging.join("server.crt"))?;
    }
    apply_modes(&staging)?;
    Ok(staging)
}

/// Rebuild an account's staging folder from scratch, preserving the given auth
/// line.
///
/// Used by `account passwd --output` when the staging folder was moved or
/// removed: the account already exists, so only the client files are written.
pub fn restage(config: &Config, name: &str, output: &Path, auth: &str) -> Result<PathBuf> {
    let pull_source = config.bundle_dir().join("pull.py");
    if !pull_source.is_file() {
        return Err(Error::Config(
            "client bundle not staged; run `make install` (or `make client-bundle`)".to_string(),
        ));
    }
    let access = policy::load_access(config)?;
    let staging = config.state_dir.join("client-ready").join(name);
    std::fs::create_dir_all(&staging)?;

    std::fs::copy(&pull_source, staging.join("pull.py"))?;
    set_executable(&staging.join("pull.py"))?;
    let conf = client_conf(config, name, output, &access)?;
    crate::atomic::write(&staging.join("lanpull.conf"), conf.as_bytes())?;
    crate::atomic::write_private(&staging.join("auth"), auth.as_bytes())?;
    if config.cert_path.is_file() {
        std::fs::copy(&config.cert_path, staging.join("server.crt"))?;
    }
    apply_modes(&staging)?;
    Ok(staging)
}

/// Recover the mirror base directory from a staged client configuration.
///
/// The staged file lists `MIRROR_<share>=<base>/<share>`; the base is the
/// parent of any entry. Returns `None` when the file has no `MIRROR_` line.
fn mirror_base(staged_conf: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(staged_conf).ok()?;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.starts_with("MIRROR_") {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let path = Path::new(value);
        return Some(
            path.parent()
                .map_or_else(|| path.to_path_buf(), Path::to_path_buf),
        );
    }
    None
}

/// Compare the device of two paths, walking up to the nearest existing parent.
fn crosses_device(from: &Path, to: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let device = |path: &Path| -> Option<u64> {
        let mut current = path;
        loop {
            if let Ok(meta) = std::fs::metadata(current) {
                return Some(meta.dev());
            }
            current = current.parent()?;
        }
    };
    match (device(from), device(to)) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

/// Recursively copy `source` into `target`.
fn copy_tree(source: &Path, target: &Path, force: bool) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_tree(&from, &to, force)?;
        } else if to.exists() && !force {
            return Err(Error::Account(format!(
                "destination file {} exists; pass --force to overwrite",
                to.display()
            )));
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Fix the expected modes of a staged client folder.
fn apply_modes(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let set = |path: &Path, mode: u32| -> Result<()> {
        if path.exists() {
            let mut permissions = std::fs::metadata(path)?.permissions();
            permissions.set_mode(mode);
            std::fs::set_permissions(path, permissions)?;
        }
        Ok(())
    };
    set(dir, 0o700)?;
    set(&dir.join("pull.py"), 0o755)?;
    set(&dir.join("auth"), 0o600)?;
    set(&dir.join("server.crt"), 0o644)?;
    set(&dir.join("lanpull.conf"), 0o644)?;
    Ok(())
}

/// Write the staged client folder for a freshly created account.
fn stage(
    config: &Config,
    name: &str,
    output: &Path,
    access: &Access,
    password: &str,
    pull_source: &Path,
    staging: &Path,
) -> Result<()> {
    std::fs::create_dir_all(staging)?;

    std::fs::copy(pull_source, staging.join("pull.py"))?;
    set_executable(&staging.join("pull.py"))?;

    std::fs::write(
        staging.join("lanpull.conf"),
        client_conf(config, name, output, access)?,
    )?;
    let auth = format!("{name}:{password}\n");
    crate::atomic::write_private(&staging.join("auth"), auth.as_bytes())?;
    std::fs::copy(&config.cert_path, staging.join("server.crt"))?;
    Ok(())
}

/// Best-effort reversal of a partially applied [`create`].
fn rollback(config: &Config, name: &str, staging: &Path) {
    if let Err(e) = std::fs::remove_dir_all(staging) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("could not remove staging {}: {e}", staging.display());
        }
    }

    let mut accounts = match Clients::load(&config.clients_path) {
        Ok(accounts) => accounts,
        Err(e) => {
            tracing::warn!("could not roll back account {name}: {e}");
            return;
        }
    };
    if accounts.remove(name) {
        if let Err(e) = accounts.save(&config.clients_path) {
            tracing::warn!("could not roll back account {name}: {e}");
        }
    }
}

/// Build the staged client config mapping each visible share to a subdirectory.
pub fn client_conf(
    config: &Config,
    account: &str,
    output: &Path,
    access: &Access,
) -> Result<String> {
    let mut text = format!(
        "# Generated by 'lanpull account add' for {account}.\nSERVER_URL=https://{}:{}\n",
        config.server_ip, config.port
    );
    for share in config.shares.keys() {
        if access.allows_share(account, share) {
            let _ = writeln!(text, "MIRROR_{share}={}", output.join(share).display());
        }
    }
    Ok(text)
}

/// Mark a file executable (mode 0755).
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}
