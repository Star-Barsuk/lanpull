//! `lanpull service` — thin wrappers over systemctl.

use std::process::Command;

use clap::Subcommand;
use lanpull_core::error::{Error, Result};
use lanpull_core::timeutil;

use crate::cli::Outcome;

/// The systemd unit name.
const SERVICE: &str = "lanpull.service";

/// Service operations.
#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Start the service.
    Start,
    /// Stop the service.
    Stop,
    /// Restart the service.
    Restart,
    /// Show the service status.
    Status,
    /// Follow the service log.
    Logs(LogsArgs),
}

impl ServiceCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Start => "service start",
            Self::Stop => "service stop",
            Self::Restart => "service restart",
            Self::Status => "service status",
            Self::Logs(_) => "service logs",
        }
    }
}

/// Arguments for `service logs`.
#[derive(Debug, clap::Args)]
pub struct LogsArgs {
    /// Number of most recent journal entries to show.
    #[arg(long, short = 'n', value_name = "N")]
    pub lines: Option<usize>,
    /// Only show entries newer than this duration (for example `1h`) or timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Do not follow the log.
    #[arg(long)]
    pub no_follow: bool,
    /// Minimum priority, for example `err`, `warning`, or `info`.
    #[arg(long)]
    pub priority: Option<String>,
}

/// Dispatch a service operation.
pub fn run(command: ServiceCommand) -> Result<Outcome> {
    match command {
        ServiceCommand::Start => control("start", "started"),
        ServiceCommand::Stop => control("stop", "stopped"),
        ServiceCommand::Restart => control("restart", "restarted"),
        ServiceCommand::Status => stream(&[
            "--no-pager".to_string(),
            "status".to_string(),
            SERVICE.to_string(),
        ]),
        ServiceCommand::Logs(args) => stream(&journal_args(&args)),
    }
}

/// Build the `journalctl` argument list for `service logs`.
///
/// A `--since` value that parses as a duration becomes an absolute `@<epoch>`,
/// so it does not depend on `journalctl`'s English time parser; any other value
/// is passed through unchanged.
fn journal_args(args: &LogsArgs) -> Vec<String> {
    let mut out = vec!["-u".to_string(), SERVICE.to_string()];
    if let Some(lines) = args.lines {
        out.push("-n".to_string());
        out.push(lines.to_string());
    }
    if let Some(since) = &args.since {
        out.push("--since".to_string());
        out.push(journal_since(since));
    }
    if let Some(priority) = &args.priority {
        out.push("-p".to_string());
        out.push(priority.clone());
    }
    if !args.no_follow {
        out.push("-f".to_string());
    }
    out
}

/// Translate a duration into a journalctl `@<epoch>` filter, or pass it through.
fn journal_since(value: &str) -> String {
    timeutil::parse_duration_secs(value).map_or_else(
        |_| value.to_string(),
        |seconds| format!("@{}", timeutil::now_unix().saturating_sub(seconds)),
    )
}

/// Run a mutating systemctl verb, capturing its output and reporting one line.
fn control(verb: &str, past: &str) -> Result<Outcome> {
    let output = Command::new("systemctl")
        .arg(verb)
        .arg(SERVICE)
        .output()
        .map_err(|e| Error::Server(format!("could not run systemctl: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Server(format!(
            "systemctl {verb} {SERVICE} failed: {}",
            stderr.trim()
        )));
    }
    Ok(Outcome::new()
        .line(format!("service {SERVICE} {past}"))
        .with_data(&serde_json::json!({ "service": SERVICE, "action": verb })))
}

/// Run a read-only systemctl verb, forwarding its output to the terminal.
///
/// `status` and `logs` are pass-through: their human output *is* the result, so
/// they write straight to the inherited stdio and return an empty outcome
/// instead of buffering it. This is the documented exception to the `--json`
/// envelope contract.
fn stream(args: &[String]) -> Result<Outcome> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .map_err(|e| Error::Server(format!("could not run systemctl: {e}")))?;
    if !status.success() {
        // `systemctl status` exits non-zero for an inactive unit; that is a
        // useful result, not a failure.
        tracing::debug!("systemctl {} exited with {status}", args.join(" "));
    }
    Ok(Outcome::new().with_data(&serde_json::json!({ "service": SERVICE })))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::missing_assert_message
    )]

    use super::*;

    fn args() -> LogsArgs {
        LogsArgs {
            lines: None,
            since: None,
            no_follow: false,
            priority: None,
        }
    }

    #[test]
    fn default_follows_the_unit() {
        assert_eq!(
            journal_args(&args()),
            vec!["-u".to_string(), SERVICE.to_string(), "-f".to_string()]
        );
    }

    #[test]
    fn no_follow_omits_the_follow_flag() {
        let mut value = args();
        value.no_follow = true;
        assert_eq!(
            journal_args(&value),
            vec!["-u".to_string(), SERVICE.to_string()]
        );
    }

    #[test]
    fn lines_priority_and_since_are_mapped() {
        let value = LogsArgs {
            lines: Some(100),
            since: Some("3600s".to_string()),
            no_follow: true,
            priority: Some("err".to_string()),
        };
        let built = journal_args(&value);
        assert_eq!(built[0], "-u");
        assert_eq!(built[1], SERVICE);
        assert!(built.contains(&"-n".to_string()));
        assert!(built.contains(&"100".to_string()));
        assert!(built.contains(&"-p".to_string()));
        assert!(built.contains(&"err".to_string()));
        assert!(built.iter().any(|arg| arg.starts_with('@')));
        assert!(!built.contains(&"-f".to_string()));
    }
}
