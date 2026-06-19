use crate::crypto::{KdfParams, NONCE_LEN, SALT_LEN};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheSignature(Vec<(String, u64, u128)>);

#[derive(Debug, Clone)]
struct L1IndexEntry {
    signature: CacheSignature,
    index: CacheIndex,
    index_format: String,
    shards_read: usize,
}

static L1_INDEX_CACHE: OnceLock<Mutex<BTreeMap<PathBuf, L1IndexEntry>>> = OnceLock::new();

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheRecord {
    pub hash_hex: String,
    pub block_file: String,
    pub original_size: u64,
    pub compressed_size: u64,
    pub codec: String,
    #[serde(default)]
    pub level: Option<i32>,
    #[serde(default)]
    pub policy_version: Option<u16>,
    #[serde(default)]
    pub source_hash: Option<[u8; 32]>,
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
    #[serde(default)]
    pub objects: BTreeMap<String, CacheObjectRecord>,
    #[serde(default)]
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheObjectRecord {
    pub codec: String,
    pub level: i32,
    pub policy_version: u16,
    pub source_hash: [u8; 32],
    pub compressed_size: u64,
    pub kind: String,
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
    #[serde(default)]
    pub pack_file: Option<String>,
    #[serde(default)]
    pub pack_offset: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct CacheStore {
    root: PathBuf,
    index: CacheIndex,
    dirty: bool,
    index_format: String,
    shards_read: usize,
    shards_written: usize,
    dirty_shards: BTreeSet<String>,
    l1_index_hit: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheMaintenanceReport {
    pub total_bytes: u64,
    pub budget_bytes: u64,
    pub files: usize,
    pub removable_bytes: u64,
    pub removed_bytes: u64,
    pub compacted_bytes: u64,
    pub generation: u64,
    pub dry_run: bool,
}

impl CacheStore {
    pub fn open(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("blocks"))?;
        fs::create_dir_all(root.join("index-v2"))?;
        let root = root.canonicalize().unwrap_or(root);
        let index_path = root.join("index.json");
        let signature = cache_signature(&root)?;
        let cached = L1_INDEX_CACHE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow::anyhow!("L1 cache index lock poisoned"))?
            .get(&root)
            .filter(|entry| entry.signature == signature)
            .cloned();
        let l1_index_hit = cached.is_some();
        let migration_complete = root.join("index-v2").join(".complete").exists();
        let (index, index_format, shards_read) = if let Some(cached) = cached {
            (cached.index, cached.index_format, cached.shards_read)
        } else if has_v2_shards(&root)? {
            let (sharded, count) = read_v2_shards(&root)?;
            if index_path.exists() && !migration_complete {
                let mut legacy: CacheIndex = serde_json::from_slice(&fs::read(&index_path)?)?;
                merge_cache_index(&mut legacy, sharded);
                (legacy, "hybrid".to_string(), count)
            } else {
                (sharded, "index-v2".to_string(), count)
            }
        } else if index_path.exists() {
            (
                serde_json::from_slice(&fs::read(index_path)?)?,
                "json".to_string(),
                0,
            )
        } else {
            (CacheIndex::default(), "empty".to_string(), 0)
        };
        if !l1_index_hit {
            update_l1_index(&root, signature, &index, &index_format, shards_read)?;
        }
        Ok(Self {
            root,
            index,
            dirty: false,
            index_format,
            shards_read,
            shards_written: 0,
            dirty_shards: BTreeSet::new(),
            l1_index_hit,
        })
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
        self.dirty = true;
        self.dirty_shards.insert("meta".to_string());
        salt
    }

    pub fn prepare_sealed_key(&mut self, kdf: KdfParams, key: &[u8; 32]) {
        let key_id = sealed_key_id(key);
        if self.index.sealed_key_id != Some(key_id) {
            self.index.sealed_records.clear();
            let _ = fs::remove_dir_all(self.root.join("index-v2"));
            let _ = fs::create_dir_all(self.root.join("index-v2"));
            self.index.sealed_key_id = Some(key_id);
            self.dirty = true;
            self.dirty_shards.insert("meta".to_string());
        }
        if self.index.sealed_kdf_params.as_ref() != Some(&kdf) {
            self.index.sealed_kdf_params = Some(kdf);
            self.dirty = true;
            self.dirty_shards.insert("meta".to_string());
        }
    }

    pub fn get_sealed_record(&self, key: &[u8; 32]) -> Option<&SealedCacheRecord> {
        self.index.sealed_records.get(&hex::encode(key))
    }

    pub fn sealed_block_path(&self, record: &SealedCacheRecord) -> PathBuf {
        self.root.join("blocks").join(&record.sealed_file)
    }

    pub fn sealed_pack_path(&self, record: &SealedCacheRecord) -> Option<PathBuf> {
        record
            .pack_file
            .as_ref()
            .map(|file| self.root.join("sealed-packs").join(file))
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
        let record = CacheRecord {
            hash_hex: hash_hex.clone(),
            block_file,
            original_size,
            compressed_size: compressed.len() as u64,
            codec: "zstd".to_string(),
            level: None,
            policy_version: None,
            source_hash: None,
        };
        if self.index.records.get(&hash_hex) != Some(&record) {
            self.index.records.insert(hash_hex, record);
            self.dirty = true;
            self.mark_dirty_key(hash);
        }
        Ok(())
    }

    pub fn insert_parameterized(
        &mut self,
        key: &[u8; 32],
        source_hash: &[u8; 32],
        original_size: u64,
        level: i32,
        compressed: &[u8],
    ) -> anyhow::Result<()> {
        let key_hex = hex::encode(key);
        let block_file = format!("{key_hex}.zst");
        atomic_write(&self.root.join("blocks").join(&block_file), compressed)?;
        let record = CacheRecord {
            hash_hex: key_hex.clone(),
            block_file,
            original_size,
            compressed_size: compressed.len() as u64,
            codec: "zstd".to_string(),
            level: Some(level),
            policy_version: Some(2),
            source_hash: Some(*source_hash),
        };
        if self.index.records.get(&key_hex) != Some(&record) {
            self.index.records.insert(key_hex, record);
            self.dirty = true;
            self.mark_dirty_key(key);
        }
        self.record_object(key, source_hash, level, "single", compressed.len() as u64);
        Ok(())
    }

    pub fn record_object(
        &mut self,
        key: &[u8; 32],
        source_hash: &[u8; 32],
        level: i32,
        kind: &str,
        compressed_size: u64,
    ) {
        let key = hex::encode(key);
        let record = CacheObjectRecord {
            codec: "zstd".to_string(),
            level,
            policy_version: 2,
            source_hash: *source_hash,
            compressed_size,
            kind: kind.to_string(),
        };
        if self.index.objects.get(&key) != Some(&record) {
            self.index.objects.insert(key.clone(), record);
            self.dirty = true;
            self.mark_dirty_hex(&key);
        }
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
        self.dirty = true;
        self.mark_dirty_key(key);
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
        self.dirty = true;
        self.mark_dirty_key(hash);
        Ok(())
    }

    pub fn insert_sealed(
        &mut self,
        key: &[u8; 32],
        mut record: SealedCacheRecord,
        ciphertext: &[u8],
    ) -> anyhow::Result<()> {
        let block_path = self.root.join("blocks").join(&record.sealed_file);
        atomic_write(&block_path, ciphertext)?;
        if let Some((pack_file, offset)) = self.append_sealed_pack(ciphertext)? {
            record.pack_file = Some(pack_file);
            record.pack_offset = Some(offset);
        }
        self.index.sealed_records.insert(hex::encode(key), record);
        self.dirty = true;
        self.mark_dirty_key(key);
        Ok(())
    }

    pub fn upsert_path_record(&mut self, record: PathCacheRecord) -> anyhow::Result<()> {
        let path = record.relative_path.clone();
        if self.index.paths.get(&path) != Some(&record) {
            self.mark_dirty_path(&path);
            self.index.paths.insert(path, record);
            self.dirty = true;
        }
        Ok(())
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let completing_migration = matches!(self.index_format.as_str(), "json" | "hybrid");
        if completing_migration {
            self.mark_all_shards_dirty();
        }
        let bytes = serde_json::to_vec_pretty(&self.index)?;
        atomic_write(&self.root.join("index.json"), &bytes)?;
        self.write_v2_dirty_shards()?;
        if completing_migration {
            atomic_write(&self.root.join("index-v2").join(".complete"), b"v1\n")?;
        }
        self.index_format = "index-v2".to_string();
        self.dirty = false;
        update_l1_index(
            &self.root,
            cache_signature(&self.root)?,
            &self.index,
            &self.index_format,
            self.shards_read.max(self.shards_written),
        )?;
        Ok(())
    }

    pub fn maintenance_status(&self) -> anyhow::Result<CacheMaintenanceReport> {
        let (total_bytes, files) = directory_usage(&self.root)?;
        Ok(CacheMaintenanceReport {
            total_bytes,
            budget_bytes: cache_budget(&self.root)?,
            files,
            generation: self.index.generation,
            ..CacheMaintenanceReport::default()
        })
    }

    pub fn gc(&mut self, dry_run: bool) -> anyhow::Result<CacheMaintenanceReport> {
        let mut report = self.maintenance_status()?;
        report.dry_run = dry_run;
        if report.total_bytes <= report.budget_bytes {
            return Ok(report);
        }
        let mut candidates = fs::read_dir(self.root.join("blocks"))?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                if !metadata.is_file()
                    || entry.path().extension().and_then(|value| value.to_str()) == Some("sealed")
                {
                    return None;
                }
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default();
                Some((modified, metadata.len(), entry.path()))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|candidate| candidate.0);
        for (_, len, path) in candidates {
            if report.total_bytes.saturating_sub(report.removed_bytes) <= report.budget_bytes {
                break;
            }
            report.removable_bytes += len;
            if !dry_run {
                fs::remove_file(&path)?;
                report.removed_bytes += len;
                self.remove_index_for_block(&path);
            }
        }
        if !dry_run && report.removed_bytes > 0 {
            self.save()?;
        }
        Ok(report)
    }

    pub fn compact_sealed(&mut self, dry_run: bool) -> anyhow::Result<CacheMaintenanceReport> {
        let mut report = self.maintenance_status()?;
        report.dry_run = dry_run;
        let records = self
            .index
            .sealed_records
            .iter()
            .map(|(key, record)| (key.clone(), record.clone()))
            .collect::<Vec<_>>();
        report.compacted_bytes = records
            .iter()
            .map(|(_, record)| record.encrypted_size)
            .sum();
        if dry_run || records.is_empty() {
            return Ok(report);
        }
        let next_generation = self.index.generation.saturating_add(1);
        let pack_dir = self.root.join("sealed-packs");
        fs::create_dir_all(&pack_dir)?;
        let pack_file = format!("generation-{next_generation}.pack");
        let temp = pack_dir.join(format!(".{pack_file}.tmp-{}", std::process::id()));
        let final_path = pack_dir.join(&pack_file);
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        let mut offset = 0_u64;
        for (key, record) in records {
            let ciphertext = self.read_sealed_ciphertext(&record)?;
            anyhow::ensure!(
                ciphertext.len() as u64 == record.encrypted_size,
                "sealed cache record length mismatch during compaction"
            );
            output.write_all(&ciphertext)?;
            let updated = self
                .index
                .sealed_records
                .get_mut(&key)
                .expect("record exists");
            updated.pack_file = Some(pack_file.clone());
            updated.pack_offset = Some(offset);
            offset += ciphertext.len() as u64;
        }
        output.sync_all()?;
        fs::rename(&temp, &final_path)?;
        self.index.generation = next_generation;
        self.dirty = true;
        self.dirty_shards.insert("meta".to_string());
        self.mark_all_shards_dirty();
        self.save()?;
        for entry in fs::read_dir(&pack_dir)? {
            let entry = entry?;
            if entry.path() != final_path
                && entry.path().extension().and_then(|v| v.to_str()) == Some("pack")
            {
                let _ = fs::remove_file(entry.path());
            }
        }
        report.generation = next_generation;
        Ok(report)
    }

    fn read_sealed_ciphertext(&self, record: &SealedCacheRecord) -> anyhow::Result<Vec<u8>> {
        if let (Some(path), Some(offset)) = (self.sealed_pack_path(record), record.pack_offset)
            && path.exists()
        {
            let mut file = fs::File::open(path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0_u8; record.encrypted_size as usize];
            std::io::Read::read_exact(&mut file, &mut bytes)?;
            return Ok(bytes);
        }
        Ok(fs::read(self.sealed_block_path(record))?)
    }

    fn remove_index_for_block(&mut self, path: &Path) {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return;
        };
        let key = name.split('.').next().unwrap_or_default().to_string();
        self.index.records.remove(&key);
        self.index.objects.remove(&key);
        self.dirty = true;
        self.mark_dirty_hex(&key);
    }

    pub fn index_format(&self) -> &str {
        &self.index_format
    }

    pub fn shards_read(&self) -> usize {
        self.shards_read
    }

    pub fn shards_written(&self) -> usize {
        self.shards_written
    }

    pub fn dirty_shard_count(&self) -> usize {
        self.dirty_shards.len()
    }

    pub fn l1_index_hit(&self) -> bool {
        self.l1_index_hit
    }

    fn mark_dirty_key(&mut self, key: &[u8; 32]) {
        self.dirty_shards.insert(hex::encode(&key[..2]));
    }

    fn mark_dirty_hex(&mut self, key_hex: &str) {
        self.dirty_shards
            .insert(key_hex.chars().take(4).collect::<String>());
    }

    fn mark_dirty_path(&mut self, path: &str) {
        self.dirty_shards.insert(path_shard(path));
    }

    fn mark_all_shards_dirty(&mut self) {
        let record_shards = self
            .index
            .records
            .keys()
            .map(|key| key[..4].to_string())
            .collect::<Vec<_>>();
        let object_shards = self
            .index
            .objects
            .keys()
            .map(|key| key[..4].to_string())
            .collect::<Vec<_>>();
        let sealed_shards = self
            .index
            .sealed_records
            .keys()
            .map(|key| key[..4].to_string())
            .collect::<Vec<_>>();
        let path_shards = self
            .index
            .paths
            .keys()
            .map(|path| path_shard(path))
            .collect::<Vec<_>>();
        self.dirty_shards.extend(record_shards);
        self.dirty_shards.extend(object_shards);
        self.dirty_shards.extend(sealed_shards);
        self.dirty_shards.extend(path_shards);
        if self.dirty_shards.is_empty() {
            self.dirty_shards.insert("meta".to_string());
        }
    }

    fn write_v2_dirty_shards(&mut self) -> anyhow::Result<()> {
        let shards = self.dirty_shards.iter().cloned().collect::<Vec<_>>();
        for shard in shards {
            let index = self.shard_index(&shard);
            let bytes = bincode::serialize(&index)?;
            atomic_write(
                &self.root.join("index-v2").join(format!("{shard}.bin")),
                &bytes,
            )?;
            self.shards_written += 1;
        }
        self.dirty_shards.clear();
        Ok(())
    }

    fn shard_index(&self, shard: &str) -> CacheIndex {
        CacheIndex {
            records: self
                .index
                .records
                .iter()
                .filter(|(key, _)| key.starts_with(shard))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            paths: self
                .index
                .paths
                .iter()
                .filter(|(path, _)| path_shard(path) == shard)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            sealed_salt: self.index.sealed_salt,
            sealed_kdf_params: self.index.sealed_kdf_params.clone(),
            sealed_key_id: self.index.sealed_key_id,
            sealed_records: self
                .index
                .sealed_records
                .iter()
                .filter(|(key, _)| key.starts_with(shard))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            objects: self
                .index
                .objects
                .iter()
                .filter(|(key, _)| key.starts_with(shard))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            generation: self.index.generation,
        }
    }

    fn append_sealed_pack(&self, ciphertext: &[u8]) -> anyhow::Result<Option<(String, u64)>> {
        let Some(key_id) = self.index.sealed_key_id else {
            return Ok(None);
        };
        let pack_file = format!("{}.pack", hex::encode(key_id));
        let pack_dir = self.root.join("sealed-packs");
        fs::create_dir_all(&pack_dir)?;
        let pack_path = pack_dir.join(&pack_file);
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&pack_path)?;
        let offset = file.seek(SeekFrom::End(0))?;
        file.write_all(ciphertext)?;
        Ok(Some((pack_file, offset)))
    }
}

fn update_l1_index(
    root: &Path,
    signature: CacheSignature,
    index: &CacheIndex,
    index_format: &str,
    shards_read: usize,
) -> anyhow::Result<()> {
    L1_INDEX_CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| anyhow::anyhow!("L1 cache index lock poisoned"))?
        .insert(
            root.to_path_buf(),
            L1IndexEntry {
                signature,
                index: index.clone(),
                index_format: index_format.to_string(),
                shards_read,
            },
        );
    Ok(())
}

fn cache_signature(root: &Path) -> anyhow::Result<CacheSignature> {
    let mut entries = Vec::new();
    let json = root.join("index.json");
    if let Ok(metadata) = fs::metadata(&json) {
        entries.push(signature_entry("index.json".to_string(), &metadata));
    }
    let shard_dir = root.join("index-v2");
    if shard_dir.exists() {
        let complete = shard_dir.join(".complete");
        if let Ok(metadata) = fs::metadata(&complete) {
            entries.push(signature_entry(".complete".to_string(), &metadata));
        }
        for entry in fs::read_dir(shard_dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("bin") {
                continue;
            }
            entries.push(signature_entry(
                entry.file_name().to_string_lossy().to_string(),
                &entry.metadata()?,
            ));
        }
    }
    entries.sort_unstable();
    Ok(CacheSignature(entries))
}

fn merge_cache_index(base: &mut CacheIndex, newer: CacheIndex) {
    base.generation = base.generation.max(newer.generation);
    base.records.extend(newer.records);
    base.paths.extend(newer.paths);
    base.sealed_records.extend(newer.sealed_records);
    base.objects.extend(newer.objects);
    if newer.sealed_salt.is_some() {
        base.sealed_salt = newer.sealed_salt;
    }
    if newer.sealed_kdf_params.is_some() {
        base.sealed_kdf_params = newer.sealed_kdf_params;
    }
    if newer.sealed_key_id.is_some() {
        base.sealed_key_id = newer.sealed_key_id;
    }
}

fn signature_entry(name: String, metadata: &fs::Metadata) -> (String, u64, u128) {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    (name, metadata.len(), modified)
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

fn has_v2_shards(root: &Path) -> anyhow::Result<bool> {
    let dir = root.join("index-v2");
    if !dir.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|value| value.to_str()) == Some("bin") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_v2_shards(root: &Path) -> anyhow::Result<(CacheIndex, usize)> {
    let mut merged = CacheIndex::default();
    let mut count = 0;
    for entry in fs::read_dir(root.join("index-v2"))? {
        let entry = entry?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("bin") {
            continue;
        }
        let shard: CacheIndex = bincode::deserialize(&fs::read(entry.path())?)?;
        merged.generation = merged.generation.max(shard.generation);
        merged.records.extend(shard.records);
        merged.paths.extend(shard.paths);
        merged.sealed_records.extend(shard.sealed_records);
        merged.objects.extend(shard.objects);
        if shard.sealed_salt.is_some() {
            merged.sealed_salt = shard.sealed_salt;
        }
        if shard.sealed_kdf_params.is_some() {
            merged.sealed_kdf_params = shard.sealed_kdf_params;
        }
        if shard.sealed_key_id.is_some() {
            merged.sealed_key_id = shard.sealed_key_id;
        }
        count += 1;
    }
    Ok((merged, count))
}

fn directory_usage(root: &Path) -> anyhow::Result<(u64, usize)> {
    fn visit(path: &Path, bytes: &mut u64, files: &mut usize) -> anyhow::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                visit(&entry.path(), bytes, files)?;
            } else if metadata.is_file() {
                *bytes = bytes.saturating_add(metadata.len());
                *files += 1;
            }
        }
        Ok(())
    }

    let mut bytes = 0_u64;
    let mut files = 0_usize;
    visit(root, &mut bytes, &mut files)?;
    Ok((bytes, files))
}

fn cache_budget(root: &Path) -> anyhow::Result<u64> {
    const GIB: u64 = 1024 * 1024 * 1024;
    let available = fs2::available_space(root)?;
    let proportional = available / 10;
    let normal = proportional.clamp(5 * GIB, 50 * GIB);
    if available < 10 * GIB {
        Ok(normal.min(available.saturating_sub(2 * GIB)))
    } else {
        Ok(normal)
    }
}

fn path_shard(path: &str) -> String {
    let hash = blake3::hash(path.as_bytes());
    hex::encode(&hash.as_bytes()[..2])
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
        assert!(index.objects.is_empty());
        let record: PathCacheRecord = serde_json::from_str(
            r#"{"relative_path":"a.txt","size":4,"mtime_ns":1,"permissions":420,"content_hash":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"last_seen_unix_ns":2}"#,
        )
        .unwrap();
        assert_eq!(record.chunk_size, None);
        assert!(record.chunks.is_empty());
        let cache_record: CacheRecord = serde_json::from_str(
            r#"{"hash_hex":"00","block_file":"00.zst","original_size":1,"compressed_size":2,"codec":"zstd"}"#,
        )
        .unwrap();
        assert_eq!(cache_record.level, None);
        assert_eq!(cache_record.policy_version, None);
        assert_eq!(cache_record.source_hash, None);
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
    fn binary_shard_index_is_preferred_after_first_save() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.insert(&hash, 4, b"zstd").unwrap();
        cache.save().unwrap();

        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.index_format(), "index-v2");
        assert_eq!(reopened.shards_read(), 1);
        assert_eq!(reopened.get(&hash).unwrap().unwrap(), b"zstd");
    }

    #[test]
    fn corrupted_binary_shard_fails_fast() {
        let temp = tempfile::tempdir().unwrap();
        let shard_dir = temp.path().join("index-v2");
        fs::create_dir_all(&shard_dir).unwrap();
        fs::write(shard_dir.join("0000.bin"), b"not a cache shard").unwrap();
        assert!(CacheStore::open(temp.path()).is_err());
    }

    #[test]
    fn partial_v2_migration_keeps_legacy_records_until_completion() {
        let temp = tempfile::tempdir().unwrap();
        let first = *blake3::hash(b"first").as_bytes();
        let second = *blake3::hash(b"second").as_bytes();
        fs::create_dir_all(temp.path().join("blocks")).unwrap();
        fs::create_dir_all(temp.path().join("index-v2")).unwrap();
        let record = |hash: [u8; 32]| CacheRecord {
            hash_hex: hex::encode(hash),
            block_file: format!("{}.zst", hex::encode(hash)),
            original_size: 1,
            compressed_size: 1,
            codec: "zstd".to_string(),
            level: Some(1),
            policy_version: Some(1),
            source_hash: Some(hash),
        };
        let mut legacy = CacheIndex::default();
        legacy.records.insert(hex::encode(first), record(first));
        legacy.records.insert(hex::encode(second), record(second));
        fs::write(
            temp.path()
                .join("blocks")
                .join(format!("{}.zst", hex::encode(first))),
            b"a",
        )
        .unwrap();
        fs::write(
            temp.path()
                .join("blocks")
                .join(format!("{}.zst", hex::encode(second))),
            b"b",
        )
        .unwrap();
        fs::write(
            temp.path().join("index.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let mut partial = CacheIndex::default();
        partial.records.insert(hex::encode(first), record(first));
        fs::write(
            temp.path()
                .join("index-v2")
                .join(format!("{}.bin", hex::encode(&first[..2]))),
            bincode::serialize(&partial).unwrap(),
        )
        .unwrap();

        let mut cache = CacheStore::open(temp.path()).unwrap();
        assert_eq!(cache.index_format(), "hybrid");
        assert!(cache.has(&first));
        assert!(cache.has(&second));
        cache.dirty = true;
        cache.save().unwrap();

        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.index_format(), "index-v2");
        assert!(reopened.has(&first));
        assert!(reopened.has(&second));
    }

    #[test]
    fn parameterized_object_metadata_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let source = *blake3::hash(b"source").as_bytes();
        let key = *blake3::hash(b"parameterized").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache
            .insert_parameterized(&key, &source, 6, 5, b"compressed")
            .unwrap();
        cache.save().unwrap();
        let reopened = CacheStore::open(temp.path()).unwrap();
        let record = reopened.index.objects.get(&hex::encode(key)).unwrap();
        assert_eq!(record.level, 5);
        assert_eq!(record.policy_version, 2);
        assert_eq!(record.source_hash, source);
        assert_eq!(record.kind, "single");
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
                                pack_file: None,
                                pack_offset: None,
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
                    pack_file: None,
                    pack_offset: None,
                },
                b"sealed",
            )
            .unwrap();
        cache.save().unwrap();
        let reopened = CacheStore::open(temp.path()).unwrap();
        let record = reopened.get_sealed_record(&block_key).unwrap();
        assert_eq!(record.encrypted_size, 6);
        assert!(reopened.sealed_block_path(record).exists());
        assert!(record.pack_file.is_some());
        assert!(reopened.sealed_pack_path(record).unwrap().exists());

        let mut reopened = CacheStore::open(temp.path()).unwrap();
        reopened.prepare_sealed_key(kdf, &[8_u8; 32]);
        assert!(reopened.get_sealed_record(&block_key).is_none());
    }

    #[test]
    fn old_sealed_cache_record_without_pack_fields_deserializes() {
        let record: SealedCacheRecord = serde_json::from_str(
            r#"{"block_id":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"nonce":[2,2,2,2,2,2,2,2,2,2,2,2],"raw_size":3,"compressed_size":4,"encrypted_size":6,"sealed_file":"a.sealed","codec":"zstd","kind":"chunk"}"#,
        )
        .unwrap();
        assert!(record.pack_file.is_none());
        assert!(record.pack_offset.is_none());
    }

    #[test]
    fn sealed_pack_record_points_to_appended_ciphertext() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.prepare_sealed_key(KdfParams::default(), &[7_u8; 32]);
        let key = *blake3::hash(b"block").as_bytes();
        cache
            .insert_sealed(
                &key,
                SealedCacheRecord {
                    block_id: [1; 32],
                    nonce: [2; NONCE_LEN],
                    raw_size: 3,
                    compressed_size: 4,
                    encrypted_size: 6,
                    sealed_file: sealed_cache_file(&key),
                    codec: "zstd".to_string(),
                    kind: "chunk".to_string(),
                    pack_file: None,
                    pack_offset: None,
                },
                b"sealed",
            )
            .unwrap();
        let record = cache.get_sealed_record(&key).unwrap();
        let pack = cache.sealed_pack_path(record).unwrap();
        let offset = record.pack_offset.unwrap();
        let bytes = fs::read(pack).unwrap();
        assert_eq!(&bytes[offset as usize..offset as usize + 6], b"sealed");
    }

    #[test]
    fn cache_maintenance_dry_run_does_not_modify_generation() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        let before = cache.maintenance_status().unwrap();
        let gc = cache.gc(true).unwrap();
        let compact = cache.compact_sealed(true).unwrap();
        assert!(gc.dry_run);
        assert!(compact.dry_run);
        assert_eq!(before.generation, cache.index.generation);
    }

    #[test]
    fn sealed_compaction_switches_generation_and_preserves_payload() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.prepare_sealed_key(KdfParams::default(), &[9_u8; 32]);
        let key = *blake3::hash(b"compact-block").as_bytes();
        cache
            .insert_sealed(
                &key,
                SealedCacheRecord {
                    block_id: [3; 32],
                    nonce: [4; NONCE_LEN],
                    raw_size: 7,
                    compressed_size: 7,
                    encrypted_size: 7,
                    sealed_file: sealed_cache_file(&key),
                    codec: "zstd".to_string(),
                    kind: "chunk".to_string(),
                    pack_file: None,
                    pack_offset: None,
                },
                b"payload",
            )
            .unwrap();
        let report = cache.compact_sealed(false).unwrap();
        assert_eq!(report.generation, 1);
        let record = cache.get_sealed_record(&key).unwrap();
        assert_eq!(cache.read_sealed_ciphertext(record).unwrap(), b"payload");
    }
}
