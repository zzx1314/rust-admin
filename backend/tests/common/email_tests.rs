use x_rust::common::value_objects::Email;

// --- Email Value Object Tests ---

#[test]
fn test_email_new_valid() {
    let email = Email::new("user@example.com").unwrap();
    assert_eq!(email.as_str(), "user@example.com");
}

#[test]
fn test_email_new_empty() {
    let result = Email::new("");
    assert!(result.is_err());
}

#[test]
fn test_email_new_no_at_sign() {
    let result = Email::new("no-at-sign");
    assert!(result.is_err());
}

#[test]
fn test_email_new_minimal_valid() {
    let email = Email::new("a@b").unwrap();
    assert_eq!(email.as_str(), "a@b");
}

#[test]
fn test_email_as_str() {
    let email = Email::new("test@example.com").unwrap();
    assert_eq!(email.as_str(), "test@example.com");
}

#[test]
fn test_email_display() {
    let email = Email::new("user@example.com").unwrap();
    assert_eq!(email.to_string(), "user@example.com");
}

#[test]
fn test_email_clone() {
    let email = Email::new("test@example.com").unwrap();
    let cloned = email.clone();
    assert_eq!(email.as_str(), cloned.as_str());
}
