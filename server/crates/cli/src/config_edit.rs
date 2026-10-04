//! Line-oriented editing of the `KEY=VALUE` configuration file.
//!
//! The editor preserves comments and ordering: it replaces an existing key in
//! place, or appends a new one. It never rewrites the whole file, so manual
//! annotations survive.

use std::path::Path;

use lanpull_core::error::{Error, Result};

/// Set `key` to `value`, replacing the first existing assignment or appending.
pub fn set(config_path: &Path, key: &str, value: &str) -> Result<()> {
    let text = read(config_path)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut replaced = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some((lhs, _)) = trimmed.split_once('=') {
            if lhs.trim() == key {
                *line = format!("{key}={value}");
                replaced = true;
                break;
            }
        }
    }
    if !replaced {
        lines.push(format!("{key}={value}"));
    }
    write(config_path, &lines)
}

/// Return the value of `key`, if present.
pub fn get(config_path: &Path, key: &str) -> Result<Option<String>> {
    let map = lanpull_core::config::parse_kv(&read(config_path)?);
    Ok(map.get(key).cloned())
}

/// Append a `SHARE_<name>=<dir>` line.
pub fn append_share(config_path: &Path, name: &str, dir: &Path) -> Result<()> {
    set(
        config_path,
        &format!("SHARE_{name}"),
        &dir.display().to_string(),
    )
}

/// Remove every `SHARE_<name>` assignment and drop dependents from manifests.
pub fn remove_share(config_path: &Path, name: &str) -> Result<()> {
    let key = format!("SHARE_{name}");
    let text = read(config_path)?;
    let mut lines: Vec<String> = Vec::new();
    let mut removed = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_key = !trimmed.starts_with('#')
            && trimmed
                .split_once('=')
                .is_some_and(|(lhs, _)| lhs.trim() == key);
        if is_key {
            removed = true;
            continue;
        }
        lines.push(line.to_string());
    }
    if !removed {
        return Err(Error::Config(format!("share {name} is not configured")));
    }
    write(config_path, &lines)
}

/// Read the configuration file as text.
fn read(config_path: &Path) -> Result<String> {
    std::fs::read_to_string(config_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotInitialized(format!(
                "configuration not found at {}",
                config_path.display()
            ))
        } else {
            Error::Config(format!("cannot read {}: {e}", config_path.display()))
        }
    })
}

/// Write the configuration file back, preserving a trailing newline.
///
/// The prospective file is validated first, so an edit that would leave an
/// invalid configuration is rejected without touching the file.
fn write(config_path: &Path, lines: &[String]) -> Result<()> {
    require_canonical_root(config_path)?;
    let mut text = lines.join("\n");
    text.push('\n');
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    lanpull_core::config::Config::parse(&text, base)?;
    lanpull_core::atomic::write(config_path, text.as_bytes())?;
    if is_canonical(config_path) {
        normalize_canonical(config_path)?;
    }
    Ok(())
}

/// Whether `config_path` is the canonical `/etc/lanpull/lanpull.conf`.
fn is_canonical(config_path: &Path) -> bool {
    config_path.parent() == Some(Path::new(lanpull_core::config::DEFAULT_CONFIG_DIR))
}

/// Refuse to edit the canonical configuration without root.
///
/// The atomic write replaces the file, so a normal user would leave it owned by
/// itself, breaking the `root:<operator-group>` mode `0640` contract of
/// `DECISIONS.md` D71. The accounts and policy files are operator-owned and are
/// not affected.
fn require_canonical_root(config_path: &Path) -> Result<()> {
    if is_canonical(config_path) && crate::init::effective_uid() != 0 {
        return Err(Error::Usage(format!(
            "editing the canonical {} needs root; run 'sudo lanpull ...'",
            config_path.display()
        )));
    }
    Ok(())
}

/// Restore the canonical ownership and mode after a root edit.
///
/// The group is the group of `/etc/lanpull` (the operator group set by
/// `install`); the owner is root and the mode is `0640`, matching
/// `DECISIONS.md` D71.
fn normalize_canonical(config_path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let group = std::fs::metadata(dir)?.gid();
    std::fs::set_permissions(config_path, std::fs::Permissions::from_mode(0o640))?;
    std::os::unix::fs::chown(config_path, Some(0), Some(group))?;
    Ok(())
}
