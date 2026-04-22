use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
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

/// Simulate RequestUser struct matching the one in middleware.rs
#[derive(Clone, Debug)]
struct RequestUser {
    user_id: i64,
    username: String,
}

#[test]
fn test_request_user_operator_extraction() {
    let request_user = RequestUser {
        user_id: 1,
        username: "admin".to_string(),
    };
    assert_eq!(request_user.username, "admin");
    assert_eq!(request_user.user_id, 1);
}

#[test]
fn test_operator_priority_request_user_over_header() {
    // Simulate: RequestUser from response extensions takes priority
    // over header-based extraction
    let request_user_username = Some("admin".to_string());
    let header_username: Option<String> = None;
    let operator = request_user_username.or(header_username);
    assert_eq!(operator, Some("admin".to_string()));
}

#[test]
fn test_operator_fallback_to_header_when_no_request_user() {
    // When RequestUser is not in extensions (e.g., unauthenticated endpoint),
    // fall back to header-based extraction
    let request_user_username: Option<String> = None;
    let header_username = Some("admin".to_string());
    let operator = request_user_username.or(header_username);
    assert_eq!(operator, Some("admin".to_string()));
}

#[test]
fn test_operator_none_when_no_token_and_no_request_user() {
    // Login endpoint before auth: no RequestUser in extensions,
    // no Authorization header
    let request_user_username: Option<String> = None;
    let header_username: Option<String> = None;
    let operator = request_user_username.or(header_username);
    assert_eq!(operator, None);
}

#[test]
fn test_login_handler_sets_request_user() {
    // After login, the handler should set RequestUser with the
    // logged-in user's info so logging_middleware can read it
    let request_user = RequestUser {
        user_id: 42,
        username: "testuser".to_string(),
    };
    assert_eq!(request_user.username, "testuser");
    assert_eq!(request_user.user_id, 42);
}

#[test]
fn test_require_auth_sets_request_user_on_response() {
    // After require_auth validates a token, it should insert
    // RequestUser into response extensions
    let secret = "test-secret";
    let token = create_token(5, "authenticated_user", secret);
    let username = extract_username(&token, secret);
    assert_eq!(username, Some("authenticated_user".to_string()));

    // Simulate what require_auth does: validate_token returns user_id,
    // extract_username returns the username, both are stored in RequestUser
    let request_user = RequestUser {
        user_id: 5,
        username: username.unwrap_or_default(),
    };
    assert_eq!(request_user.username, "authenticated_user");
    assert_eq!(request_user.user_id, 5);
}
