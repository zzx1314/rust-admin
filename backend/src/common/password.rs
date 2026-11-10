//! Password hashing.
//!
//! New passwords are stored as bcrypt hashes (`$2b$...`). Existing rows are
//! still unsalted MD5, so verification accepts both formats and the caller can
//! transparently upgrade a legacy hash after a successful login — see
//! [`verify`] and [`needs_rehash`].
//!
//! MD5 support exists purely for the migration window; once every account has
//! logged in at least once, the legacy branch can be deleted.

use std::str::FromStr;

use md5::{Digest, Md5};

/// Cost used for new hashes. 12 is a reasonable default for server hardware
/// (~250 ms in release builds) and stays well under bcrypt's 72-byte limit.
const BCRYPT_COST: u32 = 12;

/// bcrypt only consumes the first 72 bytes of the input; anything longer is
/// silently truncated by the algorithm. We reject it instead so that two
/// different long passwords can never collapse into the same hash.
const MAX_PASSWORD_BYTES: usize = 72;

const LEGACY_MD5_PREFIX: &str = "{MD5}";

/// Hash a plaintext password for storage.
pub fn hash(password: &str) -> Result<String, String> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(format!(
            "password must be at most {MAX_PASSWORD_BYTES} bytes"
        ));
    }
    bcrypt::hash(password, BCRYPT_COST).map_err(|error| format!("bcrypt hash failed: {error}"))
}

/// Lower-level MD5 digest, kept for the legacy migration path and for tests
/// that need to seed pre-migration rows.
pub fn md5_digest(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Verify a password against a stored hash of either format.
///
/// * `$2a$` / `$2b$` / `$2y$` → bcrypt (constant-time comparison inside the
///   `bcrypt` crate)
/// * anything else → legacy unsalted MD5, with or without the `{MD5}` prefix
pub fn verify(password: &str, stored_hash: &str) -> bool {
    if is_bcrypt_hash(stored_hash) {
        return bcrypt::verify(password, stored_hash).unwrap_or(false);
    }

    let expected = stored_hash
        .strip_prefix(LEGACY_MD5_PREFIX)
        .unwrap_or(stored_hash);
    if expected.is_empty() {
        return false;
    }
    constant_time_eq(md5_digest(password).as_bytes(), expected.as_bytes())
}

/// `true` when the stored hash is not a current bcrypt hash, i.e. the caller
/// should re-hash the password after a successful login.
pub fn needs_rehash(stored_hash: &str) -> bool {
    if !is_bcrypt_hash(stored_hash) {
        return true;
    }
    match bcrypt::HashParts::from_str(stored_hash) {
        Ok(parts) => parts.get_cost() < BCRYPT_COST,
        Err(_) => true,
    }
}

fn is_bcrypt_hash(stored_hash: &str) -> bool {
    let bytes = stored_hash.as_bytes();
    bytes.len() == 60
        && bytes[0] == b'$'
        && matches!(bytes[1], b'2')
        && matches!(bytes[2], b'a' | b'b' | b'y')
        && bytes[3] == b'$'
}

/// Byte-wise comparison whose running time depends only on the length.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcrypt_round_trip() {
        let hash = hash("correct horse battery staple").expect("hash");
        assert!(is_bcrypt_hash(&hash), "unexpected hash format: {hash}");
        assert!(verify("correct horse battery staple", &hash));
        assert!(!verify("wrong password", &hash));
    }

    #[test]
    fn bcrypt_salts_are_unique() {
        let first = hash("same-password").unwrap();
        let second = hash("same-password").unwrap();
        assert_ne!(first, second, "bcrypt must use a per-hash salt");
        assert!(verify("same-password", &first));
        assert!(verify("same-password", &second));
    }

    #[test]
    fn fresh_hash_needs_no_rehash() {
        let hash = hash("whatever").unwrap();
        assert!(!needs_rehash(&hash));
    }

    #[test]
    fn legacy_md5_still_verifies_and_asks_for_rehash() {
        let legacy = md5_digest("password123");
        assert!(verify("password123", &legacy));
        assert!(!verify("password124", &legacy));
        assert!(needs_rehash(&legacy), "MD5 must be migrated on next login");
    }

    #[test]
    fn legacy_md5_with_prefix_is_accepted() {
        let legacy = format!("{LEGACY_MD5_PREFIX}{}", md5_digest("password123"));
        assert!(verify("password123", &legacy));
    }

    #[test]
    fn empty_stored_hash_never_matches() {
        assert!(!verify("anything", ""));
        assert!(!verify("anything", LEGACY_MD5_PREFIX));
    }

    #[test]
    fn overlong_password_is_rejected() {
        let too_long = "a".repeat(MAX_PASSWORD_BYTES + 1);
        assert!(hash(&too_long).is_err());
        // Exactly at the limit is fine.
        assert!(hash(&"a".repeat(MAX_PASSWORD_BYTES)).is_ok());
    }

    #[test]
    fn malformed_bcrypt_hash_is_rejected() {
        // Right prefix, wrong length: must not panic, must not match.
        assert!(!verify("password", "$2b$12$tooshort"));
    }

    #[test]
    fn md5_digest_matches_the_known_vector() {
        assert_eq!(
            md5_digest("password123"),
            "482c811da5d5b4bc6d497ffa98491e38"
        );
    }
}
