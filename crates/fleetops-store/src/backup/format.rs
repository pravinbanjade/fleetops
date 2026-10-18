//! The backup file's outer layer: a header naming the KDF parameters, then
//! the payload sealed in 64 KiB AES-256-GCM chunks (the STREAM
//! construction), under a key derived from the passphrase with Argon2id.
//! Every chunk is bound to the header, and the last one is marked, so a
//! truncated or spliced file fails to open.

use aes_gcm::{
    aead::{
        stream::{DecryptorBE32, EncryptorBE32},
        KeyInit, Payload,
    },
    Aes256Gcm,
};
use argon2::{Algorithm, Argon2, Params, Version};
use fleetops_core::{Error, Result};
use rand::RngCore;
use std::io::{Read, Write};

const MAGIC: &[u8; 16] = b"FLEETOPS-BACKUP\n";
const VERSION_BYTE: u8 = 1;
const SALT_LEN: usize = 16;
/// 12-byte GCM nonce less the 4-byte counter and the last-chunk flag.
const NONCE_PREFIX_LEN: usize = 7;
const HEADER_LEN: usize = MAGIC.len() + 1 + 12 + SALT_LEN + NONCE_PREFIX_LEN;
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;

pub const MIN_PASSPHRASE: usize = 12;

/// The most a file may ask the KDF for, so a hostile header can't make the
/// server allocate gigabytes before the passphrase is even checked.
const MAX_M_KIB: u32 = 262_144;
const MAX_T: u32 = 10;
const MAX_P: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    pub const DEFAULT: KdfParams = KdfParams {
        m_kib: 65_536,
        t: 3,
        p: 1,
    };
    /// Cheap parameters for tests.
    #[doc(hidden)]
    pub const TEST: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };
}

pub fn check_passphrase(p: &str) -> Result<()> {
    if p.chars().count() < MIN_PASSPHRASE {
        return Err(Error::Invalid(format!(
            "the passphrase needs at least {MIN_PASSPHRASE} characters"
        )));
    }
    Ok(())
}

pub(crate) fn wrong() -> Error {
    Error::Invalid("wrong passphrase or damaged file".into())
}

pub(crate) fn damaged() -> Error {
    Error::Invalid("damaged backup".into())
}

fn io(e: std::io::Error) -> Error {
    Error::Internal(format!("backup: {e}"))
}

fn derive(passphrase: &str, salt: &[u8], p: KdfParams) -> Result<Aes256Gcm> {
    let params = Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|_| damaged())?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| damaged())?;
    Ok(Aes256Gcm::new(&key.into()))
}

/// Reads until `buf` is full or the input ends; returns how much was read.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io(e)),
        }
    }
    Ok(n)
}

pub fn encrypt(
    mut plain: impl Read,
    mut out: impl Write,
    passphrase: &str,
    params: KdfParams,
) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    let mut prefix = [0u8; NONCE_PREFIX_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut prefix);
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(VERSION_BYTE);
    for v in [params.m_kib, params.t, params.p] {
        header.extend_from_slice(&v.to_le_bytes());
    }
    header.extend_from_slice(&salt);
    header.extend_from_slice(&prefix);
    out.write_all(&header).map_err(io)?;

    let cipher = derive(passphrase, &salt, params)?;
    let mut enc = EncryptorBE32::from_aead(cipher, prefix.as_slice().into());
    let mut cur = vec![0u8; CHUNK];
    let mut n = read_full(&mut plain, &mut cur)?;
    let mut next = vec![0u8; CHUNK];
    loop {
        let m = if n == CHUNK {
            read_full(&mut plain, &mut next)?
        } else {
            0
        };
        let payload = Payload {
            msg: &cur[..n],
            aad: &header,
        };
        if m == 0 {
            let ct = enc.encrypt_last(payload).map_err(|_| wrong())?;
            out.write_all(&ct).map_err(io)?;
            break;
        }
        let ct = enc.encrypt_next(payload).map_err(|_| wrong())?;
        out.write_all(&ct).map_err(io)?;
        std::mem::swap(&mut cur, &mut next);
        n = m;
    }
    out.flush().map_err(io)
}

pub fn decrypt(mut input: impl Read, mut out: impl Write, passphrase: &str) -> Result<()> {
    let mut header = [0u8; HEADER_LEN];
    let n = read_full(&mut input, &mut header)?;
    if n < MAGIC.len() + 1 || &header[..MAGIC.len()] != MAGIC || header[MAGIC.len()] != VERSION_BYTE
    {
        return Err(Error::Invalid("not a FleetOps backup".into()));
    }
    if n < HEADER_LEN {
        return Err(damaged());
    }
    let u32_at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().expect("4 bytes"));
    let base = MAGIC.len() + 1;
    let params = KdfParams {
        m_kib: u32_at(base),
        t: u32_at(base + 4),
        p: u32_at(base + 8),
    };
    if params.m_kib > MAX_M_KIB
        || params.t > MAX_T
        || params.p > MAX_P
        || params.t == 0
        || params.p == 0
    {
        return Err(damaged());
    }
    let salt = &header[base + 12..base + 12 + SALT_LEN];
    let prefix = &header[base + 12 + SALT_LEN..];
    let cipher = derive(passphrase, salt, params)?;
    let mut dec = DecryptorBE32::from_aead(cipher, prefix.into());

    let size = CHUNK + TAG;
    let mut cur = vec![0u8; size];
    let mut n = read_full(&mut input, &mut cur)?;
    let mut next = vec![0u8; size];
    loop {
        let m = if n == size {
            read_full(&mut input, &mut next)?
        } else {
            0
        };
        let payload = Payload {
            msg: &cur[..n],
            aad: &header,
        };
        if m == 0 {
            let pt = dec.decrypt_last(payload).map_err(|_| wrong())?;
            out.write_all(&pt).map_err(io)?;
            break;
        }
        let pt = dec.decrypt_next(payload).map_err(|_| wrong())?;
        out.write_all(&pt).map_err(io)?;
        std::mem::swap(&mut cur, &mut next);
        n = m;
    }
    out.flush().map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery";

    fn seal(plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        encrypt(plain, &mut out, PW, KdfParams::TEST).unwrap();
        out
    }

    fn open(file: &[u8], pw: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        decrypt(file, &mut out, pw).map(|_| out)
    }

    fn msg(e: Error) -> String {
        e.to_string()
    }

    #[test]
    fn round_trip_small_and_multi_chunk() {
        for len in [0usize, 1, CHUNK, 200 * 1024] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            assert_eq!(open(&seal(&plain), PW).unwrap(), plain, "len {len}");
        }
    }

    #[test]
    fn wrong_passphrase_fails() {
        let e = open(&seal(b"hello"), "another passphrase").unwrap_err();
        assert!(msg(e).contains("wrong passphrase or damaged file"));
    }

    #[test]
    fn flipped_byte_fails() {
        let file = seal(&vec![7u8; 100_000]);
        for at in [MAGIC.len() + 1 + 12, HEADER_LEN + 3, file.len() - 1] {
            let mut bad = file.clone();
            bad[at] ^= 1;
            assert!(open(&bad, PW).is_err(), "byte {at}");
        }
    }

    #[test]
    fn truncated_last_chunk_fails() {
        let file = seal(&vec![7u8; 3 * CHUNK]);
        // Drop the last chunk exactly at a chunk boundary.
        let cut = HEADER_LEN + 2 * (CHUNK + TAG);
        assert!(open(&file[..cut], PW).is_err());
        assert!(open(&file[..file.len() - 5], PW).is_err());
    }

    #[test]
    fn rejects_oversized_kdf_params() {
        let mut file = seal(b"x");
        let at = MAGIC.len() + 1;
        file[at..at + 4].copy_from_slice(&4_000_000u32.to_le_bytes());
        let e = msg(open(&file, PW).unwrap_err());
        assert!(e.contains("damaged backup"), "{e}");
        assert!(!e.contains("wrong passphrase"));
    }

    #[test]
    fn not_a_backup() {
        let e = msg(open(&[3u8; 300], PW).unwrap_err());
        assert!(e.contains("not a FleetOps backup"), "{e}");
    }

    #[test]
    fn short_passphrase_refused() {
        assert!(check_passphrase("short").is_err());
        assert!(check_passphrase("twelve chars").is_ok());
    }
}
