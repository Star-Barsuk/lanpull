//! `lanpull service` — thin wrappers over systemctl.

use std::process::Command;

use clap::Subcommand;
use lanpull_core::error::{Error, Result};

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
    Logs,
}

impl ServiceCommand {
    /// The canonical dotted path of this command.
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Start => "service start",
            Self::Stop => "service stop",
            Self::Restart => "service restart",
            Self::Status => "service status",
            Self::Logs => "service logs",
        }
    }
}

/// Dispatch a service operation.
pub fn run(command: ServiceCommand) -> Result<Outcome> {
    match command {
        ServiceCommand::Start => control("start", "started"),
        ServiceCommand::Stop => control("stop", "stopped"),
        ServiceCommand::Restart => control("restart", "restarted"),
        ServiceCommand::Status => stream(&["--no-pager", "status", SERVICE]),
        ServiceCommand::Logs => stream(&["-u", SERVICE, "-f"]),
    }
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
fn stream(args: &[&str]) -> Result<Outcome> {
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
