use crate::adaptive_io::{AdaptiveIoController, IoDirection};
use crate::crypto::{KdfParams, NONCE_LEN, SALT_LEN};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
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

const JOURNAL_MAGIC: &[u8; 4] = b"HJ01";
const JOURNAL_VERSION: u16 = 2;
const JOURNAL_COMPACT_BYTES: u64 = 8 * 1024 * 1024;
const JOURNAL_COMPACT_ENTRIES: u64 = 4096;
const HOT_PAYLOAD_BUDGET_BYTES: usize = 128 * 1024 * 1024;

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
    #[serde(default)]
    pub pack_file: Option<String>,
    #[serde(default)]
    pub pack_offset: Option<u64>,
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
    #[serde(default)]
    pub pack_file: Option<String>,
    #[serde(default)]
    pub pack_offset: Option<u64>,
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

#[derive(Debug)]
pub struct CacheStore {
    root: PathBuf,
    index: CacheIndex,
    dirty: bool,
    index_format: String,
    shards_read: usize,
    shards_written: usize,
    dirty_shards: BTreeSet<String>,
    dirty_records: BTreeSet<String>,
    dirty_paths: BTreeSet<String>,
    dirty_objects: BTreeSet<String>,
    dirty_sealed_records: BTreeSet<String>,
    dirty_meta: bool,
    requires_shard_rewrite: bool,
    l1_index_hit: bool,
    journal_entries_written: u64,
    journal_entries_replayed: u64,
    last_commit_mode: String,
    journal_upsert_records: u64,
    journal_upsert_paths: u64,
    journal_upsert_objects: u64,
    journal_upsert_sealed: u64,
    hot_payloads: BTreeMap<String, Vec<u8>>,
    hot_payload_bytes: usize,
    object_pack_writer: Option<ObjectPackWriter>,
    io_controller: Option<Arc<AdaptiveIoController>>,
}

struct ObjectPackWriter {
    file_name: String,
    file: fs::File,
}

impl std::fmt::Debug for ObjectPackWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObjectPackWriter")
            .field("file_name", &self.file_name)
            .finish_non_exhaustive()
    }
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
    pub journal_bytes: u64,
    pub journal_entries: u64,
    pub journal_replayed_entries: u64,
    pub journal_compacted_entries: u64,
    pub journal_dirty_record_estimate: u64,
    pub journal_compact_recommended: bool,
    pub journal_estimated_reclaimed_bytes: u64,
    pub last_compact_unix_ns: i128,
    pub journal_upsert_records: u64,
    pub journal_upsert_paths: u64,
    pub journal_upsert_objects: u64,
    pub journal_upsert_sealed: u64,
    pub journal_dirty_records: u64,
    pub journal_dirty_paths: u64,
    pub journal_dirty_objects: u64,
    pub journal_dirty_sealed: u64,
    pub cache_commit_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CacheJournalEntry {
    Delta(CacheIndex),
    Upserts(CacheJournalDeltaV2),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheJournalDeltaV2 {
    #[serde(default)]
    records: Vec<(String, CacheRecord)>,
    #[serde(default)]
    paths: Vec<(String, PathCacheRecord)>,
    #[serde(default)]
    sealed_records: Vec<(String, SealedCacheRecord)>,
    #[serde(default)]
    objects: Vec<(String, CacheObjectRecord)>,
    #[serde(default)]
    meta: CacheMetaDelta,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheMetaDelta {
    #[serde(default)]
    sealed_salt: Option<[u8; SALT_LEN]>,
    #[serde(default)]
    sealed_kdf_params: Option<KdfParams>,
    #[serde(default)]
    sealed_key_id: Option<[u8; 32]>,
    #[serde(default)]
    generation: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheSaveOptions {
    pub refresh_l1: bool,
}

impl Default for CacheSaveOptions {
    fn default() -> Self {
        Self { refresh_l1: true }
    }
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
        let (mut index, mut index_format, shards_read) = if let Some(cached) = cached {
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
        let journal_entries_replayed = if l1_index_hit {
            0
        } else {
            let replayed = replay_journal(&root, &mut index)?;
            if replayed > 0 && index_format == "empty" {
                index_format = "journal".to_string();
            }
            replayed
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
            dirty_records: BTreeSet::new(),
            dirty_paths: BTreeSet::new(),
            dirty_objects: BTreeSet::new(),
            dirty_sealed_records: BTreeSet::new(),
            dirty_meta: false,
            requires_shard_rewrite: false,
            l1_index_hit,
            journal_entries_written: 0,
            journal_entries_replayed,
            last_commit_mode: "none".to_string(),
            journal_upsert_records: 0,
            journal_upsert_paths: 0,
            journal_upsert_objects: 0,
            journal_upsert_sealed: 0,
            hot_payloads: BTreeMap::new(),
            hot_payload_bytes: 0,
            object_pack_writer: None,
            io_controller: None,
        })
    }

    pub(crate) fn set_io_controller(&mut self, controller: Arc<AdaptiveIoController>) {
        self.io_controller = Some(controller);
    }

    pub fn get(&self, hash: &[u8; 32]) -> anyhow::Result<Option<Vec<u8>>> {
        let hash_hex = hex::encode(hash);
        if let Some(bytes) = self.hot_payloads.get(&format!("record:{hash_hex}")) {
            return Ok(Some(bytes.clone()));
        }
        let Some(record) = self.index.records.get(&hash_hex) else {
            return Ok(None);
        };
        if let Some(bytes) = self.read_object_pack(
            record.pack_file.as_deref(),
            record.pack_offset,
            record.compressed_size,
        )? {
            return Ok(Some(bytes));
        }
        let block_path = self.root.join("blocks").join(&record.block_file);
        if !block_path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read(block_path)?))
    }

    pub fn has(&self, hash: &[u8; 32]) -> bool {
        let hash_hex = hex::encode(hash);
        if self
            .hot_payloads
            .contains_key(&format!("record:{hash_hex}"))
        {
            return true;
        }
        self.index.records.get(&hash_hex).is_some_and(|record| {
            self.object_pack_exists(
                record.pack_file.as_deref(),
                record.pack_offset,
                record.compressed_size,
            ) || self.root.join("blocks").join(&record.block_file).exists()
        })
    }

    pub fn has_batch(&self, key: &[u8; 32]) -> bool {
        let key_hex = hex::encode(key);
        if self.hot_payloads.contains_key(&format!("batch:{key_hex}")) {
            return true;
        }
        if self.index.objects.get(&key_hex).is_some_and(|record| {
            matches!(record.kind.as_str(), "batch" | "solid")
                && self.object_pack_exists(
                    record.pack_file.as_deref(),
                    record.pack_offset,
                    record.compressed_size,
                )
        }) {
            return true;
        }
        self.root
            .join("blocks")
            .join(format!("{key_hex}.batch.zst"))
            .exists()
    }

    pub fn has_chunk(&self, hash: &[u8; 32]) -> bool {
        let hash_hex = hex::encode(hash);
        if self.hot_payloads.contains_key(&format!("chunk:{hash_hex}")) {
            return true;
        }
        if self.index.objects.get(&hash_hex).is_some_and(|record| {
            record.kind == "chunk"
                && self.object_pack_exists(
                    record.pack_file.as_deref(),
                    record.pack_offset,
                    record.compressed_size,
                )
        }) {
            return true;
        }
        self.root
            .join("blocks")
            .join(format!("{hash_hex}.chunk.zst"))
            .exists()
    }

    pub fn get_batch(&self, key: &[u8; 32]) -> anyhow::Result<Option<Vec<u8>>> {
        let key_hex = hex::encode(key);
        if let Some(bytes) = self.hot_payloads.get(&format!("batch:{key_hex}")) {
            return Ok(Some(bytes.clone()));
        }
        if let Some(record) = self.index.objects.get(&key_hex)
            && matches!(record.kind.as_str(), "batch" | "solid")
            && let Some(bytes) = self.read_object_pack(
                record.pack_file.as_deref(),
                record.pack_offset,
                record.compressed_size,
            )?
        {
            return Ok(Some(bytes));
        }
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
        if let Some(bytes) = self.hot_payloads.get(&format!("chunk:{hash_hex}")) {
            return Ok(Some(bytes.clone()));
        }
        if let Some(record) = self.index.objects.get(&hash_hex)
            && record.kind == "chunk"
            && let Some(bytes) = self.read_object_pack(
                record.pack_file.as_deref(),
                record.pack_offset,
                record.compressed_size,
            )?
        {
            return Ok(Some(bytes));
        }
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
        self.mark_dirty_meta();
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
            self.mark_dirty_meta();
        }
        if self.index.sealed_kdf_params.as_ref() != Some(&kdf) {
            self.index.sealed_kdf_params = Some(kdf);
            self.dirty = true;
            self.mark_dirty_meta();
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
        self.cache_hot_payload(format!("record:{hash_hex}"), compressed);
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
            pack_file: None,
            pack_offset: None,
        };
        if self.index.records.get(&hash_hex) != Some(&record) {
            self.index.records.insert(hash_hex.clone(), record);
            self.dirty = true;
            self.mark_dirty_record_key(hash);
            self.dirty_records.insert(hash_hex);
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
        self.insert_parameterized_impl(key, source_hash, original_size, level, compressed, true)
    }

    pub(crate) fn insert_parameterized_for_pipeline(
        &mut self,
        key: &[u8; 32],
        source_hash: &[u8; 32],
        original_size: u64,
        level: i32,
        compressed: &[u8],
    ) -> anyhow::Result<()> {
        self.insert_parameterized_impl(key, source_hash, original_size, level, compressed, false)
    }

    fn insert_parameterized_impl(
        &mut self,
        key: &[u8; 32],
        source_hash: &[u8; 32],
        original_size: u64,
        level: i32,
        compressed: &[u8],
        cache_hot: bool,
    ) -> anyhow::Result<()> {
        let key_hex = hex::encode(key);
        if cache_hot {
            self.cache_hot_payload(format!("record:{key_hex}"), compressed);
        }
        let pack_location = self.append_object_pack(compressed)?;
        let block_file = format!("{key_hex}.zst");
        let record = CacheRecord {
            hash_hex: key_hex.clone(),
            block_file,
            original_size,
            compressed_size: compressed.len() as u64,
            codec: "zstd".to_string(),
            level: Some(level),
            policy_version: Some(2),
            source_hash: Some(*source_hash),
            pack_file: Some(pack_location.0.clone()),
            pack_offset: Some(pack_location.1),
        };
        if self.index.records.get(&key_hex) != Some(&record) {
            self.index.records.insert(key_hex.clone(), record);
            self.dirty = true;
            self.mark_dirty_record_key(key);
            self.dirty_records.insert(key_hex);
        }
        self.record_object_with_location(
            key,
            source_hash,
            level,
            "single",
            compressed.len() as u64,
            Some(pack_location),
        );
        Ok(())
    }

    pub fn record_object_with_location(
        &mut self,
        key: &[u8; 32],
        source_hash: &[u8; 32],
        level: i32,
        kind: &str,
        compressed_size: u64,
        pack_location: Option<(String, u64)>,
    ) {
        let key = hex::encode(key);
        let record = CacheObjectRecord {
            codec: "zstd".to_string(),
            level,
            policy_version: 2,
            source_hash: *source_hash,
            compressed_size,
            kind: kind.to_string(),
            pack_file: pack_location.as_ref().map(|location| location.0.clone()),
            pack_offset: pack_location.map(|location| location.1),
        };
        if self.index.objects.get(&key) != Some(&record) {
            self.index.objects.insert(key.clone(), record);
            self.dirty = true;
            self.mark_dirty_object_hex(&key);
            self.dirty_objects.insert(key);
        }
    }

    pub fn insert_batch(
        &mut self,
        key: &[u8; 32],
        compressed: &[u8],
    ) -> anyhow::Result<(String, u64)> {
        self.insert_batch_impl(key, compressed, true)
    }

    pub(crate) fn insert_batch_for_pipeline(
        &mut self,
        key: &[u8; 32],
        compressed: &[u8],
    ) -> anyhow::Result<(String, u64)> {
        self.insert_batch_impl(key, compressed, false)
    }

    fn insert_batch_impl(
        &mut self,
        key: &[u8; 32],
        compressed: &[u8],
        cache_hot: bool,
    ) -> anyhow::Result<(String, u64)> {
        let key_hex = hex::encode(key);
        if cache_hot {
            self.cache_hot_payload(format!("batch:{key_hex}"), compressed);
        }
        self.append_object_pack(compressed)
    }

    pub fn insert_chunk(
        &mut self,
        hash: &[u8; 32],
        compressed: &[u8],
    ) -> anyhow::Result<(String, u64)> {
        self.insert_chunk_impl(hash, compressed, true)
    }

    pub(crate) fn insert_chunk_for_pipeline(
        &mut self,
        hash: &[u8; 32],
        compressed: &[u8],
    ) -> anyhow::Result<(String, u64)> {
        self.insert_chunk_impl(hash, compressed, false)
    }

    fn insert_chunk_impl(
        &mut self,
        hash: &[u8; 32],
        compressed: &[u8],
        cache_hot: bool,
    ) -> anyhow::Result<(String, u64)> {
        let hash_hex = hex::encode(hash);
        if cache_hot {
            self.cache_hot_payload(format!("chunk:{hash_hex}"), compressed);
        }
        self.append_object_pack(compressed)
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
        let key_hex = hex::encode(key);
        if self.index.sealed_records.get(&key_hex) != Some(&record) {
            self.index.sealed_records.insert(key_hex.clone(), record);
            self.dirty = true;
            self.mark_dirty_sealed_key(key);
            self.dirty_sealed_records.insert(key_hex);
        }
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
        self.save_with_options(CacheSaveOptions::default())
    }

    pub fn save_with_options(&mut self, options: CacheSaveOptions) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.flush_object_pack()?;
        let completing_migration = matches!(self.index_format.as_str(), "json" | "hybrid");
        let should_compact = self.journal_should_compact()?;
        if completing_migration || should_compact || self.requires_shard_rewrite {
            self.mark_all_shards_dirty();
            if completing_migration {
                let bytes = serde_json::to_vec_pretty(&self.index)?;
                atomic_write(&self.root.join("index.json"), &bytes)?;
            }
            self.write_v2_dirty_shards()?;
            self.truncate_journal()?;
            atomic_write(&self.root.join("index-v2").join(".complete"), b"v1\n")?;
            self.last_commit_mode = "shards".to_string();
        } else {
            self.append_journal_delta()?;
            self.last_commit_mode = "journal-upserts".to_string();
        }
        self.index_format = "index-v2".to_string();
        self.dirty = false;
        if options.refresh_l1 {
            update_l1_index(
                &self.root,
                cache_signature(&self.root)?,
                &self.index,
                &self.index_format,
                self.shards_read.max(self.shards_written),
            )?;
        }
        Ok(())
    }

    pub fn maintenance_status(&self) -> anyhow::Result<CacheMaintenanceReport> {
        let (total_bytes, files) = directory_usage(&self.root)?;
        let (journal_bytes, journal_entries) = journal_stats(&self.root)?;
        Ok(CacheMaintenanceReport {
            total_bytes,
            budget_bytes: cache_budget(&self.root)?,
            files,
            generation: self.index.generation,
            journal_bytes,
            journal_entries,
            journal_replayed_entries: self.journal_entries_replayed,
            journal_dirty_record_estimate: journal_entries,
            journal_compact_recommended: journal_entries >= JOURNAL_COMPACT_ENTRIES
                || journal_bytes >= JOURNAL_COMPACT_BYTES,
            journal_estimated_reclaimed_bytes: journal_bytes,
            last_compact_unix_ns: 0,
            journal_upsert_records: self.journal_upsert_records,
            journal_upsert_paths: self.journal_upsert_paths,
            journal_upsert_objects: self.journal_upsert_objects,
            journal_upsert_sealed: self.journal_upsert_sealed,
            journal_dirty_records: self.dirty_records.len() as u64,
            journal_dirty_paths: self.dirty_paths.len() as u64,
            journal_dirty_objects: self.dirty_objects.len() as u64,
            journal_dirty_sealed: self.dirty_sealed_records.len() as u64,
            cache_commit_mode: self.last_commit_mode.clone(),
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
            self.requires_shard_rewrite = true;
            self.save()?;
        }
        Ok(report)
    }

    pub fn compact_sealed(&mut self, dry_run: bool) -> anyhow::Result<CacheMaintenanceReport> {
        let mut report = self.maintenance_status()?;
        report.dry_run = dry_run;
        if !dry_run {
            report.journal_compacted_entries = self.compact_journal_to_shards()?;
        }
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
        self.mark_dirty_meta();
        self.mark_all_shards_dirty();
        self.compact_journal_to_shards()?;
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
        self.requires_shard_rewrite = true;
        self.mark_dirty_record_hex(&key);
        self.mark_dirty_object_hex(&key);
    }

    pub fn index_format(&self) -> &str {
        &self.index_format
    }

    pub fn generation(&self) -> u64 {
        self.index.generation
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

    pub fn last_commit_mode(&self) -> &str {
        &self.last_commit_mode
    }

    pub fn journal_upsert_counts(&self) -> (u64, u64, u64, u64) {
        (
            self.journal_upsert_records,
            self.journal_upsert_paths,
            self.journal_upsert_objects,
            self.journal_upsert_sealed,
        )
    }

    pub fn warm_parameterized_payloads(&mut self) -> anyhow::Result<(u64, u64)> {
        let objects = self
            .index
            .objects
            .iter()
            .map(|(key, record)| (key.clone(), record.kind.clone()))
            .collect::<Vec<_>>();
        let mut count = 0_u64;
        let mut bytes = 0_u64;
        for (key, kind) in objects {
            let hot_key = match kind.as_str() {
                "single" => format!("record:{key}"),
                "batch" | "solid" => format!("batch:{key}"),
                "chunk" => format!("chunk:{key}"),
                _ => continue,
            };
            if self.hot_payloads.contains_key(&hot_key) {
                continue;
            }
            let Ok(Some(payload)) = match kind.as_str() {
                "single" => hex_to_key(&key).map(|key| self.get(&key)),
                "batch" | "solid" => hex_to_key(&key).map(|key| self.get_batch(&key)),
                "chunk" => hex_to_key(&key).map(|key| self.get_chunk(&key)),
                _ => None,
            }
            .unwrap_or_else(|| Ok(None)) else {
                continue;
            };
            bytes += payload.len() as u64;
            count += 1;
            self.cache_hot_payload(hot_key, &payload);
        }
        Ok((count, bytes))
    }

    fn cache_hot_payload(&mut self, key: String, payload: &[u8]) {
        if payload.len() > HOT_PAYLOAD_BUDGET_BYTES {
            return;
        }
        if let Some(previous) = self.hot_payloads.remove(&key) {
            self.hot_payload_bytes = self.hot_payload_bytes.saturating_sub(previous.len());
        }
        while self.hot_payload_bytes + payload.len() > HOT_PAYLOAD_BUDGET_BYTES {
            let Some(oldest) = self.hot_payloads.keys().next().cloned() else {
                break;
            };
            if let Some(removed) = self.hot_payloads.remove(&oldest) {
                self.hot_payload_bytes = self.hot_payload_bytes.saturating_sub(removed.len());
            }
        }
        self.hot_payload_bytes += payload.len();
        self.hot_payloads.insert(key, payload.to_vec());
    }

    fn mark_dirty_record_key(&mut self, key: &[u8; 32]) {
        self.dirty_shards.insert(hex::encode(&key[..2]));
    }

    fn mark_dirty_record_hex(&mut self, key_hex: &str) {
        self.dirty_shards
            .insert(key_hex.chars().take(4).collect::<String>());
    }

    fn mark_dirty_object_hex(&mut self, key_hex: &str) {
        self.dirty_shards
            .insert(key_hex.chars().take(4).collect::<String>());
    }

    fn mark_dirty_sealed_key(&mut self, key: &[u8; 32]) {
        self.dirty_shards.insert(hex::encode(&key[..2]));
    }

    fn mark_dirty_path(&mut self, path: &str) {
        self.dirty_shards.insert(path_shard(path));
        self.dirty_paths.insert(path.to_string());
    }

    fn mark_dirty_meta(&mut self) {
        self.dirty_shards.insert("meta".to_string());
        self.dirty_meta = true;
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

    fn clear_dirty_tracking(&mut self) {
        self.dirty_shards.clear();
        self.dirty_records.clear();
        self.dirty_paths.clear();
        self.dirty_objects.clear();
        self.dirty_sealed_records.clear();
        self.dirty_meta = false;
        self.requires_shard_rewrite = false;
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
        self.clear_dirty_tracking();
        Ok(())
    }

    fn append_journal_delta(&mut self) -> anyhow::Result<()> {
        let mut delta = CacheJournalDeltaV2::default();
        for key in &self.dirty_records {
            if let Some(record) = self.index.records.get(key) {
                delta.records.push((key.clone(), record.clone()));
            }
        }
        for path in &self.dirty_paths {
            if let Some(record) = self.index.paths.get(path) {
                delta.paths.push((path.clone(), record.clone()));
            }
        }
        for key in &self.dirty_sealed_records {
            if let Some(record) = self.index.sealed_records.get(key) {
                delta.sealed_records.push((key.clone(), record.clone()));
            }
        }
        for key in &self.dirty_objects {
            if let Some(record) = self.index.objects.get(key) {
                delta.objects.push((key.clone(), record.clone()));
            }
        }
        if self.dirty_meta {
            delta.meta = CacheMetaDelta {
                sealed_salt: self.index.sealed_salt,
                sealed_kdf_params: self.index.sealed_kdf_params.clone(),
                sealed_key_id: self.index.sealed_key_id,
                generation: Some(self.index.generation),
            };
        }
        let upsert_records = delta.records.len() as u64;
        let upsert_paths = delta.paths.len() as u64;
        let upsert_objects = delta.objects.len() as u64;
        let upsert_sealed = delta.sealed_records.len() as u64;
        if upsert_records + upsert_paths + upsert_objects + upsert_sealed > 0 || self.dirty_meta {
            append_journal_entry(&self.root, &CacheJournalEntry::Upserts(delta))?;
            self.journal_entries_written += 1;
        }
        self.journal_upsert_records = upsert_records;
        self.journal_upsert_paths = upsert_paths;
        self.journal_upsert_objects = upsert_objects;
        self.journal_upsert_sealed = upsert_sealed;
        self.clear_dirty_tracking();
        Ok(())
    }

    fn journal_should_compact(&self) -> anyhow::Result<bool> {
        let (bytes, entries) = journal_stats(&self.root)?;
        Ok(bytes >= JOURNAL_COMPACT_BYTES || entries >= JOURNAL_COMPACT_ENTRIES)
    }

    fn truncate_journal(&self) -> anyhow::Result<()> {
        let path = journal_path(&self.root);
        if path.exists() {
            atomic_write(&path, b"")?;
        }
        Ok(())
    }

    fn compact_journal_to_shards(&mut self) -> anyhow::Result<u64> {
        let (_, entries) = journal_stats(&self.root)?;
        self.mark_all_shards_dirty();
        self.write_v2_dirty_shards()?;
        self.truncate_journal()?;
        atomic_write(&self.root.join("index-v2").join(".complete"), b"v1\n")?;
        self.index_format = "index-v2".to_string();
        self.dirty = false;
        self.clear_dirty_tracking();
        update_l1_index(
            &self.root,
            cache_signature(&self.root)?,
            &self.index,
            &self.index_format,
            self.shards_read.max(self.shards_written),
        )?;
        Ok(entries)
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

    fn object_pack_path(&self, pack_file: &str) -> PathBuf {
        self.root.join("object-packs").join(pack_file)
    }

    fn object_pack_exists(
        &self,
        pack_file: Option<&str>,
        pack_offset: Option<u64>,
        compressed_size: u64,
    ) -> bool {
        let (Some(pack_file), Some(offset)) = (pack_file, pack_offset) else {
            return false;
        };
        let Ok(metadata) = fs::metadata(self.object_pack_path(pack_file)) else {
            return false;
        };
        offset
            .checked_add(compressed_size)
            .is_some_and(|end| metadata.len() >= end)
    }

    fn read_object_pack(
        &self,
        pack_file: Option<&str>,
        pack_offset: Option<u64>,
        compressed_size: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.object_pack_exists(pack_file, pack_offset, compressed_size) {
            return Ok(None);
        }
        let pack_file = pack_file.expect("checked above");
        let offset = pack_offset.expect("checked above");
        let mut file = fs::File::open(self.object_pack_path(pack_file))?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0_u8; compressed_size as usize];
        for chunk in bytes.chunks_mut(1024 * 1024) {
            let permit = self.io_controller.as_ref().map(|controller| {
                controller.acquire("cache-pack-read", IoDirection::Read, chunk.len() as u64)
            });
            file.read_exact(chunk)?;
            if let Some(permit) = permit {
                permit.finish_with_bytes(chunk.len() as u64);
            }
        }
        Ok(Some(bytes))
    }

    fn append_object_pack(&mut self, payload: &[u8]) -> anyhow::Result<(String, u64)> {
        let pack_file = "objects.pack".to_string();
        if self.object_pack_writer.is_none() {
            let pack_dir = self.root.join("object-packs");
            fs::create_dir_all(&pack_dir)?;
            let pack_path = pack_dir.join(&pack_file);
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&pack_path)?;
            self.object_pack_writer = Some(ObjectPackWriter {
                file_name: pack_file,
                file,
            });
        }
        let writer = self
            .object_pack_writer
            .as_mut()
            .expect("object pack writer initialized");
        let offset = writer.file.seek(SeekFrom::End(0))?;
        for chunk in payload.chunks(1024 * 1024) {
            let permit = self.io_controller.as_ref().map(|controller| {
                controller.acquire("cache-pack-write", IoDirection::Write, chunk.len() as u64)
            });
            writer.file.write_all(chunk)?;
            if let Some(permit) = permit {
                permit.finish_with_bytes(chunk.len() as u64);
            }
        }
        Ok((writer.file_name.clone(), offset))
    }

    fn flush_object_pack(&mut self) -> anyhow::Result<()> {
        if let Some(writer) = self.object_pack_writer.as_mut() {
            let permit = self
                .io_controller
                .as_ref()
                .map(|controller| controller.acquire("cache-pack-flush", IoDirection::Write, 0));
            writer.file.flush()?;
            if let Some(permit) = permit {
                permit.finish_with_bytes(0);
            }
        }
        Ok(())
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
    let journal = journal_path(root);
    if let Ok(metadata) = fs::metadata(&journal) {
        entries.push(signature_entry("journal.bin".to_string(), &metadata));
    }
    let shard_dir = root.join("index-v2");
    if shard_dir.exists() {
        let complete = shard_dir.join(".complete");
        if let Ok(metadata) = fs::metadata(&complete) {
            entries.push(signature_entry(".complete".to_string(), &metadata));
        }
        for entry in fs::read_dir(shard_dir)? {
            let entry = entry?;
            if entry.file_name() == "journal.bin" {
                continue;
            }
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

fn journal_path(root: &Path) -> PathBuf {
    root.join("index-v2").join("journal.bin")
}

fn append_journal_entry(root: &Path, entry: &CacheJournalEntry) -> anyhow::Result<()> {
    fs::create_dir_all(root.join("index-v2"))?;
    let payload = bincode::serialize(entry)?;
    anyhow::ensure!(
        payload.len() <= u32::MAX as usize,
        "cache journal entry is too large"
    );
    let checksum = blake3::hash(&payload);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal_path(root))?;
    file.write_all(JOURNAL_MAGIC)?;
    file.write_all(&JOURNAL_VERSION.to_le_bytes())?;
    file.write_all(&(payload.len() as u32).to_le_bytes())?;
    file.write_all(&payload)?;
    file.write_all(checksum.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn replay_journal(root: &Path, index: &mut CacheIndex) -> anyhow::Result<u64> {
    let path = journal_path(root);
    if !path.exists() {
        return Ok(0);
    }
    let mut file = fs::File::open(path)?;
    let mut replayed = 0_u64;
    loop {
        let mut magic = [0_u8; 4];
        match file.read_exact(&mut magic) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        if &magic != JOURNAL_MAGIC {
            anyhow::bail!("invalid cache journal magic");
        }
        let mut version = [0_u8; 2];
        let mut len = [0_u8; 4];
        if file.read_exact(&mut version).is_err() || file.read_exact(&mut len).is_err() {
            break;
        }
        let version = u16::from_le_bytes(version);
        anyhow::ensure!(
            version == 1 || version == JOURNAL_VERSION,
            "unsupported cache journal version"
        );
        let len = u32::from_le_bytes(len) as usize;
        let mut payload = vec![0_u8; len];
        let mut checksum = [0_u8; 32];
        if file.read_exact(&mut payload).is_err() || file.read_exact(&mut checksum).is_err() {
            break;
        }
        anyhow::ensure!(
            blake3::hash(&payload).as_bytes() == &checksum,
            "cache journal checksum mismatch"
        );
        match bincode::deserialize::<CacheJournalEntry>(&payload)? {
            CacheJournalEntry::Delta(delta) => merge_cache_index(index, delta),
            CacheJournalEntry::Upserts(delta) => apply_cache_journal_delta(index, delta),
        }
        replayed += 1;
    }
    Ok(replayed)
}

fn journal_stats(root: &Path) -> anyhow::Result<(u64, u64)> {
    let path = journal_path(root);
    let Ok(metadata) = fs::metadata(&path) else {
        return Ok((0, 0));
    };
    let mut file = fs::File::open(path)?;
    let mut entries = 0_u64;
    loop {
        let mut header = [0_u8; 10];
        match file.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        if &header[..4] != JOURNAL_MAGIC {
            break;
        }
        let len = u32::from_le_bytes(header[6..10].try_into().expect("slice length")) as i64;
        if file.seek(SeekFrom::Current(len + 32)).is_err() {
            break;
        }
        entries += 1;
    }
    Ok((metadata.len(), entries))
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

fn apply_cache_journal_delta(base: &mut CacheIndex, delta: CacheJournalDeltaV2) {
    base.records.extend(delta.records);
    base.paths.extend(delta.paths);
    base.sealed_records.extend(delta.sealed_records);
    base.objects.extend(delta.objects);
    if let Some(generation) = delta.meta.generation {
        base.generation = base.generation.max(generation);
    }
    if delta.meta.sealed_salt.is_some() {
        base.sealed_salt = delta.meta.sealed_salt;
    }
    if delta.meta.sealed_kdf_params.is_some() {
        base.sealed_kdf_params = delta.meta.sealed_kdf_params;
    }
    if delta.meta.sealed_key_id.is_some() {
        base.sealed_key_id = delta.meta.sealed_key_id;
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
        if entry.file_name() == "journal.bin" {
            continue;
        }
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
        if entry.file_name() == "journal.bin" {
            continue;
        }
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

fn hex_to_key(value: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(value).ok()?;
    bytes.as_slice().try_into().ok()
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

    fn append_legacy_v1_journal(root: &Path, entry: &CacheJournalEntry) {
        fs::create_dir_all(root.join("index-v2")).unwrap();
        let payload = bincode::serialize(entry).unwrap();
        let checksum = blake3::hash(&payload);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(journal_path(root))
            .unwrap();
        file.write_all(JOURNAL_MAGIC).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&(payload.len() as u32).to_le_bytes())
            .unwrap();
        file.write_all(&payload).unwrap();
        file.write_all(checksum.as_bytes()).unwrap();
    }

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
        assert_eq!(cache_record.pack_file, None);
        assert_eq!(cache_record.pack_offset, None);
        let object_record: CacheObjectRecord = serde_json::from_str(
            r#"{"codec":"zstd","level":1,"policy_version":2,"source_hash":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"compressed_size":2,"kind":"chunk"}"#,
        )
        .unwrap();
        assert_eq!(object_record.pack_file, None);
        assert_eq!(object_record.pack_offset, None);
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
    fn journal_index_is_preferred_after_first_save() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.insert(&hash, 4, b"zstd").unwrap();
        cache.save().unwrap();

        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.index_format(), "index-v2");
        assert_eq!(reopened.shards_read(), 0);
        assert_eq!(reopened.maintenance_status().unwrap().journal_entries, 1);
        assert_eq!(
            reopened.maintenance_status().unwrap().cache_commit_mode,
            "none"
        );
        assert_eq!(reopened.get(&hash).unwrap().unwrap(), b"zstd");
    }

    #[test]
    fn record_level_journal_writes_no_shards_on_normal_save() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.insert(&hash, 4, b"zstd").unwrap();
        cache
            .upsert_path_record(PathCacheRecord {
                relative_path: "a.txt".to_string(),
                size: 4,
                mtime_ns: 1,
                permissions: 0o644,
                content_hash: hash,
                last_seen_unix_ns: 2,
                chunk_size: None,
                chunks: Vec::new(),
            })
            .unwrap();
        cache.save().unwrap();
        assert_eq!(cache.shards_written(), 0);
        assert_eq!(cache.last_commit_mode(), "journal-upserts");
        assert_eq!(cache.journal_upsert_counts(), (1, 1, 0, 0));
        assert!(
            fs::read_dir(temp.path().join("index-v2"))
                .unwrap()
                .all(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_str()
                    .is_some_and(|value| value == "journal.bin"))
        );
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get(&hash).unwrap().unwrap(), b"zstd");
        assert!(reopened.get_path_record("a.txt").is_some());
    }

    #[test]
    fn repeated_identical_upsert_does_not_append_journal_entry() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.insert(&hash, 4, b"zstd").unwrap();
        cache.save().unwrap();
        let before = journal_stats(temp.path()).unwrap();
        cache.insert(&hash, 4, b"zstd").unwrap();
        cache.save().unwrap();
        assert_eq!(journal_stats(temp.path()).unwrap(), before);
    }

    #[test]
    fn hot_payload_cache_serves_parameterized_objects_without_disk_read() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        let key = [7_u8; 32];
        let source = [8_u8; 32];
        cache
            .insert_parameterized(&key, &source, 4, 5, b"compressed")
            .unwrap();
        let record = cache.index.records.get(&hex::encode(key)).unwrap();
        let block_path = temp.path().join("blocks").join(&record.block_file);
        if block_path.exists() {
            fs::remove_file(block_path).unwrap();
        }
        if let Some(pack_file) = &record.pack_file {
            fs::remove_file(temp.path().join("object-packs").join(pack_file)).unwrap();
        }
        assert_eq!(cache.get(&key).unwrap(), Some(b"compressed".to_vec()));
        assert!(cache.has(&key));
    }

    #[test]
    fn pipeline_insert_skips_hot_copy_and_persists_object_pack() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        let key = [9_u8; 32];
        let source = [10_u8; 32];

        cache
            .insert_parameterized_for_pipeline(&key, &source, 4, 5, b"compressed")
            .unwrap();

        assert!(cache.hot_payloads.is_empty());
        assert_eq!(cache.hot_payload_bytes, 0);
        cache.save().unwrap();
        assert_eq!(cache.get(&key).unwrap(), Some(b"compressed".to_vec()));
    }

    #[test]
    fn parameterized_objects_roundtrip_from_object_pack() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        let key = [7_u8; 32];
        let source = [8_u8; 32];
        cache
            .insert_parameterized(&key, &source, 4, 5, b"compressed")
            .unwrap();
        cache.save().unwrap();

        let record = cache.index.records.get(&hex::encode(key)).unwrap();
        assert_eq!(record.block_file, format!("{}.zst", hex::encode(key)));
        assert!(record.pack_file.is_some());
        assert!(record.pack_offset.is_some());
        assert!(!temp.path().join("blocks").join(&record.block_file).exists());

        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.has(&key));
        assert_eq!(reopened.get(&key).unwrap(), Some(b"compressed".to_vec()));
    }

    #[test]
    fn batch_and_chunk_objects_roundtrip_from_object_pack() {
        let temp = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        let batch_key = [1_u8; 32];
        let chunk_key = [2_u8; 32];
        let batch_location = cache.insert_batch(&batch_key, b"batch-compressed").unwrap();
        cache.record_object_with_location(
            &batch_key,
            &batch_key,
            1,
            "batch",
            b"batch-compressed".len() as u64,
            Some(batch_location),
        );
        let chunk_location = cache.insert_chunk(&chunk_key, b"chunk-compressed").unwrap();
        cache.record_object_with_location(
            &chunk_key,
            &chunk_key,
            1,
            "chunk",
            b"chunk-compressed".len() as u64,
            Some(chunk_location),
        );
        cache.save().unwrap();

        assert!(
            !temp
                .path()
                .join("blocks")
                .join(format!("{}.batch.zst", hex::encode(batch_key)))
                .exists()
        );
        assert!(
            !temp
                .path()
                .join("blocks")
                .join(format!("{}.chunk.zst", hex::encode(chunk_key)))
                .exists()
        );

        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.has_batch(&batch_key));
        assert!(reopened.has_chunk(&chunk_key));
        assert_eq!(
            reopened.get_batch(&batch_key).unwrap(),
            Some(b"batch-compressed".to_vec())
        );
        assert_eq!(
            reopened.get_chunk(&chunk_key).unwrap(),
            Some(b"chunk-compressed".to_vec())
        );
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
            pack_file: None,
            pack_offset: None,
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

    #[test]
    fn journal_replay_restores_path_records_without_shards() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"file").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache
            .upsert_path_record(PathCacheRecord {
                relative_path: "a.txt".to_string(),
                size: 1,
                mtime_ns: 2,
                permissions: 0o644,
                content_hash: hash,
                last_seen_unix_ns: 3,
                chunk_size: None,
                chunks: Vec::new(),
            })
            .unwrap();
        cache.save().unwrap();
        assert!(journal_path(temp.path()).exists());
        assert!(
            fs::read_dir(temp.path().join("index-v2"))
                .unwrap()
                .all(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_str()
                        .is_some_and(|value| value == "journal.bin")
                })
        );
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.get_path_record("a.txt").is_some());
        assert_eq!(reopened.maintenance_status().unwrap().journal_entries, 1);
    }

    #[test]
    fn legacy_v1_journal_delta_still_replays() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"legacy").as_bytes();
        let mut index = CacheIndex::default();
        index.paths.insert(
            "legacy.txt".to_string(),
            PathCacheRecord {
                relative_path: "legacy.txt".to_string(),
                size: 1,
                mtime_ns: 2,
                permissions: 0o644,
                content_hash: hash,
                last_seen_unix_ns: 3,
                chunk_size: None,
                chunks: Vec::new(),
            },
        );
        append_legacy_v1_journal(temp.path(), &CacheJournalEntry::Delta(index));
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.get_path_record("legacy.txt").is_some());
    }

    #[test]
    fn upsert_journal_roundtrip_replays_records_paths_objects_and_sealed() {
        let temp = tempfile::tempdir().unwrap();
        let source = *blake3::hash(b"source").as_bytes();
        let key = *blake3::hash(b"object").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache
            .insert_parameterized(&key, &source, 6, 5, b"object")
            .unwrap();
        cache
            .upsert_path_record(PathCacheRecord {
                relative_path: "object.txt".to_string(),
                size: 6,
                mtime_ns: 7,
                permissions: 0o644,
                content_hash: source,
                last_seen_unix_ns: 8,
                chunk_size: None,
                chunks: Vec::new(),
            })
            .unwrap();
        cache.prepare_sealed_key(KdfParams::default(), &[9_u8; 32]);
        cache
            .insert_sealed(
                &key,
                SealedCacheRecord {
                    block_id: [1; 32],
                    nonce: [2; NONCE_LEN],
                    raw_size: 6,
                    compressed_size: 6,
                    encrypted_size: 6,
                    sealed_file: sealed_cache_file(&key),
                    codec: "zstd".to_string(),
                    kind: "single".to_string(),
                    pack_file: None,
                    pack_offset: None,
                },
                b"sealed",
            )
            .unwrap();
        cache.save().unwrap();
        assert_eq!(cache.last_commit_mode(), "journal-upserts");
        assert_eq!(cache.journal_upsert_counts(), (1, 1, 1, 1));

        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.index.records.contains_key(&hex::encode(key)));
        assert!(reopened.index.objects.contains_key(&hex::encode(key)));
        assert!(
            reopened
                .index
                .sealed_records
                .contains_key(&hex::encode(key))
        );
        assert!(reopened.get_path_record("object.txt").is_some());
    }

    #[test]
    fn save_without_l1_refresh_remains_reopenable() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"data").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache.insert(&hash, 4, b"zstd").unwrap();
        cache
            .save_with_options(CacheSaveOptions { refresh_l1: false })
            .unwrap();
        assert_eq!(cache.last_commit_mode(), "journal-upserts");
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get(&hash).unwrap().unwrap(), b"zstd");
    }

    #[test]
    fn journal_replay_ignores_truncated_tail() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"file").as_bytes();
        let mut index = CacheIndex::default();
        index.paths.insert(
            "a.txt".to_string(),
            PathCacheRecord {
                relative_path: "a.txt".to_string(),
                size: 1,
                mtime_ns: 2,
                permissions: 0o644,
                content_hash: hash,
                last_seen_unix_ns: 3,
                chunk_size: None,
                chunks: Vec::new(),
            },
        );
        append_journal_entry(temp.path(), &CacheJournalEntry::Delta(index)).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(journal_path(temp.path()))
            .unwrap();
        file.write_all(JOURNAL_MAGIC).unwrap();
        file.write_all(&JOURNAL_VERSION.to_le_bytes()).unwrap();
        file.write_all(&999_u32.to_le_bytes()).unwrap();
        drop(file);
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.get_path_record("a.txt").is_some());
    }

    #[test]
    fn journal_checksum_mismatch_fails_fast() {
        let temp = tempfile::tempdir().unwrap();
        append_journal_entry(
            temp.path(),
            &CacheJournalEntry::Delta(CacheIndex::default()),
        )
        .unwrap();
        let path = journal_path(temp.path());
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        fs::write(&path, bytes).unwrap();
        assert!(CacheStore::open(temp.path()).is_err());
    }

    #[test]
    fn compact_writes_shards_and_clears_journal() {
        let temp = tempfile::tempdir().unwrap();
        let hash = *blake3::hash(b"file").as_bytes();
        let mut cache = CacheStore::open(temp.path()).unwrap();
        cache
            .upsert_path_record(PathCacheRecord {
                relative_path: "a.txt".to_string(),
                size: 1,
                mtime_ns: 2,
                permissions: 0o644,
                content_hash: hash,
                last_seen_unix_ns: 3,
                chunk_size: None,
                chunks: Vec::new(),
            })
            .unwrap();
        cache.save().unwrap();
        cache.mark_all_shards_dirty();
        cache.write_v2_dirty_shards().unwrap();
        cache.truncate_journal().unwrap();
        let reopened = CacheStore::open(temp.path()).unwrap();
        assert!(reopened.get_path_record("a.txt").is_some());
        assert_eq!(journal_stats(temp.path()).unwrap().0, 0);
    }
}
