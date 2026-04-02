use std::path::Path;

use ssh_key::private::{EcdsaKeypair, Ed25519Keypair, KeypairData};
use ssh_key::{Algorithm, EcdsaCurve, HashAlg, LineEnding, PrivateKey};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::error::AppError;
use crate::keys::storage::KeyStore;
use crate::models::connection::{KeyAlgorithm, KeyPairMeta};

pub fn generate_keypair(
    name: &str,
    algorithm: KeyAlgorithm,
    passphrase: Option<&str>,
) -> Result<KeyPairMeta, AppError> {
    let mut rng = rand::thread_rng();
    let id = Uuid::new_v4();

    let private_key = match algorithm {
        KeyAlgorithm::Ed25519 => {
            let keypair = Ed25519Keypair::random(&mut rng);
            PrivateKey::new(KeypairData::Ed25519(keypair), "")
                .map_err(|e| AppError::KeyGen(e.to_string()))?
        }
        KeyAlgorithm::EcdsaNistP256 => {
            let keypair = EcdsaKeypair::random(&mut rng, EcdsaCurve::NistP256)
                .map_err(|e| AppError::KeyGen(e.to_string()))?;
            PrivateKey::new(KeypairData::Ecdsa(keypair), "")
                .map_err(|e| AppError::KeyGen(e.to_string()))?
        }
        KeyAlgorithm::RsaSha2_256 | KeyAlgorithm::RsaSha2_512 | KeyAlgorithm::Rsa => {
            let rsa_keypair = ssh_key::private::RsaKeypair::random(&mut rng, 4096)
                .map_err(|e| AppError::KeyGen(e.to_string()))?;
            PrivateKey::new(KeypairData::Rsa(rsa_keypair), "")
                .map_err(|e| AppError::KeyGen(e.to_string()))?
        }
    };

    let has_passphrase = matches!(passphrase, Some(p) if !p.is_empty());
    let private_key = if has_passphrase {
        private_key
            .encrypt(&mut rng, passphrase.unwrap())
            .map_err(|e| AppError::KeyGen(e.to_string()))?
    } else {
        private_key
    };

    let public_key = private_key.public_key();
    let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();

    let private_pem: Zeroizing<String> = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|e| AppError::KeyGen(e.to_string()))?
        .into();
    let public_openssh = public_key
        .to_openssh()
        .map_err(|e| AppError::KeyGen(e.to_string()))?;

    KeyStore::write_key_files(&id, private_pem.as_str(), &public_openssh)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let meta = KeyPairMeta {
        id,
        name: name.to_string(),
        algorithm,
        public_key_fingerprint: fingerprint,
        created_at: now,
        private_key_filename: format!("{}.key", id),
        public_key_filename: format!("{}.pub", id),
        has_passphrase,
    };

    Ok(meta)
}

pub fn import_keypair(
    name: &str,
    private_key_path: &Path,
    public_key_path: &Path,
) -> Result<KeyPairMeta, AppError> {
    let private_key_data = Zeroizing::new(std::fs::read_to_string(private_key_path)?);
    let public_key_data = std::fs::read_to_string(public_key_path)?;

    // Try to parse the private key without a passphrase to detect algorithm
    // If it's encrypted, from_openssh will fail, so we try the public key instead
    let (algorithm, has_passphrase) = match PrivateKey::from_openssh(&private_key_data) {
        Ok(pk) => (map_algorithm(pk.algorithm()), false),
        Err(_) => {
            // Key is likely encrypted — parse public key to detect algorithm
            let pub_key = ssh_key::PublicKey::from_openssh(&public_key_data)
                .map_err(|e| AppError::KeyGen(format!("Invalid public key: {e}")))?;
            (map_algorithm(pub_key.algorithm()), true)
        }
    };

    let pub_key = ssh_key::PublicKey::from_openssh(&public_key_data)
        .map_err(|e| AppError::KeyGen(format!("Invalid public key: {e}")))?;
    let fingerprint = pub_key.fingerprint(HashAlg::Sha256).to_string();

    let id = Uuid::new_v4();
    KeyStore::write_key_files(&id, &private_key_data, &public_key_data)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let meta = KeyPairMeta {
        id,
        name: name.to_string(),
        algorithm,
        public_key_fingerprint: fingerprint,
        created_at: now,
        private_key_filename: format!("{}.key", id),
        public_key_filename: format!("{}.pub", id),
        has_passphrase,
    };

    Ok(meta)
}

fn map_algorithm(algo: Algorithm) -> KeyAlgorithm {
    match algo {
        Algorithm::Ed25519 => KeyAlgorithm::Ed25519,
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        } => KeyAlgorithm::EcdsaNistP256,
        Algorithm::Rsa {
            hash: Some(ssh_key::HashAlg::Sha256),
        } => KeyAlgorithm::RsaSha2_256,
        Algorithm::Rsa {
            hash: Some(ssh_key::HashAlg::Sha512),
        } => KeyAlgorithm::RsaSha2_512,
        Algorithm::Rsa { .. } => KeyAlgorithm::Rsa,
        _ => KeyAlgorithm::Ed25519, // fallback
    }
}
