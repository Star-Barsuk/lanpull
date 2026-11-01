//! Per-client accounts.
//!
//! Accounts live in `config/lanpull.clients` as one
//! `name:argon2hash[:allowed_ip]` line per machine. Only the argon2 hash is
//! stored; the plaintext password exists only in the staged client `auth` file.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::Path;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::distr::{Alphanumeric, SampleString};
use rand::RngCore;

use crate::error::{Error, Result};

/// The length of a generated password.
const PASSWORD_LEN: usize = 24;
/// The length of a generated salt in bytes.
const SALT_LEN: usize = 16;
/// Argon2id memory cost in KiB (the crate default, pinned explicitly).
const ARGON2_MEMORY_KIB: u32 = 19_456;
/// Argon2id time cost (iterations).
const ARGON2_ITERATIONS: u32 = 2;
/// Argon2id parallelism.
const ARGON2_PARALLELISM: u32 = 1;

/// Build the pinned Argon2id instance used for hashing.
///
/// The parameters are fixed here rather than taken from `Argon2::default()` so
/// a change in the dependency default cannot silently weaken or alter stored
/// hashes. Verification reads the parameters embedded in the PHC string.
fn argon2_id() -> Result<Argon2<'static>> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|e| Error::Password(e.to_string()))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// One client account.
#[derive(Debug, Clone)]
pub struct Account {
    /// Account name.
    pub name: String,
    /// PHC-format argon2 hash of the account password.
    pub hash: String,
    /// Optional source IP the account is bound to.
    pub allowed_ip: Option<IpAddr>,
    /// Local (loopback) account: exempt from the arm window.
    pub local: bool,
}

/// The outcome of a credential check.
#[derive(Debug, Clone)]
pub enum Verify {
    /// The account exists and the password is correct.
    Ok(Account),
    /// The account exists but the password is wrong.
    BadPassword(Account),
    /// No account has the requested name.
    Unknown,
}

/// The set of client accounts.
#[derive(Debug, Default, Clone)]
pub struct Clients {
    accounts: BTreeMap<String, Account>,
}

impl Clients {
    /// Return an empty account set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load accounts from `path`, returning an empty set when it is absent.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(parse(&text))
    }

    /// Atomically write the accounts to `path` with mode `0600`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut text = String::new();
        for account in self.accounts.values() {
            text.push_str(&account.name);
            text.push(':');
            text.push_str(&account.hash);
            match (account.allowed_ip, account.local) {
                (Some(ip), false) => {
                    text.push(':');
                    text.push_str(&ip.to_string());
                }
                (Some(ip), true) => {
                    text.push(':');
                    text.push_str(&ip.to_string());
                    text.push_str(":local");
                }
                (None, false) => {}
                (None, true) => text.push_str("::local"),
            }
            text.push('\n');
        }
        crate::atomic::write_private(path, text.as_bytes())
    }

    /// Return `true` when there are no accounts.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Look up an account by name.
    pub fn get(&self, name: &str) -> Option<&Account> {
        self.accounts.get(name)
    }

    /// Insert or replace an account.
    pub fn insert(&mut self, account: Account) {
        self.accounts.insert(account.name.clone(), account);
    }

    /// Remove an account, returning `true` when one was present.
    pub fn remove(&mut self, name: &str) -> bool {
        self.accounts.remove(name).is_some()
    }

    /// Iterate over accounts in name order.
    pub fn iter(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    /// Check a password against an account.
    pub fn verify(&self, name: &str, password: &str) -> Verify {
        self.accounts.get(name).map_or(Verify::Unknown, |account| {
            if verify_password(&account.hash, password) {
                Verify::Ok(account.clone())
            } else {
                Verify::BadPassword(account.clone())
            }
        })
    }

    /// Set a new password hash for an existing account.
    pub fn set_hash(&mut self, name: &str, hash: String) -> bool {
        match self.accounts.get_mut(name) {
            Some(account) => {
                account.hash = hash;
                true
            }
            None => false,
        }
    }
}

/// Parse account lines, skipping blanks, comments, and malformed lines.
fn parse(text: &str) -> Clients {
    let mut clients = Clients::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(4, ':');
        let name = match parts.next() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => continue,
        };
        let hash = match parts.next() {
            Some(hash) if !hash.is_empty() => hash.to_string(),
            _ => continue,
        };
        let allowed_ip = match parts.next() {
            None | Some("" | "*") => None,
            Some(field) => match field.parse::<IpAddr>() {
                Ok(ip) => Some(ip),
                Err(_) => continue,
            },
        };
        let local = matches!(parts.next(), Some("local"));
        clients.insert(Account {
            name,
            hash,
            allowed_ip,
            local,
        });
    }
    clients
}

/// Generate a random alphanumeric password.
pub fn generate_password() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), PASSWORD_LEN)
}

/// Hash a password with argon2id, returning a PHC-format string.
pub fn hash_password(password: &str) -> Result<String> {
    let mut salt_bytes = [0_u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| Error::Password(e.to_string()))?;
    let hash = argon2_id()?
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| Error::Password(e.to_string()))?;
    Ok(hash.to_string())
}

/// Verify a password against a PHC-format argon2 hash.
///
/// The cost parameters come from the hash itself, so hashes written by an
/// earlier parameter set still verify.
pub fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    argon2_id().is_ok_and(|argon2| argon2.verify_password(password.as_bytes(), &parsed).is_ok())
}
