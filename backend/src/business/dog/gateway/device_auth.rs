//! Credential check for `GET /api/dogGateway/auth/{deviceId}`.
//!
//! A device proves its identity with a time-boxed HMAC signature over its
//! `deviceId`, a Unix timestamp and a random nonce. The shared secret never
//! travels over the wire and the timestamp window bounds replay attacks.
//!
//! Wire format (canonical string):
//!
//! ```text
//! {device_id}\n{timestamp}\n{nonce}
//! ```
//!
//! The signature is `hex(HMAC-SHA256(secret, canonical))`, sent in headers:
//!
//! ```text
//! X-Device-Timestamp: 1735689600
//! X-Device-Nonce:     3f8a...
//! X-Device-Signature: 9c1e...
//! ```

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

const DEVICE_TIMESTAMP_HEADER: &str = "x-device-timestamp";
const DEVICE_NONCE_HEADER: &str = "x-device-nonce";
const DEVICE_SIGNATURE_HEADER: &str = "x-device-signature";

/// How far a device clock may drift from the server, in seconds. Both
/// directions are bounded so a captured request cannot be replayed later.
pub const SIGNATURE_VALIDITY_SECS: i64 = 300;

/// Nonces are remembered for this long to reject duplicate requests.
const NONCE_RETENTION_SECS: i64 = SIGNATURE_VALIDITY_SECS * 2;

/// Upper bound on tracked nonces so a flood cannot exhaust memory.
const MAX_TRACKED_NONCES: usize = 100_000;

type HmacSha256 = Hmac<Sha256>;

/// Canonical string that both sides sign.
pub fn canonical_request(device_id: &str, timestamp: i64, nonce: &str) -> String {
    format!("{device_id}\n{timestamp}\n{nonce}")
}

/// `hex(HMAC-SHA256(secret, canonical))`.
pub fn sign(secret: &str, device_id: &str, timestamp: i64, nonce: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(canonical_request(device_id, timestamp, nonce).as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

/// Headers parsed from a device token request.
#[derive(Debug, Clone)]
pub struct DeviceAuthHeaders {
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceAuthError {
    MissingHeader(&'static str),
    MalformedTimestamp,
    MalformedNonce,
    EmptySignature,
}

impl std::fmt::Display for DeviceAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader(name) => write!(formatter, "missing `{name}` header"),
            Self::MalformedTimestamp => {
                write!(
                    formatter,
                    "`{DEVICE_TIMESTAMP_HEADER}` must be a Unix timestamp"
                )
            }
            Self::MalformedNonce => write!(
                formatter,
                "`{DEVICE_NONCE_HEADER}` must be 8-64 bytes of `[A-Za-z0-9_-]`"
            ),
            Self::EmptySignature => write!(formatter, "`{DEVICE_SIGNATURE_HEADER}` is empty"),
        }
    }
}

impl DeviceAuthHeaders {
    /// Parse the three authentication headers from a request.
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Result<Self, DeviceAuthError> {
        let timestamp = header_value(headers, DEVICE_TIMESTAMP_HEADER)
            .ok_or(DeviceAuthError::MissingHeader(DEVICE_TIMESTAMP_HEADER))?
            .parse::<i64>()
            .map_err(|_| DeviceAuthError::MalformedTimestamp)?;

        let nonce = header_value(headers, DEVICE_NONCE_HEADER)
            .ok_or(DeviceAuthError::MissingHeader(DEVICE_NONCE_HEADER))?
            .to_string();
        if !is_valid_nonce(&nonce) {
            return Err(DeviceAuthError::MalformedNonce);
        }

        let signature = header_value(headers, DEVICE_SIGNATURE_HEADER)
            .ok_or(DeviceAuthError::MissingHeader(DEVICE_SIGNATURE_HEADER))?
            .to_string();
        if signature.is_empty() {
            return Err(DeviceAuthError::EmptySignature);
        }

        Ok(Self {
            timestamp,
            nonce,
            signature,
        })
    }
}

fn header_value<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn is_valid_nonce(nonce: &str) -> bool {
    (8..=64).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Verifies device signatures and remembers seen nonces.
pub struct DeviceAuthenticator {
    secret: String,
    /// `nonce -> expires_at` (Unix seconds).
    seen_nonces: Mutex<HashMap<String, i64>>,
}

impl DeviceAuthenticator {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            seen_nonces: Mutex::new(HashMap::new()),
        }
    }

    /// Validate a device token request.
    ///
    /// Returns `Err` with a client-facing reason when the credential is
    /// missing, stale, replayed or does not match the secret.
    pub fn verify(&self, device_id: &str, headers: &DeviceAuthHeaders) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let skew = (now - headers.timestamp).abs();
        if skew > SIGNATURE_VALIDITY_SECS {
            return Err(format!(
                "`{DEVICE_TIMESTAMP_HEADER}` is outside the ±{SIGNATURE_VALIDITY_SECS}s window"
            ));
        }

        let expected = sign(&self.secret, device_id, headers.timestamp, &headers.nonce);
        // Constant-time comparison to avoid a timing side channel.
        if expected
            .as_bytes()
            .ct_eq(headers.signature.to_ascii_lowercase().as_bytes())
            .unwrap_u8()
            != 1
        {
            return Err("invalid device signature".to_string());
        }

        self.remember_nonce(&headers.nonce, now)
    }

    fn remember_nonce(&self, nonce: &str, now: i64) -> Result<(), String> {
        let mut nonces = self
            .seen_nonces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        nonces.retain(|_, expires_at| *expires_at > now);
        if nonces.contains_key(nonce) {
            return Err("`x-device-nonce` was already used".to_string());
        }
        if nonces.len() >= MAX_TRACKED_NONCES
            && let Some(oldest) = nonces
                .iter()
                .min_by_key(|(_, expires_at)| **expires_at)
                .map(|(key, _)| key.clone())
        {
            nonces.remove(&oldest);
        }

        nonces.insert(nonce.to_string(), now + NONCE_RETENTION_SECS);
        Ok(())
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "device-shared-secret";

    fn headers(device_id: &str, timestamp: i64, nonce: &str, secret: &str) -> DeviceAuthHeaders {
        DeviceAuthHeaders {
            timestamp,
            nonce: nonce.to_string(),
            signature: sign(secret, device_id, timestamp, nonce),
        }
    }

    #[test]
    fn signature_is_stable_and_hex_encoded() {
        let signature = sign(SECRET, "dog-1", 1_735_689_600, "nonce1234");
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(signature, sign(SECRET, "dog-1", 1_735_689_600, "nonce1234"));
    }

    #[test]
    fn signature_binds_every_input() {
        let base = sign(SECRET, "dog-1", 1_735_689_600, "nonce1234");
        assert_ne!(base, sign(SECRET, "dog-2", 1_735_689_600, "nonce1234"));
        assert_ne!(base, sign(SECRET, "dog-1", 1_735_689_601, "nonce1234"));
        assert_ne!(base, sign(SECRET, "dog-1", 1_735_689_600, "nonce5678"));
        assert_ne!(
            base,
            sign("other-secret", "dog-1", 1_735_689_600, "nonce1234")
        );
    }

    #[test]
    fn accepts_a_fresh_signature() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let now = Utc::now().timestamp();
        let request = headers("dog-1", now, "nonce1234", SECRET);
        assert!(authenticator.verify("dog-1", &request).is_ok());
    }

    #[test]
    fn rejects_a_signature_for_another_device() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let now = Utc::now().timestamp();
        let request = headers("dog-1", now, "nonce1234", SECRET);
        let error = authenticator.verify("dog-2", &request).unwrap_err();
        assert!(error.contains("invalid device signature"), "got: {error}");
    }

    #[test]
    fn rejects_a_wrong_secret() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let now = Utc::now().timestamp();
        let request = headers("dog-1", now, "nonce1234", "guessed-secret");
        assert!(authenticator.verify("dog-1", &request).is_err());
    }

    #[test]
    fn rejects_a_stale_timestamp() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let stale = Utc::now().timestamp() - SIGNATURE_VALIDITY_SECS - 10;
        let request = headers("dog-1", stale, "nonce1234", SECRET);
        let error = authenticator.verify("dog-1", &request).unwrap_err();
        assert!(error.contains("outside"), "got: {error}");
    }

    #[test]
    fn rejects_a_future_timestamp_beyond_the_window() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let future = Utc::now().timestamp() + SIGNATURE_VALIDITY_SECS + 10;
        let request = headers("dog-1", future, "nonce1234", SECRET);
        assert!(authenticator.verify("dog-1", &request).is_err());
    }

    #[test]
    fn rejects_a_replayed_nonce() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let now = Utc::now().timestamp();
        let request = headers("dog-1", now, "nonce1234", SECRET);

        assert!(authenticator.verify("dog-1", &request).is_ok());
        let error = authenticator.verify("dog-1", &request).unwrap_err();
        assert!(error.contains("already used"), "got: {error}");
    }

    #[test]
    fn accepts_a_new_nonce_for_the_same_device() {
        let authenticator = DeviceAuthenticator::new(SECRET);
        let now = Utc::now().timestamp();
        assert!(
            authenticator
                .verify("dog-1", &headers("dog-1", now, "nonceaaaa", SECRET))
                .is_ok()
        );
        assert!(
            authenticator
                .verify("dog-1", &headers("dog-1", now, "noncebbbb", SECRET))
                .is_ok()
        );
    }

    #[test]
    fn nonce_format_is_enforced() {
        assert!(is_valid_nonce("abcd1234"));
        assert!(!is_valid_nonce("short"));
        assert!(!is_valid_nonce("has space1"));
        assert!(!is_valid_nonce("has/slash1"));
        assert!(!is_valid_nonce(&"a".repeat(65)));
    }
}
