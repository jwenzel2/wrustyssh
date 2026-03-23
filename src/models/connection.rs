use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::tunnel::TunnelConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    Password,
    PublicKey,
    Both,
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMethod::Password => write!(f, "Password"),
            AuthMethod::PublicKey => write!(f, "Public Key"),
            AuthMethod::Both => write!(f, "Both"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum KeyAlgorithm {
    Ed25519,
    EcdsaNistP256,
    RsaSha2_256,
    RsaSha2_512,
    Rsa,
}

impl std::fmt::Display for KeyAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyAlgorithm::Ed25519 => write!(f, "Ed25519"),
            KeyAlgorithm::EcdsaNistP256 => write!(f, "ECDSA NIST P-256"),
            KeyAlgorithm::RsaSha2_256 => write!(f, "RSA SHA2-256"),
            KeyAlgorithm::RsaSha2_512 => write!(f, "RSA SHA2-512"),
            KeyAlgorithm::Rsa => write!(f, "RSA (legacy)"),
        }
    }
}

impl KeyAlgorithm {
    pub fn all() -> &'static [KeyAlgorithm] {
        &[
            KeyAlgorithm::Ed25519,
            KeyAlgorithm::EcdsaNistP256,
            KeyAlgorithm::RsaSha2_512,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPairMeta {
    pub id: Uuid,
    pub name: String,
    pub algorithm: KeyAlgorithm,
    pub public_key_fingerprint: String,
    pub created_at: i64,
    pub private_key_filename: String,
    pub public_key_filename: String,
    #[serde(default)]
    pub has_passphrase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: Uuid,
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    pub key_pair_id: Option<Uuid>,
    #[serde(default)]
    pub use_cloudflare_tunnel: bool,
    pub tunnels: Vec<TunnelConfig>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ConnectionProfile {
    pub fn new(name: String, hostname: String, port: u16, username: String) -> Self {
        let now = chrono_timestamp();
        Self {
            id: Uuid::new_v4(),
            name,
            hostname,
            port,
            username,
            auth_method: AuthMethod::Password,
            key_pair_id: None,
            use_cloudflare_tunnel: false,
            tunnels: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
