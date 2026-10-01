//! The agent's long-lived key, in `<data>/agent.key`, readable by its owner only.

use std::{fs, io, path::Path};

pub const KEY_FILE: &str = "agent.key";

pub fn load_key(dir: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(dir.join(KEY_FILE)) {
        Ok(s) => Ok(Some(s.trim().to_string()).filter(|k| !k.is_empty())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Written to a temporary file and renamed, so a crash never leaves half a key.
pub fn save_key(dir: &Path, key: &str) -> io::Result<()> {
    let tmp = dir.join(format!("{KEY_FILE}.tmp"));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        io::Write::write_all(&mut f, key.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, dir.join(KEY_FILE))
}

/// Fails if the key could not be saved, so a join token is never spent
/// by an agent that would lose the key it gets for it.
pub fn probe_writable(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(".write-probe");
    fs::write(&probe, b"ok")?;
    fs::remove_file(probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_key_loads_back_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_key(dir.path()).unwrap(), None);
        save_key(dir.path(), "fk_abc").unwrap();
        assert_eq!(load_key(dir.path()).unwrap().as_deref(), Some("fk_abc"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_dir_fails_the_probe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores permissions; the test is meaningless there.
        if std::fs::write(dir.path().join("x"), b"x").is_ok() {
            return;
        }
        assert!(probe_writable(dir.path()).is_err());
    }
}
