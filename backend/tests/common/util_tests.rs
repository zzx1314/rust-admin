use x_rust::common::util::{decrypt_password, encrypt_password, md5_encrypt, md5_verify};

// ==================== AES Encrypt/Decrypt Roundtrip Tests ====================

#[test]
fn test_aes_roundtrip_known_value() {
    let plain = "testpassword";
    let cipher_text = encrypt_password(plain);
    let result = decrypt_password(&cipher_text).unwrap();
    assert_eq!(result, plain);
}

#[test]
fn test_aes_roundtrip_short_password() {
    let plain = "hi";
    let cipher_text = encrypt_password(plain);
    let result = decrypt_password(&cipher_text).unwrap();
    assert_eq!(result, plain);
}

#[test]
fn test_aes_roundtrip_empty_password() {
    let plain = "";
    let cipher_text = encrypt_password(plain);
    let result = decrypt_password(&cipher_text).unwrap();
    assert_eq!(result, plain);
}

#[test]
fn test_aes_roundtrip_unicode_password() {
    let plain = "密码123!@#";
    let cipher_text = encrypt_password(plain);
    let result = decrypt_password(&cipher_text).unwrap();
    assert_eq!(result, plain);
}

#[test]
fn test_aes_decrypt_invalid_base64() {
    let result = decrypt_password("not-valid-base64!!!");
    assert!(result.is_err());
}

// ==================== MD5 Tests ====================

#[test]
fn test_md5_encrypt_known_value() {
    // MD5 of "password123" = "482c811da5d5b4bc6d497ffa98491e38"
    assert_eq!(
        md5_encrypt("password123"),
        "482c811da5d5b4bc6d497ffa98491e38"
    );
}

#[test]
fn test_md5_encrypt_empty() {
    // MD5 of "" = "d41d8cd98f00b204e9800998ecf8427e"
    assert_eq!(md5_encrypt(""), "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn test_md5_encrypt_deterministic() {
    let a = md5_encrypt("hello");
    let b = md5_encrypt("hello");
    assert_eq!(a, b);
}

#[test]
fn test_md5_verify_match() {
    assert!(md5_verify(
        "password123",
        "482c811da5d5b4bc6d497ffa98491e38"
    ));
}

#[test]
fn test_md5_verify_mismatch() {
    assert!(!md5_verify("wrong", "482c811da5d5b4bc6d497ffa98491e38"));
}

#[test]
fn test_md5_verify_with_prefix() {
    assert!(md5_verify(
        "password123",
        "{MD5}482c811da5d5b4bc6d497ffa98491e38"
    ));
}

#[test]
fn test_md5_verify_without_prefix() {
    assert!(md5_verify(
        "password123",
        "482c811da5d5b4bc6d497ffa98491e38"
    ));
}
