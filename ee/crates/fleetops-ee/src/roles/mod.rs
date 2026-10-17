//! Custom roles (custom roles spec): grid permissions granted to users and
//! teams like a built-in role. Grants count toward access whatever the
//! license state; without a license, only changes that add access are refused.

pub mod db;
pub mod grants;

use fleetops_core::{Error, Result};

/// The trimmed name, or why it is not allowed: team-name rules, and never a
/// built-in role's name.
pub fn validate_name(name: &str) -> Result<String> {
    let name = crate::teams::validate_name(name)
        .map_err(|e| Error::Invalid(e.detail().replace("team name", "role name")))?;
    if ["viewer", "operator"].contains(&name.to_lowercase().as_str()) {
        return Err(Error::Invalid("that name is a built-in role".into()));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_team_rules_and_skip_builtins() {
        assert_eq!(validate_name("  Deployer ").unwrap(), "Deployer");
        for bad in ["Viewer", "operator", "", "a/b"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert_eq!(
            validate_name("OPERATOR").unwrap_err().to_string(),
            "invalid input: that name is a built-in role"
        );
    }
}
