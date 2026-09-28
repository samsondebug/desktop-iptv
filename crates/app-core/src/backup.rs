//! Encrypted backup container (CLAUDE.md §10: "encrypted export of playlists, favorites,
//! settings, EPG overrides").
//!
//! Layout (all big-endian, fixed sizes):
//!
//! ```text
//! "DIPTVBK1"        8 bytes   magic + format version
//! salt              16 bytes  Argon2id salt (random per file)
//! nonce             12 bytes  AES-256-GCM nonce (random per file)
//! ciphertext        …         AES-256-GCM over the JSON payload; the magic is bound as AAD
//! ```
//!
//! Key = Argon2id(passphrase, salt; m = 64 MiB, t = 3, p = 1). Passphrases are never stored.
//! Playlist credentials **are** inside the payload — that is the point of encrypting it — so a
//! backup restores on a new machine without re-typing every provider login. Machine-bound items
//! (the parental PIN hash, the license token) are deliberately left out.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use rand_core::RngCore;

pub const MAGIC: &[u8; 8] = b"DIPTVBK1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("passphrase must be at least 6 characters")]
    WeakPassphrase,
    #[error("not an SKTV backup file")]
    BadMagic,
    #[error("backup file is truncated or corrupt")]
    Truncated,
    #[error("wrong passphrase or corrupt file")]
    Decrypt,
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("encryption failed")]
    Encrypt,
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], BackupError> {
    let params = Params::new(64 * 1024, 3, 1, Some(KEY_LEN)).map_err(|e| BackupError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon.hash_password_into(passphrase.as_bytes(), salt, &mut key).map_err(|e| BackupError::Kdf(e.to_string()))?;
    Ok(key)
}

/// Encrypt `plaintext` (a JSON payload) under `passphrase`.
pub fn seal(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, BackupError> {
    if passphrase.chars().count() < 6 {
        return Err(BackupError::WeakPassphrase);
    }
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let key = derive_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| BackupError::Encrypt)?;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), aes_gcm::aead::Payload { msg: plaintext, aad: MAGIC })
        .map_err(|_| BackupError::Encrypt)?;
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt a file produced by [`seal`]. A wrong passphrase is indistinguishable from corruption
/// (that is what authenticated encryption gives us), hence the single error variant.
pub fn open(passphrase: &str, file: &[u8]) -> Result<Vec<u8>, BackupError> {
    if file.len() < MAGIC.len() {
        return Err(BackupError::BadMagic);
    }
    if &file[..MAGIC.len()] != MAGIC {
        return Err(BackupError::BadMagic);
    }
    let rest = &file[MAGIC.len()..];
    if rest.len() < SALT_LEN + NONCE_LEN + 16 {
        return Err(BackupError::Truncated);
    }
    let (salt, rest) = rest.split_at(SALT_LEN);
    let (nonce, ct) = rest.split_at(NONCE_LEN);
    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| BackupError::Decrypt)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), aes_gcm::aead::Payload { msg: ct, aad: MAGIC })
        .map_err(|_| BackupError::Decrypt)
}

/// Is this file one of ours? (Cheap check before asking for a passphrase.)
pub fn looks_like_backup(file: &[u8]) -> bool {
    file.len() >= MAGIC.len() && &file[..MAGIC.len()] == MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_failure_modes() {
        let payload = br#"{"playlists":[{"pass":"hunter2"}]}"#;
        let sealed = seal("correct horse", payload).unwrap();
        assert!(looks_like_backup(&sealed));
        assert!(!sealed.windows(7).any(|w| w == b"hunter2"), "plaintext leaked into the container");
        assert_eq!(open("correct horse", &sealed).unwrap(), payload);
        assert!(matches!(open("wrong", &sealed), Err(BackupError::Decrypt)));
        assert!(matches!(open("correct horse", b"nope"), Err(BackupError::BadMagic)));
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(matches!(open("correct horse", &tampered), Err(BackupError::Decrypt)));
        assert!(matches!(seal("short", payload), Err(BackupError::WeakPassphrase)));
        // Two seals of the same payload differ (fresh salt + nonce).
        assert_ne!(seal("correct horse", payload).unwrap(), sealed);
    }
}
