use async_trait::async_trait;
use russh::client;
use russh::keys::key::PublicKey;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::app::SshEvent;

pub struct ClientHandler {
    pub event_tx: async_channel::Sender<SshEvent>,
    pub host_key_accepted: Arc<Mutex<Option<bool>>>,
    pub host_key_notify: Arc<tokio::sync::Notify>,
}

impl ClientHandler {
    pub fn new(event_tx: async_channel::Sender<SshEvent>) -> Self {
        Self {
            event_tx,
            host_key_accepted: Arc::new(Mutex::new(None)),
            host_key_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint();
        let key_type = match server_public_key {
            PublicKey::Ed25519(_) => "ssh-ed25519",
            PublicKey::RSA { .. } => "ssh-rsa",
            PublicKey::EC { ref key } => key.ident(),
        };

        let _ = self
            .event_tx
            .send(SshEvent::HostKeyVerify {
                key_type: key_type.to_string(),
                fingerprint: fingerprint.clone(),
            })
            .await;

        // Auto-accept (TOFU model) - in production, check known_hosts
        Ok(true)
    }
}
