//! Process-wide verbosity, set once from the global `-v` count.
//!
//! Command output is otherwise identical regardless of verbosity; only the
//! optional detail (such as per-account manifest counts) is gated here.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether detailed output was requested on the command line.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Record whether detailed output was requested.
pub fn set(verbose: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
}

/// Whether detailed output was requested.
pub fn enabled() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}
