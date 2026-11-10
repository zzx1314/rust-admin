//! Device credential signing for `GET /api/dogGateway/auth/{deviceId}`.
//!
//! The server only issues a device token when the request carries a
//! time-boxed HMAC-SHA256 signature over `deviceId`, a Unix timestamp and a
//! random nonce:
//!
//! ```text
//! canonical = "{device_id}\n{timestamp}\n{nonce}"
//! signature = hex(HMAC-SHA256(shared_secret, canonical))
//! ```
//!
//! Sent as `X-Device-Timestamp`, `X-Device-Nonce` and `X-Device-Signature`.
//! Both sides must clamp their clocks to within a few minutes.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Number of random bytes in a nonce (hex encoded to 64 characters).
const NONCE_BYTES: usize = 16;

/// A signed request credential, ready to be attached to an HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSignature {
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}

impl DeviceSignature {
    /// Sign `device_id` with `secret` using the supplied timestamp/nonce.
    ///
    /// Exposed mainly so tests can build a signature for a fixed timestamp.
    pub fn with_nonce(secret: &str, device_id: &str, timestamp: i64, nonce: &str) -> Self {
        Self {
            timestamp,
            nonce: nonce.to_string(),
            signature: sign(secret, device_id, timestamp, nonce),
        }
    }

    /// Sign `device_id` with `secret`, using the current time and a fresh
    /// random nonce.
    pub fn sign_now(secret: &str, device_id: &str) -> Self {
        let timestamp = chrono::Utc::now().timestamp();
        let nonce = hex_encode(&uuid::Uuid::new_v4().into_bytes()[..NONCE_BYTES]);
        Self::with_nonce(secret, device_id, timestamp, &nonce)
    }

    /// Convenience: `[("X-Device-Timestamp", ..), ...]` pairs for a request.
    pub fn headers(&self) -> [(&'static str, String); 3] {
        [
            ("X-Device-Timestamp", self.timestamp.to_string()),
            ("X-Device-Nonce", self.nonce.clone()),
            ("X-Device-Signature", self.signature.clone()),
        ]
    }
}

/// Canonical string covered by the signature.
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

    #[test]
    fn signature_is_deterministic_hex() {
        let signature = sign("secret", "dog-1", 1_735_689_600, "nonce1234");
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            signature,
            sign("secret", "dog-1", 1_735_689_600, "nonce1234")
        );
    }

    #[test]
    fn signature_depends_on_all_inputs() {
        let base = sign("secret", "dog-1", 1_735_689_600, "nonce1234");
        assert_ne!(base, sign("secret", "dog-2", 1_735_689_600, "nonce1234"));
        assert_ne!(base, sign("secret", "dog-1", 1_735_689_601, "nonce1234"));
        assert_ne!(base, sign("secret", "dog-1", 1_735_689_600, "nonce5678"));
        assert_ne!(base, sign("other", "dog-1", 1_735_689_600, "nonce1234"));
    }

    #[test]
    fn sign_now_produces_unique_nonces() {
        let first = DeviceSignature::sign_now("secret", "dog-1");
        let second = DeviceSignature::sign_now("secret", "dog-1");
        assert_ne!(first.nonce, second.nonce);
        assert_eq!(first.nonce.len(), NONCE_BYTES * 2);
    }

    #[test]
    fn headers_carry_all_three_fields() {
        let signature = DeviceSignature::with_nonce("secret", "dog-1", 42, "nonce1234");
        let headers = signature.headers();
        assert_eq!(headers[0], ("X-Device-Timestamp", "42".to_string()));
        assert_eq!(headers[1], ("X-Device-Nonce", "nonce1234".to_string()));
        assert_eq!(headers[2].0, "X-Device-Signature");
    }
}
