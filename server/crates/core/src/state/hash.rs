//! SHA-256 hashing helpers.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::Result;

/// Compute the lowercase hex SHA-256 digest of a file, streaming its contents.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).unwrap_or(&buffer);
        hasher.update(chunk);
    }

    Ok(hex::encode(hasher.finalize()))
}
