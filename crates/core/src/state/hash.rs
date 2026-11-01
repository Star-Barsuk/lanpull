//! SHA-256 hashing helpers.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::Result;

/// Size of the streaming hash buffer in bytes.
const BUFFER_SIZE: usize = 64 * 1024;

/// Compute the lowercase hex SHA-256 digest of a byte slice.
///
/// Used for in-memory inputs such as the credential cache key; it never holds a
/// password itself, only a digest used to look up a verification result.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Compute the lowercase hex SHA-256 digest of a file, streaming its contents.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let Some(chunk) = buffer.get(..read) else {
            break;
        };
        hasher.update(chunk);
    }

    Ok(hex::encode(hasher.finalize()))
}
