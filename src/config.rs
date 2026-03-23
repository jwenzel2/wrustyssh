use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::error::AppError;

static PROJECT_DIRS: OnceLock<ProjectDirs> = OnceLock::new();

fn project_dirs() -> &'static ProjectDirs {
    PROJECT_DIRS.get_or_init(|| {
        ProjectDirs::from("com", "wrustyssh", "wrustyssh")
            .expect("Failed to determine project directories")
    })
}

pub fn config_dir() -> PathBuf {
    project_dirs().config_dir().to_path_buf()
}

pub fn data_dir() -> PathBuf {
    project_dirs().data_dir().to_path_buf()
}

pub fn profiles_path() -> PathBuf {
    config_dir().join("profiles.json")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn known_hosts_path() -> PathBuf {
    config_dir().join("known_hosts")
}

pub fn keys_index_path() -> PathBuf {
    data_dir().join("keys.json")
}

pub fn keys_dir() -> PathBuf {
    data_dir().join("keys")
}

fn ensure_directory(path: PathBuf) -> Result<(), AppError> {
    match std::fs::create_dir_all(&path) {
        Ok(()) => {}
        Err(err) if path.is_dir() => return Ok(()),
        Err(err) => {
            return Err(AppError::Config(format!(
                "Failed to create directory '{}': {}",
                path.display(),
                err
            )))
        }
    }

    if path.is_dir() {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "Expected directory at '{}', but found a file",
            path.display()
        )))
    }
}

pub fn ensure_directories() -> Result<(), AppError> {
    ensure_directory(config_dir())?;
    ensure_directory(data_dir())?;
    ensure_directory(keys_dir())?;
    Ok(())
}

pub const DEFAULT_FONT_FAMILY: &str = "Cascadia Mono";
pub const DEFAULT_FONT_SIZE: u32 = 13;
pub const DEFAULT_SCROLLBACK_LINES: i64 = 10000;
pub const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";
pub const DEFAULT_TERMINAL_COLOR_SCHEME: &str = "Campbell";
pub const DEFAULT_APP_FONT_FAMILY: &str = "Segoe UI";
pub const DEFAULT_APP_FONT_SIZE: u32 = 14;
pub const DEFAULT_BUTTON_FONT_SIZE: u32 = 13;
pub const DEFAULT_CONNECTION_NAME_FONT_SIZE: u32 = 16;
pub const DEFAULT_CONNECTION_SUBTITLE_FONT_SIZE: u32 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_family: String,
    pub font_size: u32,
    pub scrollback_lines: i64,
    pub default_terminal_type: String,
    pub terminal_color_scheme: String,
    pub app_font_family: String,
    pub app_font_size: u32,
    pub button_font_size: u32,
    pub connection_name_font_size: u32,
    pub connection_subtitle_font_size: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_FONT_FAMILY.into(),
            font_size: DEFAULT_FONT_SIZE,
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            default_terminal_type: DEFAULT_TERMINAL_TYPE.into(),
            terminal_color_scheme: DEFAULT_TERMINAL_COLOR_SCHEME.into(),
            app_font_family: DEFAULT_APP_FONT_FAMILY.into(),
            app_font_size: DEFAULT_APP_FONT_SIZE,
            button_font_size: DEFAULT_BUTTON_FONT_SIZE,
            connection_name_font_size: DEFAULT_CONNECTION_NAME_FONT_SIZE,
            connection_subtitle_font_size: DEFAULT_CONNECTION_SUBTITLE_FONT_SIZE,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<(), AppError> {
        let path = settings_path();
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }
}
