//! Read-only container file browsing via Docker's archive (tar) API.

use dockerer_core::docker::{DirEntry, DirListing, EntryKind};
use dockerer_core::{Error, Result};
use std::io::Read;

/// A directory listing downloads the whole subtree as a tar stream, so the
/// download is capped; beyond this the listing is marked truncated.
pub const LIST_MAX_BYTES: usize = 64 * 1024 * 1024;
pub const LIST_MAX_ENTRIES: usize = 5_000;

pub fn check_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err(Error::Invalid(format!("path must be absolute: '{path}'")));
    }
    Ok(())
}

fn normalise(raw: &str) -> &str {
    raw.trim_start_matches("./").trim_start_matches('/')
}

/// Name of the top-level tar entry Docker uses for `path`: the last path
/// component, or "" for `/`.
fn root_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn kind_of(t: tar::EntryType) -> EntryKind {
    match t {
        tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::Link => {
            EntryKind::File
        }
        tar::EntryType::Directory => EntryKind::Dir,
        tar::EntryType::Symlink => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

/// Lists the direct children of `path` from the archive Docker returned.
/// `stream_truncated` means the download stopped at `LIST_MAX_BYTES`, so a
/// parse error at the tail is expected and not fatal.
pub fn parse_listing(tar_bytes: &[u8], path: &str, stream_truncated: bool) -> Result<DirListing> {
    let root = root_name(path);
    let mut entries = Vec::new();
    let mut truncated = stream_truncated;
    let mut archive = tar::Archive::new(tar_bytes);
    let iter = archive
        .entries()
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    for (i, entry) in iter.enumerate() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) if stream_truncated => break,
            Err(e) => return Err(Error::Internal(format!("reading archive: {e}"))),
        };
        let header = entry.header();
        let raw = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let norm = normalise(&raw);
        if i == 0
            && header.entry_type() != tar::EntryType::Directory
            && norm.trim_end_matches('/') == root
        {
            return Err(Error::Invalid(format!("'{path}' is not a directory")));
        }
        let rel = if root.is_empty() {
            norm
        } else {
            match norm.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
                Some(r) => r,
                None => continue,
            }
        };
        let rel = rel.trim_end_matches('/');
        if rel.is_empty() || rel.contains('/') {
            continue;
        }
        if entries.len() >= LIST_MAX_ENTRIES {
            truncated = true;
            break;
        }
        entries.push(DirEntry {
            name: rel.to_string(),
            kind: kind_of(header.entry_type()),
            size: header.size().unwrap_or(0),
            mode: header.mode().unwrap_or(0),
            modified: header.mtime().unwrap_or(0) as i64,
            link_target: entry
                .link_name()
                .ok()
                .flatten()
                .map(|l| l.to_string_lossy().into_owned()),
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Dir, &a.name).cmp(&(b.kind != EntryKind::Dir, &b.name))
    });
    Ok(DirListing {
        path: path.to_string(),
        entries,
        truncated,
    })
}

/// Extracts the single file Docker returned for `path`.
pub fn extract_file(tar_bytes: &[u8], path: &str) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(tar_bytes);
    let mut entries = archive
        .entries()
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    let mut entry = entries
        .next()
        .ok_or_else(|| Error::NotFound(format!("'{path}' not found")))?
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    match entry.header().entry_type() {
        tar::EntryType::Regular | tar::EntryType::Continuous => {}
        tar::EntryType::Directory => {
            return Err(Error::Invalid(format!("'{path}' is a directory")))
        }
        _ => return Err(Error::Invalid(format!("'{path}' is not a regular file"))),
    }
    let mut out = Vec::new();
    entry
        .read_to_end(&mut out)
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn tar_of(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, kind, data, link) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(1_700_000_000);
            if let Some(l) = link {
                h.set_link_name(l).unwrap();
            }
            // Write the name bytes directly: Docker emits absolute names
            // (e.g. "/bin/") for an archive of "/", which set_path rejects.
            let name = &mut h.as_gnu_mut().unwrap().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    use tar::EntryType::{Directory as D, Regular as F, Symlink as L};

    #[test]
    fn lists_direct_children_dirs_first() {
        let t = tar_of(&[
            ("etc/", D, b"", None),
            ("etc/passwd", F, b"root", None),
            ("etc/apk/", D, b"", None),
            ("etc/apk/arch", F, b"x", None),
            ("etc/localtime", L, b"", Some("/usr/share/zoneinfo/UTC")),
        ]);
        let l = parse_listing(&t, "/etc", false).unwrap();
        let names: Vec<_> = l
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect();
        assert_eq!(
            names,
            vec![
                ("apk", EntryKind::Dir),
                ("localtime", EntryKind::Symlink),
                ("passwd", EntryKind::File)
            ]
        );
        assert_eq!(l.entries[2].size, 4);
        assert_eq!(
            l.entries[1].link_target.as_deref(),
            Some("/usr/share/zoneinfo/UTC")
        );
        assert!(!l.truncated);
    }

    #[test]
    fn lists_root_directory() {
        let t = tar_of(&[
            ("/", D, b"", None),
            ("/.dockerenv", F, b"", None),
            ("/bin/", D, b"", None),
            ("/bin/sh", F, b"", None),
        ]);
        let l = parse_listing(&t, "/", false).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bin", ".dockerenv"]);
    }

    #[test]
    fn trailing_slash_in_request_is_ignored() {
        let t = tar_of(&[("etc/", D, b"", None), ("etc/hosts", F, b"", None)]);
        assert_eq!(parse_listing(&t, "/etc/", false).unwrap().entries.len(), 1);
    }

    #[test]
    fn listing_a_file_is_invalid() {
        let t = tar_of(&[("hostname", F, b"abc", None)]);
        assert!(matches!(
            parse_listing(&t, "/etc/hostname", false),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn truncated_stream_returns_partial_listing() {
        let mut t = tar_of(&[
            ("etc/", D, b"", None),
            ("etc/a", F, b"1", None),
            ("etc/b", F, &[0u8; 4096], None),
        ]);
        t.truncate(t.len() - 3000);
        let l = parse_listing(&t, "/etc", true).unwrap();
        assert!(l.truncated);
        assert!(l.entries.iter().any(|e| e.name == "a"));
    }

    #[test]
    fn entry_cap_marks_truncated() {
        let names: Vec<String> = (0..LIST_MAX_ENTRIES + 5)
            .map(|i| format!("d/f{i}"))
            .collect();
        let mut specs: Vec<(&str, tar::EntryType, &[u8], Option<&str>)> =
            vec![("d/", D, b"", None)];
        specs.extend(names.iter().map(|n| (n.as_str(), F, &b""[..], None)));
        let l = parse_listing(&tar_of(&specs), "/d", false).unwrap();
        assert_eq!(l.entries.len(), LIST_MAX_ENTRIES);
        assert!(l.truncated);
    }

    #[test]
    fn extract_file_reads_contents_and_rejects_dirs() {
        assert_eq!(
            extract_file(&tar_of(&[("hostname", F, b"box\n", None)]), "/etc/hostname").unwrap(),
            b"box\n"
        );
        assert!(matches!(
            extract_file(&tar_of(&[("etc/", D, b"", None)]), "/etc"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            extract_file(&tar_of(&[("l", L, b"", Some("/x"))]), "/l"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn paths_must_be_absolute() {
        assert!(check_path("/etc").is_ok());
        assert!(check_path("etc").is_err());
        assert!(check_path("/a\0b").is_err());
    }
}
