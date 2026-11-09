//! Local ROS map package hashing used for task dispatch verification.
//!
//! The server freezes a map baseline on every route and dispatches its
//! `mapHash` with the task. The device compares that value with the hash of its
//! local map package before running the mission.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::ClientError;

/// SHA-256 of in-memory bytes, lowercase hex.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// SHA-256 of a local map package, lowercase hex.
pub fn sha256_file(path: impl AsRef<Path>) -> Result<String, ClientError> {
    let bytes = std::fs::read(path.as_ref()).map_err(|error| {
        ClientError::Map(format!(
            "failed to read map package {}: {error}",
            path.as_ref().display()
        ))
    })?;
    Ok(sha256_bytes(&bytes))
}

/// Case-insensitive comparison of two hex digests.
pub fn hash_matches(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hash_comparison_ignores_case_and_whitespace() {
        assert!(hash_matches(" ABC123 ", "abc123"));
        assert!(!hash_matches("abc123", "abc124"));
    }
}
