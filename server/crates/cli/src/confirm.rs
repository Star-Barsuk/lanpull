//! Interactive confirmation for destructive commands.
//!
//! With `--yes` the action proceeds. Otherwise the operator must confirm on a
//! controlling terminal; when there is no terminal the command is refused
//! rather than silently proceeding.

use std::io::{BufRead as _, IsTerminal as _, Write as _};

use lanpull_core::error::{Error, Result};

/// Require confirmation for a destructive operation.
pub fn require(yes: bool, action: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::Denied(format!(
            "refusing to {action} without a terminal; pass --yes in scripts"
        )));
    }
    eprint!("{action}? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if matches!(line.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        Err(Error::Denied(format!("{action} cancelled")))
    }
}
