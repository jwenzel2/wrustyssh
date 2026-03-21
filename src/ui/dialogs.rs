use uuid::Uuid;

use super::window::KeyItem;
use crate::app::SharedState;
use crate::config::Settings;
use crate::keys::generate::{generate_keypair, import_keypair};
use crate::keys::storage::KeyStore;
use crate::models::connection::{AuthMethod, ConnectionProfile, KeyAlgorithm};
use crate::models::tunnel::TunnelConfig;

/// Build key names list for the connection dialog.
pub fn build_key_names(state: &SharedState) -> (Vec<slint::SharedString>, Vec<Uuid>) {
    let store = state.key_store.lock().unwrap();
    let mut names = vec![slint::SharedString::from("(None)")];
    let mut ids = vec![Uuid::nil()];
    for k in &store.keys {
        names.push(slint::SharedString::from(format!(
            "{} ({})",
            k.name, k.algorithm
        )));
        ids.push(k.id);
    }
    (names, ids)
}

/// Build the stored keys list for the key manager.
pub fn build_key_items(state: &SharedState) -> Vec<KeyItem> {
    let store = state.key_store.lock().unwrap();
    store
        .keys
        .iter()
        .enumerate()
        .map(|(idx, k)| KeyItem {
            id: k.id.to_string().into(),
            name: k.name.clone().into(),
            subtitle: format!("{} - {}", k.algorithm, k.public_key_fingerprint).into(),
            index: idx as i32,
        })
        .collect()
}

/// Save a connection profile (create or update).
pub fn save_connection_profile(
    state: &SharedState,
    conn_name: &str,
    hostname: &str,
    port: u16,
    username: &str,
    auth_method_index: i32,
    key_ids: &[Uuid],
    key_index: i32,
    tunnels: Vec<TunnelConfig>,
    existing_id: Option<Uuid>,
    existing_created_at: Option<i64>,
) -> Result<(), String> {
    if conn_name.is_empty() || hostname.is_empty() || username.is_empty() {
        return Err("Name, hostname, and username are required.".to_string());
    }

    let auth_method = match auth_method_index {
        0 => AuthMethod::Password,
        1 => AuthMethod::PublicKey,
        2 => AuthMethod::Both,
        _ => AuthMethod::Password,
    };

    let key_idx = key_index as usize;
    let key_pair_id = if key_idx > 0 && key_idx < key_ids.len() {
        Some(key_ids[key_idx])
    } else {
        None
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let profile = ConnectionProfile {
        id: existing_id.unwrap_or_else(Uuid::new_v4),
        name: conn_name.to_string(),
        hostname: hostname.to_string(),
        port,
        username: username.to_string(),
        auth_method,
        key_pair_id,
        tunnels,
        created_at: existing_created_at.unwrap_or(now),
        updated_at: now,
    };

    let mut store = state.profile_store.lock().unwrap();
    if existing_id.is_some() {
        let _ = store.update(profile);
    } else {
        let _ = store.add(profile);
    }

    Ok(())
}

/// Delete a connection profile by index.
pub fn delete_connection(state: &SharedState, index: usize) {
    let mut store = state.profile_store.lock().unwrap();
    if let Some(profile) = store.profiles.get(index) {
        let id = profile.id;
        let _ = store.remove(&id);
    }
}

/// Generate a new SSH key.
pub fn generate_key(
    state: &SharedState,
    name: &str,
    passphrase: &str,
    algo_index: i32,
) -> Result<String, String> {
    if name.is_empty() {
        return Err("Key name is required.".to_string());
    }

    let algorithm = match algo_index {
        0 => KeyAlgorithm::Ed25519,
        1 => KeyAlgorithm::EcdsaNistP256,
        2 => KeyAlgorithm::RsaSha2_512,
        _ => KeyAlgorithm::Ed25519,
    };

    let passphrase_opt = if passphrase.is_empty() {
        None
    } else {
        Some(passphrase)
    };

    match generate_keypair(name, algorithm, passphrase_opt) {
        Ok(meta) => {
            let algo_name = meta.algorithm.to_string();
            let mut store = state.key_store.lock().unwrap();
            if let Err(e) = store.add(meta) {
                return Err(format!("Failed to save key: {e}"));
            }
            Ok(algo_name)
        }
        Err(e) => Err(format!("Key generation failed: {e}")),
    }
}

/// Import an existing SSH key.
pub fn import_key(
    state: &SharedState,
    name: &str,
    private_path: &str,
    public_path: &str,
) -> Result<String, String> {
    if name.is_empty() {
        return Err("Key name is required.".to_string());
    }
    if private_path == "No file selected" || public_path == "No file selected" {
        return Err("Please select both private and public key files.".to_string());
    }

    let priv_path = std::path::PathBuf::from(private_path);
    let pub_path = std::path::PathBuf::from(public_path);

    match import_keypair(name, &priv_path, &pub_path) {
        Ok(meta) => {
            let algo_name = meta.algorithm.to_string();
            let mut store = state.key_store.lock().unwrap();
            if let Err(e) = store.add(meta) {
                return Err(format!("Failed to save imported key: {e}"));
            }
            Ok(format!(
                "Successfully imported {} key \"{}\".",
                algo_name, name
            ))
        }
        Err(e) => Err(format!("Import failed: {e}")),
    }
}

/// Delete a key by index.
pub fn delete_key_by_index(state: &SharedState, index: usize) {
    let mut store = state.key_store.lock().unwrap();
    if let Some(key) = store.keys.get(index) {
        let id = key.id;
        let _ = store.remove(&id);
    }
}

/// Copy a public key to clipboard.
pub fn copy_public_key(state: &SharedState, index: usize) -> Result<String, String> {
    let store = state.key_store.lock().unwrap();
    if let Some(key) = store.keys.get(index) {
        let id = key.id;
        drop(store);
        KeyStore::read_public_key(&id).map_err(|e| format!("Failed to read public key: {e}"))
    } else {
        Err("Key not found.".to_string())
    }
}

/// Save preferences.
pub fn save_preferences(
    state: &SharedState,
    font_family: &str,
    font_size: u32,
    scrollback_lines: i64,
    terminal_type: &str,
    terminal_color_scheme: &str,
    app_font_family: &str,
    app_font_size: u32,
    button_font_size: u32,
    connection_name_font_size: u32,
    connection_subtitle_font_size: u32,
) -> Result<(), String> {
    let new_settings = Settings {
        font_family: font_family.to_string(),
        font_size,
        scrollback_lines,
        default_terminal_type: terminal_type.to_string(),
        terminal_color_scheme: terminal_color_scheme.to_string(),
        app_font_family: app_font_family.to_string(),
        app_font_size,
        button_font_size,
        connection_name_font_size,
        connection_subtitle_font_size,
    };

    if let Err(e) = new_settings.save() {
        return Err(format!("Failed to save settings: {e}"));
    }

    let mut settings = state.settings.lock().unwrap();
    *settings = new_settings;
    Ok(())
}

/// Create a tunnel config from dialog values.
pub fn create_tunnel_config(
    name: &str,
    local_host: &str,
    local_port: u16,
    remote_host: &str,
    remote_port: u16,
    enabled: bool,
) -> Option<TunnelConfig> {
    if name.is_empty() {
        return None;
    }

    Some(TunnelConfig {
        id: Uuid::new_v4(),
        name: name.to_string(),
        tunnel_type: crate::models::tunnel::TunnelType::LocalForward,
        local_host: local_host.to_string(),
        local_port,
        remote_host: remote_host.to_string(),
        remote_port,
        enabled,
    })
}
