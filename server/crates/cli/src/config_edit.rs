//! Line-oriented editing of the `KEY=VALUE` configuration file.
//!
//! The editor preserves comments and ordering: it replaces an existing key in
//! place, or appends a new one. It never rewrites the whole file, so manual
//! annotations survive.

use std::path::Path;

use lanpull_core::error::{Error, Result};

/// Set `key` to `value`, replacing the first existing assignment or appending.
pub fn set(config_path: &Path, key: &str, value: &str) -> Result<()> {
    set_many(config_path, &[(key, value)], false)
}

/// Set several `key`/`value` pairs in one validated write.
///
/// All replacements are applied in memory and the prospective file is
/// validated once, so a change that would leave an invalid configuration is
/// rejected without touching the file and without a partial edit. With
/// `dry_run` the validation still runs but nothing is written.
pub fn set_many(config_path: &Path, changes: &[(&str, &str)], dry_run: bool) -> Result<()> {
    let text = read(config_path)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (key, value) in changes {
        replace_or_append(&mut lines, key, value);
    }
    let mut prospective = lines.join("\n");
    prospective.push('\n');
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    lanpull_core::config::Config::parse(&prospective, base)?;
    if dry_run {
        return Ok(());
    }
    require_canonical_root(config_path)?;
    lanpull_core::atomic::write(config_path, prospective.as_bytes())?;
    if is_canonical(config_path) {
        normalize_canonical(config_path)?;
    }
    Ok(())
}

/// Replace the first assignment of `key`, or append a new one.
fn replace_or_append(lines: &mut Vec<String>, key: &str, value: &str) {
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some((lhs, _)) = trimmed.split_once('=') {
            if lhs.trim() == key {
                *line = format!("{key}={value}");
                return;
            }
        }
    }
    lines.push(format!("{key}={value}"));
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
