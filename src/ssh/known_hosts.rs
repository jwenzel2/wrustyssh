use std::collections::HashMap;

use crate::config;

/// Simple known-hosts store: maps "hostname:port" -> (key_type, fingerprint).
/// Stored as one JSON object for simplicity.
#[derive(Debug)]
pub struct KnownHosts {
    entries: HashMap<String, HostEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct HostEntry {
    key_type: String,
    fingerprint: String,
}

impl KnownHosts {
    pub fn load() -> Self {
        let path = config::known_hosts_path();
        let entries = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
                Err(e) => {
                    log::warn!("Failed to read known_hosts: {e}");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        Self { entries }
    }

    fn save(&self) {
        let path = config::known_hosts_path();
        match serde_json::to_string_pretty(&self.entries) {
            Ok(data) => {
                if let Err(e) = std::fs::write(&path, data) {
                    log::error!("Failed to write known_hosts: {e}");
                }
            }
            Err(e) => log::error!("Failed to serialize known_hosts: {e}"),
        }
    }

    /// Look up a host. Returns `None` if not in the store, `Some(fingerprint)`
    /// if it is.
    pub fn lookup(&self, host_key: &str) -> Option<&str> {
        self.entries.get(host_key).map(|e| e.fingerprint.as_str())
    }

    /// Record or update a host's fingerprint, then persist.
    pub fn accept(&mut self, host_key: &str, key_type: &str, fingerprint: &str) {
        self.entries.insert(
            host_key.to_string(),
            HostEntry {
                key_type: key_type.to_string(),
                fingerprint: fingerprint.to_string(),
            },
        );
        self.save();
    }

    /// Build the lookup key from hostname and port.
    pub fn host_key(hostname: &str, port: u16) -> String {
        format!("{hostname}:{port}")
    }
}

/// Check a server's fingerprint against known_hosts.
/// Returns the path for the caller to decide what to do.
pub fn check(
    hostname: &str,
    port: u16,
    _key_type: &str,
    fingerprint: &str,
) -> CheckResult {
    let hosts = KnownHosts::load();
    let key = KnownHosts::host_key(hostname, port);
    match hosts.lookup(&key) {
        None => CheckResult::New,
        Some(stored) if stored == fingerprint => CheckResult::Match,
        Some(stored) => CheckResult::Changed {
            old_fingerprint: stored.to_string(),
        },
    }
}

/// Accept and persist a host key.
pub fn accept(hostname: &str, port: u16, key_type: &str, fingerprint: &str) {
    let mut hosts = KnownHosts::load();
    let key = KnownHosts::host_key(hostname, port);
    hosts.accept(&key, key_type, fingerprint);
}

#[derive(Debug)]
pub enum CheckResult {
    /// Host not in known_hosts — first connection.
    New,
    /// Fingerprint matches known_hosts.
    Match,
    /// Fingerprint differs from known_hosts (possible MITM).
    Changed { old_fingerprint: String },
}
