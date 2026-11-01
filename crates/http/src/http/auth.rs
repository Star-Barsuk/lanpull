//! Authentication: credentials in, principal out.
//!
//! The HTTP layer only knows the [`Authenticator`] trait; the lanpull
//! implementation verifies HTTP Basic credentials against the account file.
//! Password verification is deliberately expensive (argon2), so a successful
//! check is cached for a short window keyed by the password digest and the
//! current stored hash; a rotation or a flush invalidates it immediately.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::http::{header, HeaderMap};
use base64::Engine as _;

use lanpull_core::clients::{self, Account};
use lanpull_core::config::Config;
use lanpull_core::hash::sha256_hex;
use lanpull_core::live::LiveCache;
use lanpull_core::throttle::Throttle;
use lanpull_core::timeutil;

/// A syntactically valid argon2id PHC hash used to equalize the work of an
/// unknown-account check. It never matches a real password; `verify_password`
/// still runs the full argon2 routine, so the cost is independent of whether
/// the account exists.
const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$rn66er0ZraW8U6cAQFyp0w$ERZFdPrADf5JiG8ZH21LS6xGMhWXf+9XlSqwkLQFnUU";

/// How long a successful password verification is reused.
const CACHE_TTL: Duration = Duration::from_secs(30);
/// Upper bound on cached verifications.
const CACHE_CAP: usize = 4096;

/// The authenticated account attached to a request.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Account name.
    pub name: String,
}

/// Credentials decoded from the `Authorization` header.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Account name.
    pub user: String,
    /// Plaintext password, held only for the duration of the check.
    pub password: String,
}

/// Why a request was not authenticated.
///
/// The reason is exposed as a header for the operator-facing client. An
/// unknown account and a wrong password deliberately share one reason: telling
/// them apart would let an unauthenticated caller enumerate accounts, and
/// would defeat the equal-cost dummy-hash work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No credentials were sent.
    Missing,
    /// The account is unknown or the password is wrong.
    Credentials,
    /// The account is bound to a different source address.
    ForeignIp,
    /// The account is not armed.
    NotArmed,
    /// Too many recent failures from this source address.
    RateLimited,
}

impl AuthError {
    /// The machine-readable reason sent to the client and written to the log.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Missing | Self::Credentials => "auth_failed",
            Self::ForeignIp => "foreign_ip",
            Self::NotArmed => "not_armed",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// Verifies credentials for a source address.
///
/// Implementations are called from a blocking worker, so they may perform
/// filesystem access and CPU-bound work.
pub trait Authenticator: Send + Sync {
    /// Return the authenticated principal, or the reason for the refusal.
    fn authenticate(
        &self,
        credentials: Option<Credentials>,
        peer: IpAddr,
    ) -> Result<Principal, AuthError>;
}

/// One cached successful verification.
#[derive(Debug)]
struct Cached {
    /// The stored PHC hash the password was verified against.
    hash: String,
    /// When the verification happened.
    at: Instant,
}

/// The account-file implementation of [`Authenticator`].
pub(super) struct LanpullAuthenticator {
    /// Server configuration.
    config: Arc<Config>,
    /// Live cache of accounts and arm state.
    live: Arc<LiveCache>,
    /// Failed-authentication throttle.
    throttle: Arc<Throttle>,
    /// Hash used to equalize an unknown-user check.
    dummy_hash: String,
    /// Successful password verifications, bounded and short-lived.
    verified: Mutex<BTreeMap<(String, String), Cached>>,
}

impl std::fmt::Debug for LanpullAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanpullAuthenticator")
            .finish_non_exhaustive()
    }
}

impl LanpullAuthenticator {
    /// Build an authenticator over a configuration and its live cache.
    pub(super) fn new(config: Arc<Config>, live: Arc<LiveCache>) -> Self {
        Self {
            config,
            live,
            throttle: Arc::new(Throttle::new()),
            dummy_hash: DUMMY_PASSWORD_HASH.to_string(),
            verified: Mutex::new(BTreeMap::new()),
        }
    }

    /// Return whether the password matches the account, using the short cache.
    fn password_ok(&self, account: &Account, password: &str) -> bool {
        let key = (account.name.clone(), sha256_hex(password.as_bytes()));
        let now = Instant::now();
        {
            let mut cache = self.cache();
            if let Some(entry) = cache.get(&key) {
                if entry.hash == account.hash && now.duration_since(entry.at) < CACHE_TTL {
                    return true;
                }
                cache.remove(&key);
            }
        }
        if !clients::verify_password(&account.hash, password) {
            return false;
        }
        let mut cache = self.cache();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(
            key,
            Cached {
                hash: account.hash.clone(),
                at: now,
            },
        );
        true
    }

    /// Lock the verification cache, recovering from a poisoned mutex.
    fn cache(&self) -> MutexGuard<'_, BTreeMap<(String, String), Cached>> {
        self.verified.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Authenticator for LanpullAuthenticator {
    fn authenticate(
        &self,
        credentials: Option<Credentials>,
        peer: IpAddr,
    ) -> Result<Principal, AuthError> {
        if !self.throttle.allow(peer) {
            return Err(AuthError::RateLimited);
        }
        let Some(credentials) = credentials else {
            return Err(AuthError::Missing);
        };
        let clients = match self.live.clients(&self.config.clients_path) {
            Ok(clients) => clients,
            Err(e) => {
                tracing::warn!(error = %e, "cannot read account file");
                return Err(AuthError::Credentials);
            }
        };
        let Some(account) = clients.get(&credentials.user) else {
            // Verify against a dummy hash so an unknown user costs the same as
            // a wrong password and cannot be distinguished by timing.
            let _ = clients::verify_password(&self.dummy_hash, &credentials.password);
            self.throttle.record_failure(peer);
            return Err(AuthError::Credentials);
        };
        if !self.password_ok(account, &credentials.password) {
            self.throttle.record_failure(peer);
            return Err(AuthError::Credentials);
        }
        if let Some(allowed) = account.allowed_ip {
            if allowed != peer {
                return Err(AuthError::ForeignIp);
            }
        }
        if !account.local {
            let armed = match self.live.arm(&self.config.arm_path()) {
                Ok(arm) => arm.is_armed(&account.name, timeutil::now_unix()),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot read arm state");
                    false
                }
            };
            if !armed {
                return Err(AuthError::NotArmed);
            }
        }
        self.throttle.record_success(peer);
        Ok(Principal {
            name: account.name.clone(),
        })
    }
}

/// Decode HTTP Basic credentials.
pub(super) fn basic_credentials(headers: &HeaderMap) -> Option<Credentials> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some(Credentials {
        user: user.to_string(),
        password: password.to_string(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::missing_assert_message)]

    use super::*;

    fn header(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value.parse().unwrap());
        headers
    }

    #[test]
    fn basic_credentials_round_trip() {
        let credentials = basic_credentials(&header("Basic YWxwaGE6c2VjcmV0")).unwrap();
        assert_eq!(credentials.user, "alpha");
        assert_eq!(credentials.password, "secret");
    }

    #[test]
    fn basic_credentials_reject_other_schemes() {
        assert!(basic_credentials(&header("Bearer token")).is_none());
        assert!(basic_credentials(&HeaderMap::new()).is_none());
    }

    #[test]
    fn errors_share_one_credential_reason() {
        assert_eq!(AuthError::Missing.reason(), "auth_failed");
        assert_eq!(AuthError::Credentials.reason(), "auth_failed");
        assert_ne!(AuthError::ForeignIp.reason(), "auth_failed");
        assert_eq!(AuthError::NotArmed.reason(), "not_armed");
        assert_eq!(AuthError::RateLimited.reason(), "rate_limited");
    }
}
