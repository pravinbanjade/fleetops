use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const USERNAME_MAX: usize = 64;
pub const PASSWORD_MIN: usize = 8;
pub const PASSWORD_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

impl FromStr for Role {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            other => Err(Error::Invalid(format!("unknown role '{other}'"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn validate_username(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > USERNAME_MAX {
        return Err(Error::Invalid(format!(
            "username must be 1-{USERNAME_MAX} characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(Error::Invalid(
            "username may only contain letters, digits, '_', '-' and '.'".into(),
        ));
    }
    Ok(())
}

/// Upper bound keeps attacker-supplied input to the hasher small.
pub fn validate_password(password: &str) -> Result<()> {
    let n = password.chars().count();
    if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&n) {
        return Err(Error::Invalid(format!(
            "password must be {PASSWORD_MIN}-{PASSWORD_MAX} characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_roundtrip() {
        assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
        assert_eq!("user".parse::<Role>().unwrap(), Role::User);
        assert_eq!(Role::Admin.as_str(), "admin");
        assert!(matches!("root".parse::<Role>(), Err(Error::Invalid(_))));
    }

    #[test]
    fn role_serde_is_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        let r: Role = serde_json::from_str("\"user\"").unwrap();
        assert_eq!(r, Role::User);
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("a.b-c_1").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username("ünicode").is_err());
        assert!(validate_username(&"a".repeat(64)).is_ok());
        assert!(validate_username(&"a".repeat(65)).is_err());
    }

    #[test]
    fn password_length_bounds() {
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password(&"x".repeat(256)).is_ok());
        assert!(validate_password(&"x".repeat(257)).is_err());
    }
}
