use async_trait::async_trait;
use russh::client;
use russh::keys::key::PublicKey;

use crate::app::{HostKeyStatus, SshEvent};
use crate::ssh::known_hosts;

pub struct ClientHandler {
    pub event_tx: async_channel::Sender<SshEvent>,
    /// "hostname:port" used for known_hosts lookups.
    pub host_id: String,
    /// Hostname (without port) for display and known_hosts storage.
    pub hostname: String,
    /// Port number for known_hosts storage.
    pub port: u16,
}

impl ClientHandler {
    pub fn new(
        event_tx: async_channel::Sender<SshEvent>,
        hostname: String,
        port: u16,
    ) -> Self {
        let host_id = known_hosts::KnownHosts::host_key(&hostname, port);
        Self {
            event_tx,
            host_id,
            hostname,
            port,
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

        // Check known_hosts first
        match known_hosts::check(&self.hostname, self.port, key_type, &fingerprint) {
            known_hosts::CheckResult::Match => {
                // Known host, fingerprint matches — accept silently
                return Ok(true);
            }
            known_hosts::CheckResult::New => {
                // New host — ask the user (TOFU)
                let (response_tx, response_rx) = async_channel::bounded(1);
                let _ = self
                    .event_tx
                    .send(SshEvent::HostKeyVerify {
                        hostname: self.host_id.clone(),
                        key_type: key_type.to_string(),
                        fingerprint: fingerprint.clone(),
                        status: HostKeyStatus::New,
                        response_tx,
                    })
                    .await;

                let accepted = response_rx.recv().await.unwrap_or(false);
                if accepted {
                    known_hosts::accept(&self.hostname, self.port, key_type, &fingerprint);
                }
                Ok(accepted)
            }
            known_hosts::CheckResult::Changed { old_fingerprint } => {
                // Fingerprint mismatch — warn the user strongly
                let (response_tx, response_rx) = async_channel::bounded(1);
                let _ = self
                    .event_tx
                    .send(SshEvent::HostKeyVerify {
                        hostname: self.host_id.clone(),
                        key_type: key_type.to_string(),
                        fingerprint: fingerprint.clone(),
                        status: HostKeyStatus::Changed { old_fingerprint },
                        response_tx,
                    })
                    .await;

                let accepted = response_rx.recv().await.unwrap_or(false);
                if accepted {
                    known_hosts::accept(&self.hostname, self.port, key_type, &fingerprint);
                }
                Ok(accepted)
            }
        }
    }
}
