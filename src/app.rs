use std::sync::{Arc, Mutex};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::config::Settings;
use crate::keys::storage::KeyStore;
use crate::models::tunnel::TunnelConfig;
use crate::storage::profiles::ProfileStore;

/// Commands sent from GTK UI thread to Tokio SSH task
#[derive(Debug)]
pub enum SshCommand {
    SendData(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    StartTunnel(TunnelConfig),
    StopTunnel(Uuid),
    Disconnect,
}

/// Result of host-key lookup against the known_hosts file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// First time seeing this host — no entry in known_hosts.
    New,
    /// The host exists in known_hosts but the fingerprint changed.
    Changed { old_fingerprint: String },
}

/// Events sent from Tokio SSH task to UI thread
#[derive(Debug, Clone)]
pub enum SshEvent {
    Connected,
    Data(Vec<u8>),
    TunnelEstablished(Uuid),
    TunnelFailed(Uuid, String),
    Disconnected(Option<String>),
    Error(String),
    HostKeyVerify {
        hostname: String,
        key_type: String,
        fingerprint: String,
        status: HostKeyStatus,
        /// Send `true` to accept, `false` to reject.
        response_tx: async_channel::Sender<bool>,
    },
}

/// Application-wide shared state
#[derive(Clone)]
pub struct SharedState {
    pub settings: Arc<Mutex<Settings>>,
    pub profile_store: Arc<Mutex<ProfileStore>>,
    pub key_store: Arc<Mutex<KeyStore>>,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            settings: Arc::new(Mutex::new(Settings::load())),
            profile_store: Arc::new(Mutex::new(ProfileStore::load())),
            key_store: Arc::new(Mutex::new(KeyStore::load())),
        }
    }
}

/// Holds authentication credentials for a connection attempt (not persisted)
#[allow(dead_code)]
pub struct AuthCredentials {
    pub password: Option<Zeroizing<String>>,
    pub private_key_path: Option<std::path::PathBuf>,
}
