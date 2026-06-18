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
    pub records: BTreeMap<String, CacheRecord>,
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

    pub fn insert(
        &mut self,
        hash: &[u8; 32],
        original_size: u64,
        compressed: &[u8],
    ) -> anyhow::Result<()> {
        let hash_hex = hex::encode(hash);
        let block_file = format!("{hash_hex}.zst");
        fs::write(self.root.join("blocks").join(&block_file), compressed)?;
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
        self.save()
    }

    fn save(&self) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.index)?;
        fs::write(self.root.join("index.json"), bytes)?;
        Ok(())
    }
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
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get(&hash).unwrap().unwrap(), b"compressed");
    }
}
