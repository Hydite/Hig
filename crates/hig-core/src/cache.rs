use crate::crypto::{KdfParams, NONCE_LEN, SALT_LEN};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
    pub bytes_reused: u64,
    pub bytes_compressed: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

pub fn reusable_path_chunks(
    record: &PathCacheRecord,
    file_size: u64,
    chunk_size: u64,
) -> Option<Vec<PathChunkRecord>> {
    if record.chunk_size != Some(chunk_size) || record.chunks.is_empty() {
        return None;
    }
    let mut expected_offset = 0_u64;
    for chunk in &record.chunks {
        if chunk.file_offset != expected_offset || chunk.len == 0 {
            return None;
        }
        expected_offset = expected_offset.checked_add(chunk.len)?;
        if expected_offset > file_size {
            return None;
        }
    }
    if expected_offset != file_size {
        return None;
    }
    Some(record.chunks.clone())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRecord {
    pub hash_hex: String,
    pub block_file: String,
    pub original_size: u64,
    pub compressed_size: u64,
    pub codec: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheIndex {
    #[serde(default)]
    pub records: BTreeMap<String, CacheRecord>,
    #[serde(default)]
    pub paths: BTreeMap<String, PathCacheRecord>,
    #[serde(default)]
    pub sealed_salt: Option<[u8; SALT_LEN]>,
    #[serde(default)]
    pub sealed_kdf_params: Option<KdfParams>,
    #[serde(default)]
    pub sealed_key_id: Option<[u8; 32]>,
    #[serde(default)]
    pub sealed_records: BTreeMap<String, SealedCacheRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathCacheRecord {
    pub relative_path: String,
    pub size: u64,
    pub mtime_ns: i128,
    pub permissions: u32,
    pub content_hash: [u8; 32],
    pub last_seen_unix_ns: i128,
    #[serde(default)]
    pub chunk_size: Option<u64>,
    #[serde(default)]
    pub chunks: Vec<PathChunkRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathChunkRecord {
    pub chunk_hash: [u8; 32],
    pub file_offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedCacheRecord {
    pub block_id: [u8; 32],
    pub nonce: [u8; NONCE_LEN],
    pub raw_size: u64,
    pub compressed_size: u64,
    pub encrypted_size: u64,
    pub sealed_file: String,
    pub codec: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct CacheStore {
    root: PathBuf,
    index: CacheIndex,
}

impl CacheStore {
    pub fn open(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("blocks"))?;
        let index_path = root.join("index.json");
        let index = if index_path.exists() {
            serde_json::from_slice(&fs::read(index_path)?)?
        } else {
            CacheIndex::default()
        };
        Ok(Self { root, index })
    }

    pub fn get(&self, hash: &[u8; 32]) -> anyhow::Result<Option<Vec<u8>>> {
        let hash_hex = hex::encode(hash);
        let Some(record) = self.index.records.get(&hash_hex) else {
            return Ok(None);
        };
        let block_path = self.root.join("blocks").join(&record.block_file);
        if !block_path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read(block_path)?))
    }

    pub fn has(&self, hash: &[u8; 32]) -> bool {
        let hash_hex = hex::encode(hash);
        self.index
            .records
            .get(&hash_hex)
            .is_some_and(|record| self.root.join("blocks").join(&record.block_file).exists())
    }

    pub fn has_batch(&self, key: &[u8; 32]) -> bool {
        self.root
            .join("blocks")
            .join(format!("{}.batch.zst", hex::encode(key)))
            .exists()
    }

    pub fn has_chunk(&self, hash: &[u8; 32]) -> bool {
        self.root
            .join("blocks")
            .join(format!("{}.chunk.zst", hex::encode(hash)))
            .exists()
    }

    pub fn get_batch(&self, key: &[u8; 32]) -> anyhow::Result<Option<Vec<u8>>> {
        let key_hex = hex::encode(key);
        let block_path = self
            .root
            .join("blocks")
            .join(format!("{key_hex}.batch.zst"));
        if !block_path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read(block_path)?))
    }

    pub fn get_chunk(&self, hash: &[u8; 32]) -> anyhow::Result<Option<Vec<u8>>> {
        let hash_hex = hex::encode(hash);
        let block_path = self
            .root
            .join("blocks")
            .join(format!("{hash_hex}.chunk.zst"));
        if !block_path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read(block_path)?))
    }

    pub fn sealed_salt_or_create(&mut self) -> [u8; SALT_LEN] {
        if let Some(salt) = self.index.sealed_salt {
            return salt;
        }
        let salt = crate::crypto::random_bytes::<SALT_LEN>();
        self.index.sealed_salt = Some(salt);
        salt
    }

    pub fn prepare_sealed_key(&mut self, kdf: KdfParams, key: &[u8; 32]) {
        let key_id = sealed_key_id(key);
        if self.index.sealed_key_id != Some(key_id) {
            self.index.sealed_records.clear();
            self.index.sealed_key_id = Some(key_id);
        }
        self.index.sealed_kdf_params = Some(kdf);
    }

    pub fn get_sealed_record(&self, key: &[u8; 32]) -> Option<&SealedCacheRecord> {
        self.index.sealed_records.get(&hex::encode(key))
    }

    pub fn sealed_block_path(&self, record: &SealedCacheRecord) -> PathBuf {
        self.root.join("blocks").join(&record.sealed_file)
    }

    pub fn get_path_record(&self, relative_path: &str) -> Option<&PathCacheRecord> {
        self.index.paths.get(relative_path)
    }

    pub fn insert(
        &mut self,
        hash: &[u8; 32],
        original_size: u64,
        compressed: &[u8],
    ) -> anyhow::Result<()> {
        let hash_hex = hex::encode(hash);
        let block_file = format!("{hash_hex}.zst");
        atomic_write(&self.root.join("blocks").join(&block_file), compressed)?;
        self.index.records.insert(
            hash_hex.clone(),
            CacheRecord {
                hash_hex,
                block_file,
                original_size,
                compressed_size: compressed.len() as u64,
                codec: "zstd".to_string(),
            },
        );
        Ok(())
    }

    pub fn insert_batch(&mut self, key: &[u8; 32], compressed: &[u8]) -> anyhow::Result<()> {
        let key_hex = hex::encode(key);
        atomic_write(
            &self
                .root
                .join("blocks")
                .join(format!("{key_hex}.batch.zst")),
            compressed,
        )?;
        Ok(())
    }

    pub fn insert_chunk(&mut self, hash: &[u8; 32], compressed: &[u8]) -> anyhow::Result<()> {
        let hash_hex = hex::encode(hash);
        atomic_write(
            &self
                .root
                .join("blocks")
                .join(format!("{hash_hex}.chunk.zst")),
            compressed,
        )?;
        Ok(())
    }

    pub fn insert_sealed(
        &mut self,
        key: &[u8; 32],
        record: SealedCacheRecord,
        ciphertext: &[u8],
    ) -> anyhow::Result<()> {
        let block_path = self.root.join("blocks").join(&record.sealed_file);
        atomic_write(&block_path, ciphertext)?;
        self.index.sealed_records.insert(hex::encode(key), record);
        Ok(())
    }

    pub fn upsert_path_record(&mut self, record: PathCacheRecord) -> anyhow::Result<()> {
        self.index
            .paths
            .insert(record.relative_path.clone(), record);
        Ok(())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.index)?;
        fs::write(self.root.join("index.json"), bytes)?;
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut temp = path.to_path_buf();
    let unique = format!(
        "tmp-{}-{}",
        std::process::id(),
        crate::crypto::random_bytes::<8>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    temp.set_extension(unique);
    fs::write(&temp, bytes)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            let _ = fs::remove_file(temp);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(temp);
            Err(error.into())
        }
    }
}

pub fn sealed_cache_file(key: &[u8; 32]) -> String {
    format!("{}.sealed", hex::encode(key))
}

pub fn sealed_nonce(key: &[u8; 32]) -> [u8; NONCE_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig sealed block nonce");
    hasher.update(key);
    let hash = hasher.finalize();
    let mut nonce = [0_u8; NONCE_LEN];
    nonce.copy_from_slice(&hash.as_bytes()[..NONCE_LEN]);
    nonce
}

pub fn sealed_key_id(key: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig sealed key id");
    hasher.update(key);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_index_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        assert!(cache.get(&hash).unwrap().is_none());
        cache.insert(&hash, 4, b"compressed").unwrap();
        cache.save().unwrap();
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get(&hash).unwrap().unwrap(), b"compressed");
    }

    #[test]
    fn old_cache_index_without_paths_deserializes() {
        let index: CacheIndex = serde_json::from_str(r#"{"records":{}}"#).unwrap();
        assert!(index.records.is_empty());
        assert!(index.paths.is_empty());
        let record: PathCacheRecord = serde_json::from_str(
            r#"{"relative_path":"a.txt","size":4,"mtime_ns":1,"permissions":420,"content_hash":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"last_seen_unix_ns":2}"#,
        )
        .unwrap();
        assert_eq!(record.chunk_size, None);
        assert!(record.chunks.is_empty());
    }

    #[test]
    fn path_cache_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache
            .upsert_path_record(PathCacheRecord {
                relative_path: "a.txt".to_string(),
                size: 4,
                mtime_ns: 123,
                permissions: 0o644,
                content_hash: hash,
                last_seen_unix_ns: 456,
                chunk_size: None,
                chunks: Vec::new(),
            })
            .unwrap();
        cache.save().unwrap();
        let reopened = CacheStore::open(temp.path()).unwrap();
        let record = reopened.get_path_record("a.txt").unwrap();
        assert_eq!(record.content_hash, hash);
        assert_eq!(record.mtime_ns, 123);
    }

    #[test]
    fn reusable_path_chunks_require_complete_coverage() {
        let hash = *blake3::hash(b"chunk").as_bytes();
        let mut record = PathCacheRecord {
            relative_path: "large.bin".to_string(),
            size: 16,
            mtime_ns: 123,
            permissions: 0o644,
            content_hash: hash,
            last_seen_unix_ns: 456,
            chunk_size: Some(8),
            chunks: vec![
                PathChunkRecord {
                    chunk_hash: hash,
                    file_offset: 0,
                    len: 8,
                },
                PathChunkRecord {
                    chunk_hash: hash,
                    file_offset: 8,
                    len: 8,
                },
            ],
        };
        assert!(reusable_path_chunks(&record, 16, 8).is_some());
        assert!(reusable_path_chunks(&record, 16, 4).is_none());
        record.chunks[1].file_offset = 9;
        assert!(reusable_path_chunks(&record, 16, 8).is_none());
        record.chunks[1].file_offset = 8;
        record.chunks[1].len = 9;
        assert!(reusable_path_chunks(&record, 16, 8).is_none());
    }

    #[test]
    fn sealed_nonce_is_stable_and_keyed_by_block() {
        let first = *blake3::hash(b"first").as_bytes();
        let second = *blake3::hash(b"second").as_bytes();
        assert_eq!(sealed_nonce(&first), sealed_nonce(&first));
        assert_ne!(sealed_nonce(&first), sealed_nonce(&second));
    }

    #[test]
    fn concurrent_sealed_writes_leave_complete_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let key = *blake3::hash(b"sealed-key").as_bytes();
        let ciphertext = vec![7_u8; 1024 * 1024];
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let root = root.clone();
                let ciphertext = ciphertext.clone();
                scope.spawn(move || {
                    let mut cache = CacheStore::open(root).unwrap();
                    cache
                        .insert_sealed(
                            &key,
                            SealedCacheRecord {
                                block_id: [1; 32],
                                nonce: [2; NONCE_LEN],
                                raw_size: ciphertext.len() as u64,
                                compressed_size: ciphertext.len() as u64,
                                encrypted_size: ciphertext.len() as u64,
                                sealed_file: sealed_cache_file(&key),
                                codec: "zstd".to_string(),
                                kind: "chunk".to_string(),
                            },
                            &ciphertext,
                        )
                        .unwrap();
                });
            }
        });
        assert_eq!(
            fs::read(root.join("blocks").join(sealed_cache_file(&key))).unwrap(),
            ciphertext
        );
    }

    #[test]
    fn sealed_cache_record_roundtrip_and_key_mismatch_clears_records() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        let kdf = KdfParams::default();
        let key = [7_u8; 32];
        let block_key = *blake3::hash(b"block").as_bytes();
        cache.prepare_sealed_key(kdf.clone(), &key);
        cache
            .insert_sealed(
                &block_key,
                SealedCacheRecord {
                    block_id: [1; 32],
                    nonce: [2; NONCE_LEN],
                    raw_size: 3,
                    compressed_size: 4,
                    encrypted_size: 6,
                    sealed_file: sealed_cache_file(&block_key),
                    codec: "zstd".to_string(),
                    kind: "chunk".to_string(),
                },
                b"sealed",
            )
            .unwrap();
        cache.save().unwrap();
        let reopened = CacheStore::open(temp.path()).unwrap();
        let record = reopened.get_sealed_record(&block_key).unwrap();
        assert_eq!(record.encrypted_size, 6);
        assert!(reopened.sealed_block_path(record).exists());

        let mut reopened = CacheStore::open(temp.path()).unwrap();
        reopened.prepare_sealed_key(kdf, &[8_u8; 32]);
        assert!(reopened.get_sealed_record(&block_key).is_none());
    }
}
