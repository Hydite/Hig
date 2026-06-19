use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::KdfProfile;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const SALT_LEN: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_cost_kib: u32,
    pub time_cost: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            memory_cost_kib: 19 * 1024,
            time_cost: 2,
            parallelism: 1,
        }
    }
}

impl KdfProfile {
    pub fn params(self) -> KdfParams {
        match self {
            Self::Secure => KdfParams::default(),
            Self::Interactive => KdfParams {
                memory_cost_kib: 8 * 1024,
                time_cost: 1,
                parallelism: 1,
            },
            Self::FastBench => KdfParams {
                memory_cost_kib: 1024,
                time_cost: 1,
                parallelism: 1,
            },
        }
    }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

pub fn derive_key(
    password: &str,
    salt: &[u8],
    params: &KdfParams,
) -> anyhow::Result<[u8; KEY_LEN]> {
    let argon = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(
            params.memory_cost_kib,
            params.time_cost,
            params.parallelism,
            Some(KEY_LEN),
        )
        .map_err(|err| anyhow::anyhow!("invalid key derivation parameters: {err}"))?,
    );
    let mut key = [0_u8; KEY_LEN];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|err| anyhow::anyhow!("key derivation failed: {err}"))?;
    Ok(key)
}

pub fn encrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), plaintext)
        .map_err(|_| anyhow::anyhow!("encryption failed"))
}

pub fn decrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption/authentication failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_requires_same_password() {
        let salt = random_bytes::<SALT_LEN>();
        let params = KdfParams::default();
        let key = derive_key("correct", &salt, &params).unwrap();
        let wrong_key = derive_key("wrong", &salt, &params).unwrap();
        let nonce = random_bytes::<NONCE_LEN>();
        let encrypted = encrypt(&key, &nonce, b"secret").unwrap();
        assert_eq!(decrypt(&key, &nonce, &encrypted).unwrap(), b"secret");
        assert!(decrypt(&wrong_key, &nonce, &encrypted).is_err());
    }

    #[test]
    fn kdf_profiles_map_to_expected_strengths() {
        let secure = KdfProfile::Secure.params();
        let interactive = KdfProfile::Interactive.params();
        let fast = KdfProfile::FastBench.params();
        assert_eq!(secure, KdfParams::default());
        assert!(interactive.memory_cost_kib < secure.memory_cost_kib);
        assert!(interactive.time_cost <= secure.time_cost);
        assert!(fast.memory_cost_kib < interactive.memory_cost_kib);
        assert!(fast.time_cost <= interactive.time_cost);
    }
}
