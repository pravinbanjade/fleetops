//! Teams (teams spec): groups of users that carry environment access.
//! Team grants count toward access whatever the license state; without a
//! license, only changes that add access are refused.

pub mod db;
pub mod grants;

use fleetops_core::{Error, Result};

pub const NAME_MAX: usize = 64;
pub const DESCRIPTION_MAX: usize = 256;

/// The trimmed name, or why it is not allowed.
pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > NAME_MAX {
        return Err(Error::Invalid(format!(
            "team name must be 1-{NAME_MAX} characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
    {
        return Err(Error::Invalid(
            "team name may only contain letters, digits, spaces, '_', '-' and '.'".into(),
        ));
    }
    Ok(name.to_string())
}

/// The trimmed description; empty is none.
pub fn validate_description(d: Option<&str>) -> Result<Option<String>> {
    let Some(d) = d.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(None);
    };
    if d.len() > DESCRIPTION_MAX {
        return Err(Error::Invalid(format!(
            "description must be at most {DESCRIPTION_MAX} characters"
        )));
    }
    Ok(Some(d.to_string()))
}
