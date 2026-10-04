//! `lanpull service` — thin wrappers over systemctl.

use std::process::Command;

use clap::Subcommand;
use lanpull_core::error::{Error, Result};

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
pub fn run(command: ServiceCommand) -> Result<()> {
    match command {
        ServiceCommand::Start => systemctl(&["start", SERVICE], false),
        ServiceCommand::Stop => systemctl(&["stop", SERVICE], false),
        ServiceCommand::Restart => systemctl(&["restart", SERVICE], false),
        ServiceCommand::Status => systemctl(&["--no-pager", "status", SERVICE], true),
        ServiceCommand::Logs => systemctl(&["-u", SERVICE, "-f"], true),
    }
}

/// Run `systemctl` with `args`, inheriting stdio.
///
/// `allow_failure` is used for `status`/`logs`: an inactive unit makes
/// `systemctl status` exit non-zero, which is still a useful result.
fn systemctl(args: &[&str], allow_failure: bool) -> Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .map_err(|e| Error::Server(format!("could not run systemctl: {e}")))?;
    if allow_failure || status.success() {
        Ok(())
    } else {
        Err(Error::Server(format!(
            "systemctl {} failed with {status}",
            args.join(" ")
        )))
    }
}
