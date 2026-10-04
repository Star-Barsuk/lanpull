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

/// Dispatch a service operation.
pub fn run(command: ServiceCommand) -> Result<()> {
    match command {
        ServiceCommand::Start => systemctl(&["start", SERVICE]),
        ServiceCommand::Stop => systemctl(&["stop", SERVICE]),
        ServiceCommand::Restart => systemctl(&["restart", SERVICE]),
        ServiceCommand::Status => {
            systemctl(&["--no-pager", "status", SERVICE])?;
            Ok(())
        }
        ServiceCommand::Logs => systemctl(&["-u", SERVICE, "-f"]),
    }
}

/// Run `systemctl` with `args`, inheriting stdio.
fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .map_err(|e| Error::Server(format!("could not run systemctl: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Server(format!(
            "systemctl {} failed with {status}",
            args.join(" ")
        )))
    }
}
