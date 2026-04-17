use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use md5::{Digest, Md5};

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const DEFAULT_KEY: [u8; 16] = *b"Welcome Superred";

/// MD5 encrypt: returns lowercase hex digest.
pub fn md5_encrypt(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Verify input against a stored MD5 hash.
/// Supports hashes with or without the "{MD5}" prefix.
pub fn md5_verify(input: &str, md5_hash: &str) -> bool {
    let normalized = md5_hash.strip_prefix("{MD5}").unwrap_or(md5_hash);
    md5_encrypt(input) == normalized
}

/// Decrypt AES-128-CBC ZeroPadding encrypted password (base64-encoded).
///
/// Matches the Java implementation:
/// - Key and IV are the same 16-byte string, encoded as ISO-8859-1 bytes
/// - ZeroPadding (manual trim of trailing 0x00 bytes)
/// - Output is UTF-8 plaintext
pub fn decrypt_password(cipher_text: &str) -> Result<String, String> {
    let decoded = BASE64
        .decode(cipher_text)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    if decoded.len() % 16 != 0 {
        return Err("Invalid ciphertext length: not a multiple of 16".into());
    }

    let mut data = decoded;
    let key = DEFAULT_KEY.into();
    let iv = DEFAULT_KEY.into();

    Aes128CbcDec::new(&key, &iv)
        .decrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut data)
        .map_err(|e| format!("AES decrypt failed: {}", e))?;

    // Trim ZeroPadding: remove trailing 0x00 bytes
    let trim_len = data
        .iter()
        .rev()
        .position(|&b| b != 0)
        .unwrap_or(data.len());
    let plain_len = data.len() - trim_len;

    String::from_utf8(data[..plain_len].to_vec())
        .map_err(|e| format!("Invalid UTF-8 in decrypted data: {}", e))
}

/// Encrypt plaintext with AES-128-CBC ZeroPadding (for test use).
///
/// Matches the Java frontend encryption exactly.
pub fn encrypt_password(plain_text: &str) -> String {
    use aes::Aes128;
    use cbc::cipher::{BlockEncryptMut, block_padding::NoPadding};

    type Aes128CbcEnc = cbc::Encryptor<Aes128>;

    let data = plain_text.as_bytes();
    let block_size = 16;
    // Zero-pad to multiple of 16 (matches Java implementation)
    let padded_len = if data.is_empty() {
        block_size
    } else {
        data.len().div_ceil(block_size) * block_size
    };
    let mut buf = vec![0u8; padded_len];
    buf[..data.len()].copy_from_slice(data);

    let key = DEFAULT_KEY.into();
    let iv = DEFAULT_KEY.into();

    let ct = Aes128CbcEnc::new(&key, &iv)
        .encrypt_padded_mut::<NoPadding>(&mut buf, padded_len)
        .unwrap();

    BASE64.encode(ct)
}
