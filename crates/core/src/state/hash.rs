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

#[cfg(test)]
mod tests {
    // Tests may unwrap and use bare asserts for brevity; production code may not.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )]
    use super::*;
    use std::io::Write;

    #[test]
    fn hex_digest_matches_the_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn file_digest_streams_and_matches_the_byte_digest() {
        let dir = std::env::temp_dir().join(format!("lanpull-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blob");
        let mut file = File::create(&path).unwrap();
        // More than one streaming buffer, so the read loop wraps.
        let payload = vec![0xAB_u8; 200_000];
        file.write_all(&payload).unwrap();
        drop(file);
        let streamed = sha256_file(&path);
        let expected = sha256_hex(&payload);
        assert!(
            matches!(&streamed, Ok(digest) if *digest == expected),
            "the streamed digest must match the byte digest"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
