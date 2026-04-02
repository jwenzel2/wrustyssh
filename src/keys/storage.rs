use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::path::Path;
use uuid::Uuid;

use crate::config;
use crate::error::AppError;
use crate::models::connection::KeyPairMeta;
use crate::storage::paths;

#[derive(Debug, Serialize, Deserialize)]
pub struct KeyBackupEntry {
    pub meta: KeyPairMeta,
    pub private_key: String,
    pub public_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KeyBackup {
    pub version: u32,
    pub keys: Vec<KeyBackupEntry>,
}

#[derive(Debug)]
pub struct KeyStore {
    pub keys: Vec<KeyPairMeta>,
}

impl KeyStore {
    pub fn load() -> Self {
        let path = config::keys_index_path();
        let keys = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
                Err(e) => {
                    log::warn!("Failed to read key index: {e}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        Self { keys }
    }

    pub fn save(&self) -> Result<(), AppError> {
        let path = config::keys_index_path();
        let data = serde_json::to_string_pretty(&self.keys)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn add(&mut self, meta: KeyPairMeta) -> Result<(), AppError> {
        self.keys.push(meta);
        self.save()
    }

    pub fn remove(&mut self, id: &Uuid) -> Result<(), AppError> {
        // Remove key files
        let priv_path = paths::private_key_path(id);
        let pub_path = paths::public_key_path(id);
        if priv_path.exists() {
            std::fs::remove_file(&priv_path)?;
        }
        if pub_path.exists() {
            std::fs::remove_file(&pub_path)?;
        }
        self.keys.retain(|k| &k.id != id);
        self.save()
    }

    pub fn get(&self, id: &Uuid) -> Option<&KeyPairMeta> {
        self.keys.iter().find(|k| &k.id == id)
    }

    pub fn write_key_files(
        id: &Uuid,
        private_key_pem: &str,
        public_key_openssh: &str,
    ) -> Result<(), AppError> {
        let priv_path = paths::private_key_path(id);
        let pub_path = paths::public_key_path(id);

        std::fs::write(&priv_path, private_key_pem)?;
        #[cfg(unix)]
        std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))?;
        #[cfg(windows)]
        restrict_file_to_current_user(&priv_path);

        std::fs::write(&pub_path, public_key_openssh)?;
        #[cfg(unix)]
        std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644))?;

        Ok(())
    }

    pub fn read_public_key(id: &Uuid) -> Result<String, AppError> {
        let path = paths::public_key_path(id);
        Ok(std::fs::read_to_string(path)?)
    }

    /// Export all keys as an encrypted backup. The plaintext JSON is encrypted
    /// with AES-256-GCM using a key derived from `passphrase` via Argon2id.
    pub fn export_encrypted_backup(&self, passphrase: &str) -> Result<Vec<u8>, AppError> {
        let plaintext = self.export_backup()?;
        encrypt_backup(plaintext.as_bytes(), passphrase)
    }

    /// Import keys from an encrypted backup produced by `export_encrypted_backup`.
    pub fn import_encrypted_backup(
        &mut self,
        data: &[u8],
        passphrase: &str,
    ) -> Result<usize, AppError> {
        let plaintext = decrypt_backup(data, passphrase)?;
        let json = std::str::from_utf8(&plaintext)
            .map_err(|e| AppError::Other(format!("Decrypted backup is not valid UTF-8: {e}")))?;
        self.import_backup(json)
    }

    pub fn export_backup(&self) -> Result<zeroize::Zeroizing<String>, AppError> {
        let mut entries = Vec::new();
        for meta in &self.keys {
            let private_key = std::fs::read_to_string(paths::private_key_path(&meta.id))?;
            let public_key = std::fs::read_to_string(paths::public_key_path(&meta.id))?;
            entries.push(KeyBackupEntry {
                meta: meta.clone(),
                private_key,
                public_key,
            });
        }
        let json = serde_json::to_string_pretty(&KeyBackup {
            version: 1,
            keys: entries,
        })?;
        Ok(zeroize::Zeroizing::new(json))
    }

    pub fn import_backup(&mut self, json: &str) -> Result<usize, AppError> {
        let backup: KeyBackup = serde_json::from_str(json)
            .map_err(|e| AppError::Other(format!("Invalid backup file: {e}")))?;
        let mut imported = 0;
        for entry in backup.keys {
            if self.keys.iter().any(|k| k.id == entry.meta.id) {
                continue;
            }
            Self::write_key_files(&entry.meta.id, &entry.private_key, &entry.public_key)?;
            self.keys.push(entry.meta);
            imported += 1;
        }
        self.save()?;
        Ok(imported)
    }
}

// ─── Encrypted backup helpers ─────────────────────────────

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use base64::Engine;

const BACKUP_MAGIC: &[u8; 4] = b"WKBK"; // wrustyssh key backup
const ARGON2_SALT_LEN: usize = 16;
const AES_NONCE_LEN: usize = 12;

/// Encrypt `plaintext` with AES-256-GCM, deriving the key from `passphrase`
/// via Argon2id. Returns: MAGIC(4) || salt(16) || nonce(12) || ciphertext.
fn encrypt_backup(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>, AppError> {
    use aes_gcm::aead::rand_core::RngCore;

    let mut salt = [0u8; ARGON2_SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    let mut key_bytes = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), &salt, &mut key_bytes)
        .map_err(|e| AppError::Other(format!("Key derivation failed: {e}")))?;

    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| AppError::Other(format!("Cipher init failed: {e}")))?;

    let mut nonce_bytes = [0u8; AES_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| AppError::Other(format!("Encryption failed: {e}")))?;

    let mut out = Vec::with_capacity(4 + ARGON2_SALT_LEN + AES_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(BACKUP_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);

    // Return base64-encoded for safe file storage
    let encoded = base64::engine::general_purpose::STANDARD.encode(&out);
    Ok(encoded.into_bytes())
}

/// Decrypt a backup produced by `encrypt_backup`.
fn decrypt_backup(data: &[u8], passphrase: &str) -> Result<Vec<u8>, AppError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| AppError::Other(format!("Backup is not valid base64: {e}")))?;

    let header_len = 4 + ARGON2_SALT_LEN + AES_NONCE_LEN;
    if raw.len() < header_len {
        return Err(AppError::Other("Backup file too short".into()));
    }
    if &raw[..4] != BACKUP_MAGIC {
        return Err(AppError::Other(
            "Not a valid wrustyssh encrypted backup (bad magic)".into(),
        ));
    }

    let salt = &raw[4..4 + ARGON2_SALT_LEN];
    let nonce_bytes = &raw[4 + ARGON2_SALT_LEN..header_len];
    let ciphertext = &raw[header_len..];

    let mut key_bytes = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key_bytes)
        .map_err(|e| AppError::Other(format!("Key derivation failed: {e}")))?;

    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| AppError::Other(format!("Cipher init failed: {e}")))?;

    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AppError::Other("Decryption failed — wrong passphrase?".into()))
}

/// Restrict a file so only the current user has access (Windows).
/// Uses `icacls` to remove inherited permissions and grant full control
/// exclusively to the current user.
#[cfg(windows)]
fn restrict_file_to_current_user(path: &Path) {
    let path_str = path.display().to_string();
    let username = std::env::var("USERNAME").unwrap_or_default();
    if username.is_empty() {
        log::warn!("Could not determine USERNAME to set ACL on {path_str}");
        return;
    }

    // Remove inherited permissions, then grant only the current user full control
    let result = std::process::Command::new("icacls")
        .args([&path_str, "/inheritance:r", "/grant:r", &format!("{username}:F")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match result {
        Ok(status) if status.success() => {
            log::info!("Set restrictive ACL on {path_str}");
        }
        Ok(status) => {
            log::warn!("icacls exited with {status} for {path_str}");
        }
        Err(e) => {
            log::warn!("Failed to run icacls for {path_str}: {e}");
        }
    }
}
