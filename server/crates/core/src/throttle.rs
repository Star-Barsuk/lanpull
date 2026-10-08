//! Failed-authentication throttling.
//!
//! Authentication is intentionally per-request and password verification is
//! deliberately expensive (argon2), so an unauthenticated client on the LAN
//! could otherwise force unbounded hashing. Failures are counted per source
//! address; after too many within a window the address is locked out for a
//! short cooling period without running argon2 at all. The state is runtime
//! only and never persisted.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Failures allowed within the window before a source is locked out.
const MAX_FAILURES: u32 = 10;
/// Length of the rolling window a failure is remembered for.
const WINDOW: Duration = Duration::from_secs(60);
/// How long a source stays locked out once the threshold is reached.
const LOCKOUT: Duration = Duration::from_secs(60);

/// One source's failure history.
#[derive(Debug, Default, Clone)]
struct Entry {
    /// Number of failures recorded in the current window.
    failures: u32,
    /// Start of the current failure window.
    first_failure: Option<Instant>,
    /// Time until which the source is locked out.
    blocked_until: Option<Instant>,
}

impl Entry {
    /// Return whether the entry still matters at `now`.
    ///
    /// An entry is live while its window is open or a lockout is in effect; it
    /// is otherwise dead weight and is pruned, so the map cannot grow without
    /// bound over a long uptime.
    fn is_live(&self, now: Instant, window: Duration) -> bool {
        if self.blocked_until.is_some_and(|until| until > now) {
            return true;
        }
        self.first_failure
            .is_some_and(|first| now.duration_since(first) <= window)
    }

    /// Fold one failure into the entry, locking out at the threshold.
    fn record(&mut self, now: Instant, window: Duration, max_failures: u32, lockout: Duration) {
        if self
            .first_failure
            .is_none_or(|first| now.duration_since(first) > window)
        {
            self.failures = 0;
            self.first_failure = Some(now);
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= max_failures {
            self.blocked_until = now.checked_add(lockout);
        }
    }
}

/// A small per-source failure counter with a lockout.
#[derive(Debug)]
pub struct Throttle {
    entries: Mutex<BTreeMap<IpAddr, Entry>>,
    max_failures: u32,
    window: Duration,
    lockout: Duration,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::with_limits(MAX_FAILURES, WINDOW, LOCKOUT)
    }
}

impl Throttle {
    /// Return an empty throttle with the default limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a throttle with explicit limits (used by tests).
    pub const fn with_limits(max_failures: u32, window: Duration, lockout: Duration) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            max_failures,
            window,
            lockout,
        }
    }

    /// Return `true` when `ip` may attempt authentication now.
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let blocked = {
            let mut entries = self.entries();
            entries.retain(|_, entry| entry.is_live(now, self.window));
            entries.get(&ip).and_then(|entry| entry.blocked_until)
        };
        match blocked {
            Some(until) if until > now => false,
            Some(_) => {
                self.entries().remove(&ip);
                true
            }
            None => true,
        }
    }

    /// Record a failed authentication attempt from `ip`.
    pub fn record_failure(&self, ip: IpAddr) {
        let now = Instant::now();
        let (window, max_failures, lockout) = (self.window, self.max_failures, self.lockout);
        let mut entries = self.entries();
        entries.retain(|_, entry| entry.is_live(now, window));
        entries
            .entry(ip)
            .or_default()
            .record(now, window, max_failures, lockout);
    }

    /// Clear the failure history for `ip` after a successful authentication.
    pub fn record_success(&self, ip: IpAddr) {
        self.entries().remove(&ip);
    }

    /// Lock the entry map, recovering from a poisoned mutex.
    fn entries(&self) -> MutexGuard<'_, BTreeMap<IpAddr, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::arithmetic_side_effects, clippy::missing_assert_message)]

    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([127, 0, 0, last])
    }

    #[test]
    fn allows_below_threshold_and_locks_at_it() {
        let throttle = Throttle::with_limits(3, Duration::from_secs(60), Duration::from_secs(60));
        assert!(throttle.allow(ip(1)));
        for _ in 0..2 {
            throttle.record_failure(ip(1));
        }
        assert!(throttle.allow(ip(1)));
        throttle.record_failure(ip(1));
        assert!(!throttle.allow(ip(1)));
    }

    #[test]
    fn success_clears_history() {
        let throttle = Throttle::with_limits(2, Duration::from_secs(60), Duration::from_secs(60));
        throttle.record_failure(ip(2));
        throttle.record_success(ip(2));
        throttle.record_failure(ip(2));
        assert!(throttle.allow(ip(2)));
    }

    #[test]
    fn lockout_expires() {
        let throttle =
            Throttle::with_limits(1, Duration::from_millis(1), Duration::from_millis(20));
        throttle.record_failure(ip(3));
        assert!(!throttle.allow(ip(3)));
        std::thread::sleep(Duration::from_millis(30));
        assert!(throttle.allow(ip(3)));
    }

    #[test]
    fn window_expires_failures() {
        let throttle = Throttle::with_limits(2, Duration::from_millis(10), Duration::from_secs(60));
        throttle.record_failure(ip(4));
        std::thread::sleep(Duration::from_millis(20));
        throttle.record_failure(ip(4));
        assert!(throttle.allow(ip(4)));
    }

    #[test]
    fn sources_are_independent() {
        let throttle = Throttle::with_limits(1, Duration::from_secs(60), Duration::from_secs(60));
        throttle.record_failure(ip(5));
        assert!(!throttle.allow(ip(5)));
        assert!(throttle.allow(ip(6)));
    }

    #[test]
    fn stale_entries_are_pruned() {
        let throttle =
            Throttle::with_limits(10, Duration::from_millis(10), Duration::from_secs(60));
        throttle.record_failure(ip(7));
        assert_eq!(throttle.entries().len(), 1);
        std::thread::sleep(Duration::from_millis(30));
        throttle.record_failure(ip(8));
        assert_eq!(throttle.entries().len(), 1);
        assert!(throttle.entries().contains_key(&ip(8)));
    }
}
