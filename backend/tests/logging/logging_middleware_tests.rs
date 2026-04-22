use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: i64,
    username: String,
    exp: u64,
    iat: u64,
}

fn create_token(user_id: i64, username: &str, secret: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        exp: now + 3600,
        iat: now,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

fn extract_username(token: &str, secret: &str) -> Option<String> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()
    .map(|data| data.claims.username)
}

#[test]
fn test_extract_username_valid_token() {
    let secret = "test-secret";
    let token = create_token(1, "admin", secret);
    let username = extract_username(&token, secret);
    assert_eq!(username, Some("admin".to_string()));
}

#[test]
fn test_extract_username_invalid_token() {
    let secret = "test-secret";
    let username = extract_username("invalid-token", secret);
    assert_eq!(username, None);
}

#[test]
fn test_extract_username_wrong_secret() {
    let secret = "test-secret";
    let wrong_secret = "wrong-secret";
    let token = create_token(1, "admin", secret);
    let username = extract_username(&token, wrong_secret);
    assert_eq!(username, None);
}
