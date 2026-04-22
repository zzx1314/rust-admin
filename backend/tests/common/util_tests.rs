use chrono::{TimeZone, Utc};
use x_rust::common::util::{decrypt_password, encrypt_password, format_datetime, format_datetime_opt, md5_encrypt, md5_verify};

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

// ==================== Beijing Time Formatting Tests ====================

#[test]
fn test_format_datetime_utc_to_beijing() {
    // 2025-01-01T00:00:00Z (midnight UTC) = 2025-01-01 08:00:00 Beijing
    let dt = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    assert_eq!(format_datetime(dt), "2025-01-01 08:00:00");
}

#[test]
fn test_format_datetime_utc_afternoon() {
    // 2025-06-15T14:30:45Z = 2025-06-15 22:30:45 Beijing
    let dt = Utc.with_ymd_and_hms(2025, 6, 15, 14, 30, 45).unwrap();
    assert_eq!(format_datetime(dt), "2025-06-15 22:30:45");
}

#[test]
fn test_format_datetime_cross_day_boundary() {
    // 2025-12-31T20:00:00Z = 2026-01-01 04:00:00 Beijing (next day)
    let dt = Utc.with_ymd_and_hms(2025, 12, 31, 20, 0, 0).unwrap();
    assert_eq!(format_datetime(dt), "2026-01-01 04:00:00");
}

#[test]
fn test_format_datetime_opt_some() {
    let dt = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    assert_eq!(format_datetime_opt(Some(dt)), Some("2025-01-01 08:00:00".to_string()));
}

#[test]
fn test_format_datetime_opt_none() {
    assert_eq!(format_datetime_opt(None), None);
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
