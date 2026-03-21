use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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

        std::fs::write(&pub_path, public_key_openssh)?;
        #[cfg(unix)]
        std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644))?;

        Ok(())
    }

    pub fn read_public_key(id: &Uuid) -> Result<String, AppError> {
        let path = paths::public_key_path(id);
        Ok(std::fs::read_to_string(path)?)
    }

    pub fn export_backup(&self) -> Result<String, AppError> {
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
        Ok(serde_json::to_string_pretty(&KeyBackup {
            version: 1,
            keys: entries,
        })?)
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
