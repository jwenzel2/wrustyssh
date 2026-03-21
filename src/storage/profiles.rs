use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config;
use crate::error::AppError;
use crate::models::connection::ConnectionProfile;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileBackup {
    pub version: u32,
    pub profiles: Vec<ConnectionProfile>,
}

#[derive(Debug)]
pub struct ProfileStore {
    pub profiles: Vec<ConnectionProfile>,
}

impl ProfileStore {
    pub fn load() -> Self {
        let path = config::profiles_path();
        let profiles = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
                Err(e) => {
                    log::warn!("Failed to read profiles: {e}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        Self { profiles }
    }

    pub fn save(&self) -> Result<(), AppError> {
        let path = config::profiles_path();
        let data = serde_json::to_string_pretty(&self.profiles)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn add(&mut self, profile: ConnectionProfile) -> Result<(), AppError> {
        self.profiles.push(profile);
        self.save()
    }

    pub fn update(&mut self, profile: ConnectionProfile) -> Result<(), AppError> {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile;
            self.save()
        } else {
            Err(AppError::Config("Profile not found".into()))
        }
    }

    pub fn remove(&mut self, id: &Uuid) -> Result<(), AppError> {
        self.profiles.retain(|p| &p.id != id);
        self.save()
    }

    pub fn get(&self, id: &Uuid) -> Option<&ConnectionProfile> {
        self.profiles.iter().find(|p| &p.id == id)
    }

    pub fn export_backup(&self) -> Result<String, AppError> {
        Ok(serde_json::to_string_pretty(&ProfileBackup {
            version: 1,
            profiles: self.profiles.clone(),
        })?)
    }

    pub fn import_backup(&mut self, json: &str) -> Result<usize, AppError> {
        let backup: ProfileBackup = serde_json::from_str(json)
            .map_err(|e| AppError::Other(format!("Invalid backup file: {e}")))?;
        let mut imported = 0;
        for profile in backup.profiles {
            if self.profiles.iter().any(|p| p.id == profile.id) {
                continue;
            }
            self.profiles.push(profile);
            imported += 1;
        }
        self.save()?;
        Ok(imported)
    }
}
