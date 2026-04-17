use std::fmt;

#[derive(Debug, Clone)]
pub struct Email(String);

impl Email {
    pub fn new(email: &str) -> Result<Self, String> {
        if email.is_empty() {
            return Err("Email cannot be empty".to_string());
        }
        if !email.contains('@') {
            return Err("Email must contain @".to_string());
        }
        Ok(Email(email.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
