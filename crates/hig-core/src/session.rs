use crate::crypto::KdfParams;
use crate::{EncryptionMode, KdfProfile};
use serde::{Deserialize, Serialize};
use std::path::Path;

const DEFAULT_TTL_SECS: u64 = 1_800;
const MAX_TTL_SECS: u64 = 7_200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBinding {
    pub fingerprint: [u8; 32],
    pub cache_dir: String,
    pub kdf_profile: KdfProfile,
    pub kdf: KdfParams,
    pub encryption: EncryptionMode,
    pub hig_version: String,
}

pub fn default_session_ttl(ttl: Option<u64>) -> u64 {
    ttl.unwrap_or(DEFAULT_TTL_SECS).min(MAX_TTL_SECS)
}

pub fn derive_session_binding(
    cache_dir: &Path,
    kdf_profile: KdfProfile,
    kdf: &KdfParams,
    encryption: EncryptionMode,
) -> SessionBinding {
    let cache_dir = cache_dir
        .canonicalize()
        .unwrap_or_else(|_| cache_dir.to_path_buf());
    let cache_dir_string = cache_dir.to_string_lossy().to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig session binding v2");
    hasher.update(cache_dir_string.as_bytes());
    hasher.update(format!("{kdf_profile:?}:{encryption:?}:1.8.0").as_bytes());
    hasher.update(&kdf.memory_cost_kib.to_le_bytes());
    hasher.update(&kdf.time_cost.to_le_bytes());
    hasher.update(&kdf.parallelism.to_le_bytes());
    SessionBinding {
        fingerprint: *hasher.finalize().as_bytes(),
        cache_dir: cache_dir_string,
        kdf_profile,
        kdf: kdf.clone(),
        encryption,
        hig_version: "1.8.0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_changes_with_cache_and_profile() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let kdf = KdfParams::default();
        let secure = derive_session_binding(
            first.path(),
            KdfProfile::Secure,
            &kdf,
            EncryptionMode::Password,
        );
        let other_cache = derive_session_binding(
            second.path(),
            KdfProfile::Secure,
            &kdf,
            EncryptionMode::Password,
        );
        let interactive = derive_session_binding(
            first.path(),
            KdfProfile::Interactive,
            &KdfProfile::Interactive.params(),
            EncryptionMode::Password,
        );
        assert_ne!(secure.fingerprint, other_cache.fingerprint);
        assert_ne!(secure.fingerprint, interactive.fingerprint);
    }

    #[test]
    fn ttl_is_bounded() {
        assert_eq!(default_session_ttl(None), DEFAULT_TTL_SECS);
        assert_eq!(default_session_ttl(Some(99_999)), MAX_TTL_SECS);
    }
}
