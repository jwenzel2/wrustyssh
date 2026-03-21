use std::path::{Path, PathBuf};

use super::window::SftpFileItem;
use crate::ssh::sftp::SftpEntry;

/// Format a file size for display.
pub fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Read a local directory and return SftpFileItem list for the UI.
pub fn read_local_dir(path: &Path) -> Vec<SftpFileItem> {
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => {
            log::error!("Failed to read directory {}: {}", path.display(), e);
            return items;
        }
    };

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata().ok();
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

        let item = SftpFileItem {
            name: name.into(),
            is_dir: is_dir,
            size: if is_dir {
                "".into()
            } else {
                format_size(size).into()
            },
            icon: if is_dir {
                "\u{1F4C1}".into()
            } else {
                "\u{1F4C4}".into()
            },
        };

        if is_dir {
            dirs.push(item);
        } else {
            files.push(item);
        }
    }

    // Sort: directories first, then files, alphabetically
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    items.extend(dirs);
    items.extend(files);
    items
}

/// Convert SftpEntry list from backend to SftpFileItem list for the UI.
pub fn sftp_entries_to_items(entries: &[SftpEntry]) -> Vec<SftpFileItem> {
    entries
        .iter()
        .map(|e| SftpFileItem {
            name: e.name.clone().into(),
            is_dir: e.is_dir,
            size: if e.is_dir {
                "".into()
            } else {
                format_size(e.size).into()
            },
            icon: if e.is_dir {
                "\u{1F4C1}".into()
            } else {
                "\u{1F4C4}".into()
            },
        })
        .collect()
}

/// Get parent path for local navigation.
pub fn local_parent(path: &Path) -> PathBuf {
    path.parent().unwrap_or(path).to_path_buf()
}

/// Get parent path for remote navigation.
pub fn remote_parent(path: &str) -> String {
    if path == "/" || path == "." || path.is_empty() {
        return path.to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            "/".to_string()
        } else {
            trimmed[..pos].to_string()
        }
    } else {
        ".".to_string()
    }
}

/// Join a child name onto a remote directory path.
pub fn join_remote_child(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else if base == "." || base.is_empty() {
        format!("./{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Build a short selection summary for the SFTP pane.
pub fn selection_summary(items: &[SftpFileItem], selected_index: i32) -> String {
    if selected_index < 0 {
        return "No selection".to_string();
    }

    let Some(item) = items.get(selected_index as usize) else {
        return "No selection".to_string();
    };

    if item.is_dir {
        format!("Selected folder: {}", item.name)
    } else if item.size.is_empty() {
        format!("Selected file: {}", item.name)
    } else {
        format!("Selected file: {} ({})", item.name, item.size)
    }
}
