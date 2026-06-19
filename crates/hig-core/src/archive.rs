use crate::cache::{
    CacheStats, CacheStore, PathCacheRecord, PathChunkRecord, SealedCacheRecord, sealed_cache_file,
    sealed_nonce,
};
use crate::codec;
use crate::crypto::{self, KdfParams, NONCE_LEN, SALT_LEN};
use crate::scan::{ScanOptions, scan_dir, unix_ns};
use crate::writer::{ArchiveWriter, PayloadSource};
use crate::{
    ArchiveFormat, BatchOptions, BlockStats, ChunkOptions, Compression, EncryptionMode, IoOptions,
    PackOptions, PackReport, PackTimings, SpeedMode, UnpackOptions,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

const MAGIC_V1: &[u8; 8] = b"HIGV1\0\0\0";
const MAGIC_V2: &[u8; 8] = b"HIGV2\0\0\0";
const VERSION_V1: u32 = 1;
const VERSION_V2: u32 = 2;
const HEADER_FIXED_LEN: usize = 8 + 4 + 4 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN + 8;
const HEADER_FLAG_PASSWORD: u32 = SALT_LEN as u32;
const HEADER_FLAG_NONE: u32 = 0x8000_0000 | SALT_LEN as u32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub files: Vec<FileEntry>,
    pub blocks: Vec<BlockEntry>,
    pub root_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub relative_path: String,
    pub size: u64,
    pub mtime_secs: i64,
    pub permissions: u32,
    pub content_hash: [u8; 32],
    pub block_id: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockEntry {
    pub block_id: [u8; 32],
    pub content_hash: [u8; 32],
    pub original_size: u64,
    pub compressed_size: u64,
    pub encrypted_size: u64,
    pub archive_offset: u64,
    pub nonce: [u8; NONCE_LEN],
    pub codec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct V2Manifest {
    pub version: u32,
    pub files: Vec<V2FileEntry>,
    pub blocks: Vec<V2BlockEntry>,
    pub root_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct V2FileEntry {
    pub relative_path: String,
    pub size: u64,
    pub mtime_ns: i128,
    pub permissions: u32,
    pub content_hash: [u8; 32],
    pub block_id: [u8; 32],
    pub block_offset: u64,
    pub block_len: u64,
    #[serde(default)]
    pub layout: Option<FileLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct V2ManifestLegacy {
    pub version: u32,
    pub files: Vec<V2FileEntryLegacy>,
    pub blocks: Vec<V2BlockEntry>,
    pub root_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct V2FileEntryLegacy {
    pub relative_path: String,
    pub size: u64,
    pub mtime_ns: i128,
    pub permissions: u32,
    pub content_hash: [u8; 32],
    pub block_id: [u8; 32],
    pub block_offset: u64,
    pub block_len: u64,
}

impl V2ManifestLegacy {
    fn into_current(self) -> V2Manifest {
        V2Manifest {
            version: self.version,
            files: self.files.into_iter().map(Into::into).collect(),
            blocks: self.blocks,
            root_hash: self.root_hash,
        }
    }
}

impl From<V2FileEntryLegacy> for V2FileEntry {
    fn from(file: V2FileEntryLegacy) -> Self {
        Self {
            relative_path: file.relative_path,
            size: file.size,
            mtime_ns: file.mtime_ns,
            permissions: file.permissions,
            content_hash: file.content_hash,
            block_id: file.block_id,
            block_offset: file.block_offset,
            block_len: file.block_len,
            layout: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileLayout {
    InlineBlock {
        block_id: [u8; 32],
        offset: u64,
        len: u64,
    },
    Chunked {
        chunks: Vec<ChunkRef>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    pub chunk_hash: [u8; 32],
    pub block_id: [u8; 32],
    pub file_offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct V2BlockEntry {
    pub block_id: [u8; 32],
    pub raw_size: u64,
    pub compressed_size: u64,
    pub encrypted_size: u64,
    pub archive_offset: u64,
    pub nonce: [u8; NONCE_LEN],
    pub codec: String,
    pub kind: BlockKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlockKind {
    Batch,
    Single,
    Chunk,
}

#[derive(Debug, Clone)]
struct ArchiveHeader {
    version: u32,
    kdf: KdfParams,
    salt: [u8; SALT_LEN],
    manifest_nonce: [u8; NONCE_LEN],
    manifest_len: u64,
    encryption: EncryptionMode,
}

struct PreparedBlock {
    entry: BlockEntry,
    ciphertext: Vec<u8>,
}

struct PreparedV2Block {
    entry: V2BlockEntry,
    payload: PayloadSource,
}

struct SealedPayloadMeta {
    block_id: [u8; 32],
    nonce: [u8; NONCE_LEN],
    raw_size: u64,
    compressed_size: u64,
    kind: BlockKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SealedPayloadSource {
    Pack,
    File,
}

#[derive(Debug, Clone)]
enum PlannedBlock {
    Single {
        file_index: usize,
    },
    Batch {
        file_indices: Vec<usize>,
        raw_size: u64,
        batch_key: [u8; 32],
    },
    Chunk {
        file_index: usize,
        file_offset: u64,
        len: u64,
        chunk_hash: [u8; 32],
        from_metadata_cache: bool,
    },
}

#[derive(Default)]
struct WarmSummary {
    single_blocks: usize,
    batch_blocks: usize,
    chunk_blocks: usize,
    single_bytes: u64,
    batch_bytes: u64,
    chunk_bytes: u64,
    read_ms: u128,
    compression_ms: u128,
}

enum WarmKind {
    Single { hash: [u8; 32] },
    Batch { key: [u8; 32] },
    Chunk { hash: [u8; 32] },
}

struct WarmResult {
    kind: WarmKind,
    raw_size: u64,
    compressed: Vec<u8>,
    read_ms: u128,
    compression_ms: u128,
}

pub fn pack(options: PackOptions) -> anyhow::Result<PackReport> {
    if options.format == ArchiveFormat::HigV1 && options.encryption != EncryptionMode::Password {
        anyhow::bail!("HIGV1 only supports password encryption");
    }
    if options.encryption == EncryptionMode::Password && options.password.is_none() {
        anyhow::bail!("password encryption requires a password");
    }
    if options.encryption == EncryptionMode::None && options.password.is_some() {
        anyhow::bail!("--password cannot be used with encryption mode none");
    }
    match options.format {
        ArchiveFormat::HigV1 => pack_v1(options),
        ArchiveFormat::HigV2 => pack_v2(options),
    }
}

fn pack_v1(options: PackOptions) -> anyhow::Result<PackReport> {
    let started = Instant::now();
    if let Some(threads) = options.threads {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global();
    }

    let input_dir = options.input_dir.canonicalize()?;
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| input_dir.join(".hig-cache"));
    let mut cache = if options.use_cache {
        Some(CacheStore::open(&cache_dir)?)
    } else {
        None
    };
    let scan = scan_dir(
        &input_dir,
        &cache_dir,
        &options.output_file,
        cache.as_ref(),
        ScanOptions {
            trust_metadata: options.trust_metadata,
            chunk: options.chunk,
        },
    )?;
    let files = scan.files;
    let input_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let mut stats = CacheStats::default();

    let kdf = options.kdf_profile.params();
    let salt = crypto::random_bytes::<SALT_LEN>();
    let key = crypto::derive_key(required_password(&options.password)?, &salt, &kdf)?;
    let mut prepared = Vec::with_capacity(files.len());
    let mut file_entries = Vec::with_capacity(files.len());

    for file in &files {
        let compressed = if let Some(cache_store) = cache.as_mut() {
            if let Some(bytes) = cache_store.get(&file.content_hash)? {
                stats.hits += 1;
                stats.bytes_reused += file.size;
                bytes
            } else {
                stats.misses += 1;
                stats.bytes_compressed += file.size;
                let input = fs::read(&file.absolute_path)?;
                let bytes = codec::compress(options.compression, &input, options.level)?;
                cache_store.insert(&file.content_hash, file.size, &bytes)?;
                bytes
            }
        } else {
            stats.misses += 1;
            stats.bytes_compressed += file.size;
            let input = fs::read(&file.absolute_path)?;
            codec::compress(options.compression, &input, options.level)?
        };

        let nonce = crypto::random_bytes::<NONCE_LEN>();
        let ciphertext = crypto::encrypt(&key, &nonce, &compressed)?;
        let block_id = *blake3::hash(&compressed).as_bytes();
        file_entries.push(FileEntry {
            relative_path: file.relative_path.clone(),
            size: file.size,
            mtime_secs: file.mtime_secs,
            permissions: file.permissions,
            content_hash: file.content_hash,
            block_id,
        });
        prepared.push(PreparedBlock {
            entry: BlockEntry {
                block_id,
                content_hash: file.content_hash,
                original_size: file.size,
                compressed_size: compressed.len() as u64,
                encrypted_size: ciphertext.len() as u64,
                archive_offset: 0,
                nonce,
                codec: "zstd".to_string(),
            },
            ciphertext,
        });
        if let Some(cache_store) = cache.as_mut() {
            cache_store.upsert_path_record(PathCacheRecord {
                relative_path: file.relative_path.clone(),
                size: file.size,
                mtime_ns: file.mtime_ns,
                permissions: file.permissions,
                content_hash: file.content_hash,
                last_seen_unix_ns: unix_ns(SystemTime::now()),
                chunk_size: None,
                chunks: Vec::new(),
            })?;
        }
    }
    if let Some(cache_store) = cache.as_ref() {
        cache_store.save()?;
    }

    let mut manifest = Manifest {
        version: VERSION_V1,
        files: file_entries,
        blocks: prepared.iter().map(|block| block.entry.clone()).collect(),
        root_hash: root_hash(
            &files
                .iter()
                .map(|file| (&file.relative_path, file.content_hash))
                .collect::<Vec<_>>(),
        ),
    };

    let manifest_nonce = crypto::random_bytes::<NONCE_LEN>();
    let manifest_plain = bincode::serialize(&manifest)?;
    let manifest_cipher_len = crypto::encrypt(&key, &manifest_nonce, &manifest_plain)?.len() as u64;
    let mut offset = HEADER_FIXED_LEN as u64 + manifest_cipher_len;
    for (entry, prepared_block) in manifest.blocks.iter_mut().zip(prepared.iter_mut()) {
        entry.archive_offset = offset;
        prepared_block.entry.archive_offset = offset;
        offset += prepared_block.ciphertext.len() as u64;
    }
    let manifest_plain = bincode::serialize(&manifest)?;
    let manifest_ciphertext = crypto::encrypt(&key, &manifest_nonce, &manifest_plain)?;

    let write_started = Instant::now();
    let expected_len = archive_len(
        manifest_ciphertext.len() as u64,
        prepared.iter().map(|block| block.ciphertext.len() as u64),
    )?;
    let mut out = ArchiveWriter::create(&options.output_file, expected_len, IoOptions::default())?;
    write_header(
        &mut out,
        MAGIC_V1,
        &ArchiveHeader {
            version: VERSION_V1,
            kdf,
            salt,
            manifest_nonce,
            manifest_len: manifest_ciphertext.len() as u64,
            encryption: EncryptionMode::Password,
        },
    )?;
    out.write_all(&manifest_ciphertext)?;
    let payloads = prepared
        .into_iter()
        .map(|block| PayloadSource::Memory(block.ciphertext))
        .collect::<Vec<_>>();
    let payload_memory_bytes = payloads
        .iter()
        .filter_map(|payload| match payload {
            PayloadSource::Memory(bytes) => Some(bytes.len() as u64),
            PayloadSource::CachedFile { .. } | PayloadSource::CachedRange { .. } => None,
        })
        .sum();
    out.write_payloads(&payloads)?;
    let writer_report = out.finish()?;
    let write_ms = write_started.elapsed().as_millis();
    let archive_bytes = expected_len;

    Ok(PackReport {
        input_files: files.len(),
        input_bytes,
        archive_bytes,
        duration: started.elapsed(),
        timings: PackTimings {
            write_ms,
            payload_write_ms: writer_report.payload_write_ms,
            payload_read_ms: writer_report.payload_read_ms,
            writer_wait_ms: writer_report.writer_wait_ms,
            output_flush_ms: writer_report.flush_ms,
            output_rename_ms: writer_report.rename_ms,
            ..PackTimings::default()
        },
        cache: stats,
        scan: scan.stats,
        blocks: BlockStats {
            single_blocks: files.len(),
            ..BlockStats::default()
        },
        speed: options.speed,
        kdf_profile: options.kdf_profile,
        encryption_mode: EncryptionMode::Password,
        worker_count: worker_count(options.threads),
        writer_strategy: writer_report.strategy,
        archive_preallocated_bytes: writer_report.preallocated_bytes,
        cached_payload_open_count: writer_report.cached_payload_open_count,
        cached_range_open_count: writer_report.cached_range_open_count,
        cached_payload_read_bytes: writer_report.cached_payload_read_bytes,
        prefetched_bytes: writer_report.prefetched_bytes,
        direct_write_count: writer_report.direct_write_count,
        buffered_write_count: writer_report.buffered_write_count,
        preallocation_enabled: writer_report.preallocation_enabled,
        peak_pipeline_memory_bytes: writer_report
            .peak_pipeline_memory_bytes
            .max(payload_memory_bytes),
    })
}

pub fn unpack(options: UnpackOptions) -> anyhow::Result<()> {
    let mut archive = fs::File::open(&options.archive_file)?;
    let mut magic = [0_u8; 8];
    archive.read_exact(&mut magic)?;
    match &magic {
        MAGIC_V1 => unpack_v1(options, archive),
        MAGIC_V2 => unpack_v2(options, archive),
        _ => anyhow::bail!("not a hig archive"),
    }
}

fn pack_v2(options: PackOptions) -> anyhow::Result<PackReport> {
    let started = Instant::now();
    let fastest = options.speed == SpeedMode::Fastest;
    let trust_metadata = options.trust_metadata || fastest;
    let sealed_enabled = options.use_cache
        && options.sealed_cache
        && fastest
        && options.encryption == EncryptionMode::Password;
    if let Some(threads) = options.threads {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global();
    }

    let input_dir = options.input_dir.canonicalize()?;
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| input_dir.join(".hig-cache"));
    let mut cache = if options.use_cache {
        Some(CacheStore::open(&cache_dir)?)
    } else {
        None
    };
    let kdf = options.kdf_profile.params();
    let salt = if sealed_enabled {
        cache
            .as_mut()
            .map(CacheStore::sealed_salt_or_create)
            .unwrap_or_else(crypto::random_bytes::<SALT_LEN>)
    } else {
        crypto::random_bytes::<SALT_LEN>()
    };
    let kdf_task = match options.encryption {
        EncryptionMode::Password => {
            let password = required_password(&options.password)?.to_owned();
            let task_kdf = kdf.clone();
            Some(std::thread::spawn(move || {
                let started = Instant::now();
                let key = crypto::derive_key(&password, &salt, &task_kdf)?;
                Ok::<_, anyhow::Error>((key, started.elapsed().as_millis()))
            }))
        }
        EncryptionMode::None => None,
    };
    let scan_started = Instant::now();
    let scan = scan_dir(
        &input_dir,
        &cache_dir,
        &options.output_file,
        cache.as_ref(),
        ScanOptions {
            trust_metadata,
            chunk: options.chunk,
        },
    )?;
    let scan_ms = scan_started.elapsed().as_millis();
    let files = scan.files;
    let input_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let plan_started = Instant::now();
    let plans = plan_blocks(&files, options.batch, options.chunk)?;
    let plan_ms = plan_started.elapsed().as_millis();
    let mut cache_stats = CacheStats::default();
    let mut block_stats = BlockStats::default();

    let (key, kdf_ms) = match kdf_task {
        Some(task) => {
            let (key, elapsed) = task
                .join()
                .map_err(|_| anyhow::anyhow!("KDF worker panicked"))??;
            (Some(key), elapsed)
        }
        None => (None, 0),
    };
    if sealed_enabled && let Some(cache_store) = cache.as_mut() {
        cache_store.prepare_sealed_key(kdf.clone(), key.as_ref().expect("password mode key"));
    }
    let kdf_overlapped_ms = kdf_ms.min(scan_ms.saturating_add(plan_ms));
    let mut prepared = Vec::with_capacity(plans.len());
    let mut file_entries = Vec::with_capacity(files.len());
    let mut chunk_refs: std::collections::BTreeMap<usize, Vec<ChunkRef>> =
        std::collections::BTreeMap::new();
    let mut path_chunk_refs: std::collections::BTreeMap<usize, Vec<PathChunkRecord>> =
        std::collections::BTreeMap::new();

    let pack_blocks_started = Instant::now();
    let warm_summary = if let Some(cache_store) = cache.as_mut() {
        prewarm_compressed_cache(
            cache_store,
            &plans,
            &files,
            options.compression,
            options.level,
        )?
    } else {
        WarmSummary::default()
    };
    let defer_block_crypto = options.encryption == EncryptionMode::Password && !sealed_enabled;
    let mut crypto_ms = 0_u128;
    for plan in plans {
        match plan {
            PlannedBlock::Single { file_index } => {
                block_stats.single_blocks += 1;
                let file = &files[file_index];
                if sealed_enabled
                    && let Some((mut sealed_entry, payload, source)) =
                        try_sealed_payload(cache.as_ref(), &file.content_hash)?
                {
                    block_stats.sealed_block_hits += 1;
                    block_stats.sealed_bytes_reused += sealed_entry.encrypted_size;
                    block_stats.payload_source_cache_files += 1;
                    match source {
                        SealedPayloadSource::Pack => block_stats.cache_pack_hits += 1,
                        SealedPayloadSource::File => block_stats.cache_pack_fallbacks += 1,
                    }
                    sealed_entry.kind = BlockKind::Single;
                    file_entries.push(V2FileEntry {
                        relative_path: file.relative_path.clone(),
                        size: file.size,
                        mtime_ns: file.mtime_ns,
                        permissions: file.permissions,
                        content_hash: file.content_hash,
                        block_id: sealed_entry.block_id,
                        block_offset: 0,
                        block_len: file.size,
                        layout: Some(FileLayout::InlineBlock {
                            block_id: sealed_entry.block_id,
                            offset: 0,
                            len: file.size,
                        }),
                    });
                    prepared.push(PreparedV2Block {
                        entry: sealed_entry,
                        payload,
                    });
                    upsert_path_cache(cache.as_mut(), file, options.chunk.chunk_size, &[])?;
                    continue;
                }
                if sealed_enabled {
                    block_stats.sealed_block_misses += 1;
                    block_stats.cache_pack_misses += 1;
                }
                let compressed = if let Some(cache_store) = cache.as_mut() {
                    if let Some(bytes) = cache_store.get(&file.content_hash)? {
                        cache_stats.hits += 1;
                        cache_stats.bytes_reused += file.size;
                        if sealed_enabled {
                            block_stats.reencrypted_cache_hits += 1;
                        }
                        bytes
                    } else {
                        cache_stats.misses += 1;
                        cache_stats.bytes_compressed += file.size;
                        let input = fs::read(&file.absolute_path)?;
                        let bytes = codec::compress(options.compression, &input, options.level)?;
                        cache_store.insert(&file.content_hash, file.size, &bytes)?;
                        bytes
                    }
                } else {
                    cache_stats.misses += 1;
                    cache_stats.bytes_compressed += file.size;
                    let input = fs::read(&file.absolute_path)?;
                    codec::compress(options.compression, &input, options.level)?
                };
                let nonce = if sealed_enabled {
                    sealed_nonce(&file.content_hash)
                } else {
                    crypto::random_bytes::<NONCE_LEN>()
                };
                let ciphertext = if defer_block_crypto {
                    compressed.clone()
                } else {
                    protect_payload_timed(
                        options.encryption,
                        key.as_ref(),
                        &nonce,
                        &compressed,
                        &mut crypto_ms,
                    )?
                };
                let block_id = *blake3::hash(&compressed).as_bytes();
                if sealed_enabled {
                    write_sealed_payload(
                        cache.as_mut(),
                        &file.content_hash,
                        SealedPayloadMeta {
                            block_id,
                            nonce,
                            raw_size: file.size,
                            compressed_size: compressed.len() as u64,
                            kind: BlockKind::Single,
                        },
                        &ciphertext,
                    )?;
                }
                let encrypted_size = ciphertext.len() as u64;
                let payload = prepared_memory_payload(ciphertext, &mut block_stats);
                file_entries.push(V2FileEntry {
                    relative_path: file.relative_path.clone(),
                    size: file.size,
                    mtime_ns: file.mtime_ns,
                    permissions: file.permissions,
                    content_hash: file.content_hash,
                    block_id,
                    block_offset: 0,
                    block_len: file.size,
                    layout: Some(FileLayout::InlineBlock {
                        block_id,
                        offset: 0,
                        len: file.size,
                    }),
                });
                prepared.push(PreparedV2Block {
                    entry: V2BlockEntry {
                        block_id,
                        raw_size: file.size,
                        compressed_size: compressed.len() as u64,
                        encrypted_size,
                        archive_offset: 0,
                        nonce,
                        codec: "zstd".to_string(),
                        kind: BlockKind::Single,
                    },
                    payload,
                });
                upsert_path_cache(cache.as_mut(), file, options.chunk.chunk_size, &[])?;
            }
            PlannedBlock::Batch {
                file_indices,
                raw_size,
                batch_key,
            } => {
                block_stats.batch_blocks += 1;
                block_stats.batched_files += file_indices.len();
                if sealed_enabled
                    && let Some((mut sealed_entry, payload, source)) =
                        try_sealed_payload(cache.as_ref(), &batch_key)?
                {
                    block_stats.sealed_block_hits += 1;
                    block_stats.sealed_bytes_reused += sealed_entry.encrypted_size;
                    block_stats.payload_source_cache_files += 1;
                    match source {
                        SealedPayloadSource::Pack => block_stats.cache_pack_hits += 1,
                        SealedPayloadSource::File => block_stats.cache_pack_fallbacks += 1,
                    }
                    sealed_entry.kind = BlockKind::Batch;
                    let mut block_offset = 0_u64;
                    for index in &file_indices {
                        let file = &files[*index];
                        file_entries.push(V2FileEntry {
                            relative_path: file.relative_path.clone(),
                            size: file.size,
                            mtime_ns: file.mtime_ns,
                            permissions: file.permissions,
                            content_hash: file.content_hash,
                            block_id: sealed_entry.block_id,
                            block_offset,
                            block_len: file.size,
                            layout: Some(FileLayout::InlineBlock {
                                block_id: sealed_entry.block_id,
                                offset: block_offset,
                                len: file.size,
                            }),
                        });
                        block_offset += file.size;
                        upsert_path_cache(cache.as_mut(), file, options.chunk.chunk_size, &[])?;
                    }
                    prepared.push(PreparedV2Block {
                        entry: sealed_entry,
                        payload,
                    });
                    continue;
                }
                if sealed_enabled {
                    block_stats.sealed_block_misses += 1;
                    block_stats.cache_pack_misses += 1;
                }
                let compressed = if let Some(cache_store) = cache.as_mut() {
                    if let Some(bytes) = cache_store.get_batch(&batch_key)? {
                        block_stats.batch_cache_hits += 1;
                        cache_stats.bytes_reused += raw_size;
                        if sealed_enabled {
                            block_stats.reencrypted_cache_hits += 1;
                        }
                        bytes
                    } else {
                        block_stats.batch_cache_misses += 1;
                        cache_stats.bytes_compressed += raw_size;
                        let raw = build_batch_raw(&files, &file_indices)?;
                        let bytes = codec::compress(options.compression, &raw, options.level)?;
                        cache_store.insert_batch(&batch_key, &bytes)?;
                        bytes
                    }
                } else {
                    block_stats.batch_cache_misses += 1;
                    cache_stats.bytes_compressed += raw_size;
                    let raw = build_batch_raw(&files, &file_indices)?;
                    codec::compress(options.compression, &raw, options.level)?
                };
                let nonce = if sealed_enabled {
                    sealed_nonce(&batch_key)
                } else {
                    crypto::random_bytes::<NONCE_LEN>()
                };
                let ciphertext = if defer_block_crypto {
                    compressed.clone()
                } else {
                    protect_payload_timed(
                        options.encryption,
                        key.as_ref(),
                        &nonce,
                        &compressed,
                        &mut crypto_ms,
                    )?
                };
                let block_id = *blake3::hash(&compressed).as_bytes();
                if sealed_enabled {
                    write_sealed_payload(
                        cache.as_mut(),
                        &batch_key,
                        SealedPayloadMeta {
                            block_id,
                            nonce,
                            raw_size,
                            compressed_size: compressed.len() as u64,
                            kind: BlockKind::Batch,
                        },
                        &ciphertext,
                    )?;
                }
                let encrypted_size = ciphertext.len() as u64;
                let payload = prepared_memory_payload(ciphertext, &mut block_stats);
                let mut block_offset = 0_u64;
                for index in &file_indices {
                    let file = &files[*index];
                    file_entries.push(V2FileEntry {
                        relative_path: file.relative_path.clone(),
                        size: file.size,
                        mtime_ns: file.mtime_ns,
                        permissions: file.permissions,
                        content_hash: file.content_hash,
                        block_id,
                        block_offset,
                        block_len: file.size,
                        layout: Some(FileLayout::InlineBlock {
                            block_id,
                            offset: block_offset,
                            len: file.size,
                        }),
                    });
                    block_offset += file.size;
                    upsert_path_cache(cache.as_mut(), file, options.chunk.chunk_size, &[])?;
                }
                prepared.push(PreparedV2Block {
                    entry: V2BlockEntry {
                        block_id,
                        raw_size,
                        compressed_size: compressed.len() as u64,
                        encrypted_size,
                        archive_offset: 0,
                        nonce,
                        codec: "zstd".to_string(),
                        kind: BlockKind::Batch,
                    },
                    payload,
                });
            }
            PlannedBlock::Chunk {
                file_index,
                file_offset,
                len,
                chunk_hash,
                from_metadata_cache,
            } => {
                block_stats.chunk_blocks += 1;
                if from_metadata_cache {
                    block_stats.chunk_plan_cache_hits += 1;
                } else {
                    block_stats.chunk_plan_cache_misses += 1;
                }
                let file = &files[file_index];
                if sealed_enabled
                    && let Some((mut sealed_entry, payload, source)) =
                        try_sealed_payload(cache.as_ref(), &chunk_hash)?
                {
                    block_stats.sealed_block_hits += 1;
                    block_stats.sealed_bytes_reused += sealed_entry.encrypted_size;
                    block_stats.payload_source_cache_files += 1;
                    match source {
                        SealedPayloadSource::Pack => block_stats.cache_pack_hits += 1,
                        SealedPayloadSource::File => block_stats.cache_pack_fallbacks += 1,
                    }
                    sealed_entry.kind = BlockKind::Chunk;
                    chunk_refs.entry(file_index).or_default().push(ChunkRef {
                        chunk_hash,
                        block_id: sealed_entry.block_id,
                        file_offset,
                        len,
                    });
                    path_chunk_refs
                        .entry(file_index)
                        .or_default()
                        .push(PathChunkRecord {
                            chunk_hash,
                            file_offset,
                            len,
                        });
                    prepared.push(PreparedV2Block {
                        entry: sealed_entry,
                        payload,
                    });
                    continue;
                }
                if sealed_enabled {
                    block_stats.sealed_block_misses += 1;
                    block_stats.cache_pack_misses += 1;
                }
                let compressed = if let Some(cache_store) = cache.as_mut() {
                    if let Some(bytes) = cache_store.get_chunk(&chunk_hash)? {
                        cache_stats.hits += 1;
                        cache_stats.bytes_reused += len;
                        block_stats.chunk_cache_hits += 1;
                        block_stats.chunk_bytes_reused += len;
                        if sealed_enabled {
                            block_stats.reencrypted_cache_hits += 1;
                        }
                        bytes
                    } else {
                        cache_stats.misses += 1;
                        cache_stats.bytes_compressed += len;
                        block_stats.chunk_cache_misses += 1;
                        block_stats.chunk_bytes_compressed += len;
                        let raw = read_file_slice(file, file_offset, len)?;
                        let bytes = codec::compress(options.compression, &raw, options.level)?;
                        cache_store.insert_chunk(&chunk_hash, &bytes)?;
                        bytes
                    }
                } else {
                    cache_stats.misses += 1;
                    cache_stats.bytes_compressed += len;
                    block_stats.chunk_cache_misses += 1;
                    block_stats.chunk_bytes_compressed += len;
                    let raw = read_file_slice(file, file_offset, len)?;
                    codec::compress(options.compression, &raw, options.level)?
                };
                let nonce = if sealed_enabled {
                    sealed_nonce(&chunk_hash)
                } else {
                    crypto::random_bytes::<NONCE_LEN>()
                };
                let ciphertext = if defer_block_crypto {
                    compressed.clone()
                } else {
                    protect_payload_timed(
                        options.encryption,
                        key.as_ref(),
                        &nonce,
                        &compressed,
                        &mut crypto_ms,
                    )?
                };
                let block_id = *blake3::hash(&compressed).as_bytes();
                if sealed_enabled {
                    write_sealed_payload(
                        cache.as_mut(),
                        &chunk_hash,
                        SealedPayloadMeta {
                            block_id,
                            nonce,
                            raw_size: len,
                            compressed_size: compressed.len() as u64,
                            kind: BlockKind::Chunk,
                        },
                        &ciphertext,
                    )?;
                }
                let encrypted_size = ciphertext.len() as u64;
                let payload = prepared_memory_payload(ciphertext, &mut block_stats);
                chunk_refs.entry(file_index).or_default().push(ChunkRef {
                    chunk_hash,
                    block_id,
                    file_offset,
                    len,
                });
                path_chunk_refs
                    .entry(file_index)
                    .or_default()
                    .push(PathChunkRecord {
                        chunk_hash,
                        file_offset,
                        len,
                    });
                prepared.push(PreparedV2Block {
                    entry: V2BlockEntry {
                        block_id,
                        raw_size: len,
                        compressed_size: compressed.len() as u64,
                        encrypted_size,
                        archive_offset: 0,
                        nonce,
                        codec: "zstd".to_string(),
                        kind: BlockKind::Chunk,
                    },
                    payload,
                });
            }
        }
    }
    cache_stats.hits = cache_stats
        .hits
        .saturating_sub(warm_summary.single_blocks + warm_summary.chunk_blocks);
    cache_stats.misses += warm_summary.single_blocks + warm_summary.chunk_blocks;
    cache_stats.bytes_reused = cache_stats.bytes_reused.saturating_sub(
        warm_summary.single_bytes + warm_summary.batch_bytes + warm_summary.chunk_bytes,
    );
    cache_stats.bytes_compressed +=
        warm_summary.single_bytes + warm_summary.batch_bytes + warm_summary.chunk_bytes;
    block_stats.batch_cache_hits = block_stats
        .batch_cache_hits
        .saturating_sub(warm_summary.batch_blocks);
    block_stats.batch_cache_misses += warm_summary.batch_blocks;
    block_stats.chunk_cache_hits = block_stats
        .chunk_cache_hits
        .saturating_sub(warm_summary.chunk_blocks);
    block_stats.chunk_cache_misses += warm_summary.chunk_blocks;
    block_stats.chunk_bytes_reused = block_stats
        .chunk_bytes_reused
        .saturating_sub(warm_summary.chunk_bytes);
    block_stats.chunk_bytes_compressed += warm_summary.chunk_bytes;
    block_stats.reencrypted_cache_hits = block_stats.reencrypted_cache_hits.saturating_sub(
        warm_summary.single_blocks + warm_summary.batch_blocks + warm_summary.chunk_blocks,
    );
    if defer_block_crypto {
        let crypto_started = Instant::now();
        let encryption_key = key.as_ref().expect("password mode key");
        prepared
            .par_iter_mut()
            .try_for_each(|block| -> anyhow::Result<()> {
                let PayloadSource::Memory(plaintext) = &mut block.payload else {
                    return Ok(());
                };
                let ciphertext = crypto::encrypt(encryption_key, &block.entry.nonce, plaintext)?;
                block.entry.encrypted_size = ciphertext.len() as u64;
                *plaintext = ciphertext;
                Ok(())
            })?;
        crypto_ms += crypto_started.elapsed().as_millis();
        block_stats.payload_source_memory_bytes = prepared
            .iter()
            .filter_map(|block| match &block.payload {
                PayloadSource::Memory(bytes) => Some(bytes.len() as u64),
                PayloadSource::CachedFile { .. } | PayloadSource::CachedRange { .. } => None,
            })
            .sum();
    }
    let pack_blocks_ms = pack_blocks_started.elapsed().as_millis();
    for (file_index, chunks) in chunk_refs {
        block_stats.chunked_files += 1;
        let file = &files[file_index];
        let path_chunks = path_chunk_refs.remove(&file_index).unwrap_or_default();
        let block_id = chunks
            .first()
            .map(|chunk| chunk.block_id)
            .unwrap_or([0; 32]);
        file_entries.push(V2FileEntry {
            relative_path: file.relative_path.clone(),
            size: file.size,
            mtime_ns: file.mtime_ns,
            permissions: file.permissions,
            content_hash: file.content_hash,
            block_id,
            block_offset: 0,
            block_len: file.size,
            layout: Some(FileLayout::Chunked { chunks }),
        });
        upsert_path_cache(cache.as_mut(), file, options.chunk.chunk_size, &path_chunks)?;
    }
    if let Some(cache_store) = cache.as_ref() {
        cache_store.save()?;
    }

    let manifest_started = Instant::now();
    file_entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let manifest = V2Manifest {
        version: VERSION_V2,
        files: file_entries,
        blocks: prepared.iter().map(|block| block.entry.clone()).collect(),
        root_hash: root_hash(
            &files
                .iter()
                .map(|file| (&file.relative_path, file.content_hash))
                .collect::<Vec<_>>(),
        ),
    };

    let manifest_nonce = crypto::random_bytes::<NONCE_LEN>();
    let manifest_plain = bincode::serialize(&manifest)?;
    let manifest_compressed = codec::compress(Compression::Zstd, &manifest_plain, 1)?;
    let manifest_ciphertext = protect_payload_timed(
        options.encryption,
        key.as_ref(),
        &manifest_nonce,
        &manifest_compressed,
        &mut crypto_ms,
    )?;
    let manifest_ms = manifest_started.elapsed().as_millis();

    let write_started = Instant::now();
    let expected_len = archive_len(
        manifest_ciphertext.len() as u64,
        prepared.iter().map(|block| block.entry.encrypted_size),
    )?;
    let mut out = ArchiveWriter::create(&options.output_file, expected_len, IoOptions::default())?;
    write_header(
        &mut out,
        MAGIC_V2,
        &ArchiveHeader {
            version: VERSION_V2,
            kdf,
            salt,
            manifest_nonce,
            manifest_len: manifest_ciphertext.len() as u64,
            encryption: options.encryption,
        },
    )?;
    out.write_all(&manifest_ciphertext)?;
    let payloads = prepared
        .into_iter()
        .map(|block| block.payload)
        .collect::<Vec<_>>();
    out.write_payloads(&payloads)?;
    let writer_report = out.finish()?;
    let archive_bytes = expected_len;
    let write_ms = write_started.elapsed().as_millis();
    let peak_pipeline_memory_bytes = writer_report
        .peak_pipeline_memory_bytes
        .max(block_stats.payload_source_memory_bytes);

    Ok(PackReport {
        input_files: files.len(),
        input_bytes,
        archive_bytes,
        duration: started.elapsed(),
        timings: PackTimings {
            scan_ms,
            plan_ms,
            kdf_ms,
            pack_blocks_ms,
            manifest_ms,
            write_ms,
            kdf_overlapped_ms,
            crypto_ms,
            compression_ms: warm_summary.compression_ms,
            read_ms: warm_summary.read_ms,
            payload_write_ms: writer_report.payload_write_ms,
            payload_read_ms: writer_report.payload_read_ms,
            writer_wait_ms: writer_report.writer_wait_ms,
            output_flush_ms: writer_report.flush_ms,
            output_rename_ms: writer_report.rename_ms,
        },
        cache: cache_stats,
        scan: scan.stats,
        blocks: block_stats,
        speed: options.speed,
        kdf_profile: options.kdf_profile,
        encryption_mode: options.encryption,
        worker_count: worker_count(options.threads),
        writer_strategy: writer_report.strategy,
        archive_preallocated_bytes: writer_report.preallocated_bytes,
        cached_payload_open_count: writer_report.cached_payload_open_count,
        cached_range_open_count: writer_report.cached_range_open_count,
        cached_payload_read_bytes: writer_report.cached_payload_read_bytes,
        prefetched_bytes: writer_report.prefetched_bytes,
        direct_write_count: writer_report.direct_write_count,
        buffered_write_count: writer_report.buffered_write_count,
        preallocation_enabled: writer_report.preallocation_enabled,
        peak_pipeline_memory_bytes,
    })
}

fn unpack_v1(options: UnpackOptions, mut archive: fs::File) -> anyhow::Result<()> {
    let header = read_header_after_magic(&mut archive, VERSION_V1)?;
    let mut manifest_ciphertext = vec![0_u8; header.manifest_len as usize];
    archive.read_exact(&mut manifest_ciphertext)?;
    let key = crypto::derive_key(
        required_password(&options.password)?,
        &header.salt,
        &header.kdf,
    )?;
    let manifest_plain = crypto::decrypt(&key, &header.manifest_nonce, &manifest_ciphertext)?;
    let manifest: Manifest = bincode::deserialize(&manifest_plain)?;

    if options.output_dir.exists() && !options.output_dir.is_dir() {
        anyhow::bail!(
            "output path is not a directory: {}",
            options.output_dir.display()
        );
    }

    let mut verified_files = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        let target = checked_target(&options.output_dir, &file.relative_path)?;
        if target.exists() && !options.overwrite {
            anyhow::bail!("refusing to overwrite existing file: {}", target.display());
        }
        let block = manifest
            .blocks
            .iter()
            .find(|block| block.block_id == file.block_id)
            .ok_or_else(|| anyhow::anyhow!("missing block for {}", file.relative_path))?;
        let mut ciphertext = vec![0_u8; block.encrypted_size as usize];
        use std::io::{Seek, SeekFrom};
        archive.seek(SeekFrom::Start(block.archive_offset))?;
        archive.read_exact(&mut ciphertext)?;
        let compressed = crypto::decrypt(&key, &block.nonce, &ciphertext)?;
        if blake3::hash(&compressed).as_bytes() != &block.block_id {
            anyhow::bail!("block hash mismatch for {}", file.relative_path);
        }
        let content = codec::decompress(Compression::Zstd, &compressed, file.size)?;
        if blake3::hash(&content).as_bytes() != &file.content_hash {
            anyhow::bail!("file hash mismatch for {}", file.relative_path);
        }
        verified_files.push((target, content, file.permissions));
    }

    fs::create_dir_all(&options.output_dir)?;
    for (target, content, permissions) in verified_files {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, content)?;
        set_permissions(&target, permissions)?;
    }
    Ok(())
}

fn unpack_v2(options: UnpackOptions, mut archive: fs::File) -> anyhow::Result<()> {
    let header = read_header_after_magic(&mut archive, VERSION_V2)?;
    let mut manifest_ciphertext = vec![0_u8; header.manifest_len as usize];
    archive.read_exact(&mut manifest_ciphertext)?;
    let key = match header.encryption {
        EncryptionMode::Password => Some(crypto::derive_key(
            required_password(&options.password)?,
            &header.salt,
            &header.kdf,
        )?),
        EncryptionMode::None => None,
    };
    let manifest_compressed = unprotect_payload(
        header.encryption,
        key.as_ref(),
        &header.manifest_nonce,
        &manifest_ciphertext,
    )?;
    let manifest_plain = codec::decompress_unknown(Compression::Zstd, &manifest_compressed)?;
    let manifest = decode_v2_manifest(&manifest_plain)?;
    let computed_root_hash = root_hash(
        &manifest
            .files
            .iter()
            .map(|file| (&file.relative_path, file.content_hash))
            .collect::<Vec<_>>(),
    );
    if computed_root_hash != manifest.root_hash {
        anyhow::bail!("manifest root hash mismatch");
    }

    if options.output_dir.exists() && !options.output_dir.is_dir() {
        anyhow::bail!(
            "output path is not a directory: {}",
            options.output_dir.display()
        );
    }

    let mut verified_files = Vec::with_capacity(manifest.files.len());
    let mut next_block_offset = HEADER_FIXED_LEN as u64 + header.manifest_len;
    let mut raw_blocks = std::collections::BTreeMap::new();
    for block in &manifest.blocks {
        let mut ciphertext = vec![0_u8; block.encrypted_size as usize];
        archive.seek(SeekFrom::Start(next_block_offset))?;
        archive.read_exact(&mut ciphertext)?;
        next_block_offset += block.encrypted_size;
        let compressed =
            unprotect_payload(header.encryption, key.as_ref(), &block.nonce, &ciphertext)?;
        if blake3::hash(&compressed).as_bytes() != &block.block_id {
            anyhow::bail!("block hash mismatch");
        }
        let raw = codec::decompress(Compression::Zstd, &compressed, block.raw_size)?;
        raw_blocks.insert(block.block_id, raw);
    }

    for file in &manifest.files {
        let target = checked_target(&options.output_dir, &file.relative_path)?;
        if target.exists() && !options.overwrite {
            anyhow::bail!("refusing to overwrite existing file: {}", target.display());
        }
        let content = match file.layout.as_ref() {
            Some(FileLayout::InlineBlock {
                block_id,
                offset,
                len,
            }) => {
                let raw = raw_blocks
                    .get(block_id)
                    .ok_or_else(|| anyhow::anyhow!("missing block for {}", file.relative_path))?;
                slice_block(raw, *offset, *len, &file.relative_path)?.to_vec()
            }
            Some(FileLayout::Chunked { chunks }) => {
                let mut content = vec![0_u8; file.size as usize];
                for chunk in chunks {
                    let raw = raw_blocks.get(&chunk.block_id).ok_or_else(|| {
                        anyhow::anyhow!("missing chunk block for {}", file.relative_path)
                    })?;
                    if raw.len() != chunk.len as usize {
                        anyhow::bail!("chunk length mismatch for {}", file.relative_path);
                    }
                    if blake3::hash(raw).as_bytes() != &chunk.chunk_hash {
                        anyhow::bail!("chunk hash mismatch for {}", file.relative_path);
                    }
                    let start = chunk.file_offset as usize;
                    let end = start + chunk.len as usize;
                    if end > content.len() {
                        anyhow::bail!("chunk exceeds file bounds for {}", file.relative_path);
                    }
                    content[start..end].copy_from_slice(raw);
                }
                content
            }
            None => {
                let raw = raw_blocks
                    .get(&file.block_id)
                    .ok_or_else(|| anyhow::anyhow!("missing block for {}", file.relative_path))?;
                slice_block(raw, file.block_offset, file.block_len, &file.relative_path)?.to_vec()
            }
        };
        if blake3::hash(&content).as_bytes() != &file.content_hash {
            anyhow::bail!("file hash mismatch for {}", file.relative_path);
        }
        verified_files.push((target, content, file.permissions));
    }

    fs::create_dir_all(&options.output_dir)?;
    for (target, content, permissions) in verified_files {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, content)?;
        set_permissions(&target, permissions)?;
    }
    Ok(())
}

fn plan_blocks(
    files: &[crate::ScannedFile],
    batch_options: BatchOptions,
    chunk_options: ChunkOptions,
) -> anyhow::Result<Vec<PlannedBlock>> {
    if chunk_options.enabled && chunk_options.chunk_size == 0 {
        anyhow::bail!("chunk size must be greater than zero");
    }
    let mut plans = Vec::new();
    let mut current = Vec::new();
    let mut current_size = 0_u64;
    for (index, file) in files.iter().enumerate() {
        if chunk_options.enabled && file.size > 0 && file.size >= chunk_options.chunk_file_threshold
        {
            flush_batch(files, &mut plans, &mut current, &mut current_size);
            if let Some(chunks) = file.cached_chunks.as_ref() {
                append_cached_chunk_plans(&mut plans, index, chunks);
            } else {
                append_chunk_plans(files, &mut plans, index, chunk_options.chunk_size)?;
            }
            continue;
        }

        if !batch_options.enabled || file.size > batch_options.small_file_threshold {
            flush_batch(files, &mut plans, &mut current, &mut current_size);
            plans.push(PlannedBlock::Single { file_index: index });
            continue;
        }

        if !current.is_empty() && current_size + file.size > batch_options.max_batch_raw_bytes {
            flush_batch(files, &mut plans, &mut current, &mut current_size);
        }
        current.push(index);
        current_size += file.size;
    }
    flush_batch(files, &mut plans, &mut current, &mut current_size);
    Ok(plans)
}

fn flush_batch(
    files: &[crate::ScannedFile],
    plans: &mut Vec<PlannedBlock>,
    current: &mut Vec<usize>,
    current_size: &mut u64,
) {
    if current.is_empty() {
        return;
    }
    let indices = std::mem::take(current);
    let raw_size = *current_size;
    *current_size = 0;
    let batch_key = batch_key(files, &indices);
    plans.push(PlannedBlock::Batch {
        file_indices: indices,
        raw_size,
        batch_key,
    });
}

fn append_chunk_plans(
    files: &[crate::ScannedFile],
    plans: &mut Vec<PlannedBlock>,
    file_index: usize,
    chunk_size: u64,
) -> anyhow::Result<()> {
    let file = &files[file_index];
    let mut input = fs::File::open(&file.absolute_path)?;
    let mut offset = 0_u64;
    while offset < file.size {
        let len = (file.size - offset).min(chunk_size);
        let mut buffer = vec![0_u8; len as usize];
        input.read_exact(&mut buffer)?;
        let chunk_hash = *blake3::hash(&buffer).as_bytes();
        plans.push(PlannedBlock::Chunk {
            file_index,
            file_offset: offset,
            len,
            chunk_hash,
            from_metadata_cache: false,
        });
        offset += len;
    }
    Ok(())
}

fn append_cached_chunk_plans(
    plans: &mut Vec<PlannedBlock>,
    file_index: usize,
    chunks: &[PathChunkRecord],
) {
    for chunk in chunks {
        plans.push(PlannedBlock::Chunk {
            file_index,
            file_offset: chunk.file_offset,
            len: chunk.len,
            chunk_hash: chunk.chunk_hash,
            from_metadata_cache: true,
        });
    }
}

fn batch_key(files: &[crate::ScannedFile], indices: &[usize]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for index in indices {
        let file = &files[*index];
        hasher.update(file.relative_path.as_bytes());
        hasher.update(&file.content_hash);
        hasher.update(&file.size.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

fn build_batch_raw(files: &[crate::ScannedFile], indices: &[usize]) -> anyhow::Result<Vec<u8>> {
    let raw_size = indices.iter().map(|index| files[*index].size).sum::<u64>();
    let mut raw = Vec::with_capacity(raw_size as usize);
    for index in indices {
        raw.extend(fs::read(&files[*index].absolute_path)?);
    }
    Ok(raw)
}

fn prewarm_compressed_cache(
    cache: &mut CacheStore,
    plans: &[PlannedBlock],
    files: &[crate::ScannedFile],
    compression: Compression,
    level: i32,
) -> anyhow::Result<WarmSummary> {
    let missing = plans
        .iter()
        .filter(|plan| match plan {
            PlannedBlock::Single { file_index } => !cache.has(&files[*file_index].content_hash),
            PlannedBlock::Batch { batch_key, .. } => !cache.has_batch(batch_key),
            PlannedBlock::Chunk { chunk_hash, .. } => !cache.has_chunk(chunk_hash),
        })
        .cloned()
        .collect::<Vec<_>>();

    let results = missing
        .par_iter()
        .map(|plan| -> anyhow::Result<WarmResult> {
            let read_started = Instant::now();
            let (kind, raw_size, raw) = match plan {
                PlannedBlock::Single { file_index } => {
                    let file = &files[*file_index];
                    (
                        WarmKind::Single {
                            hash: file.content_hash,
                        },
                        file.size,
                        fs::read(&file.absolute_path)?,
                    )
                }
                PlannedBlock::Batch {
                    file_indices,
                    raw_size,
                    batch_key,
                } => (
                    WarmKind::Batch { key: *batch_key },
                    *raw_size,
                    build_batch_raw(files, file_indices)?,
                ),
                PlannedBlock::Chunk {
                    file_index,
                    file_offset,
                    len,
                    chunk_hash,
                    ..
                } => (
                    WarmKind::Chunk { hash: *chunk_hash },
                    *len,
                    read_file_slice(&files[*file_index], *file_offset, *len)?,
                ),
            };
            let read_ms = read_started.elapsed().as_millis();
            let compression_started = Instant::now();
            let compressed = codec::compress(compression, &raw, level)?;
            Ok(WarmResult {
                kind,
                raw_size,
                compressed,
                read_ms,
                compression_ms: compression_started.elapsed().as_millis(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut summary = WarmSummary::default();
    for result in results {
        summary.read_ms += result.read_ms;
        summary.compression_ms += result.compression_ms;
        match result.kind {
            WarmKind::Single { hash } => {
                cache.insert(&hash, result.raw_size, &result.compressed)?;
                summary.single_blocks += 1;
                summary.single_bytes += result.raw_size;
            }
            WarmKind::Batch { key } => {
                cache.insert_batch(&key, &result.compressed)?;
                summary.batch_blocks += 1;
                summary.batch_bytes += result.raw_size;
            }
            WarmKind::Chunk { hash } => {
                cache.insert_chunk(&hash, &result.compressed)?;
                summary.chunk_blocks += 1;
                summary.chunk_bytes += result.raw_size;
            }
        }
    }
    Ok(summary)
}

fn try_sealed_payload(
    cache: Option<&CacheStore>,
    key: &[u8; 32],
) -> anyhow::Result<Option<(V2BlockEntry, PayloadSource, SealedPayloadSource)>> {
    let Some(cache_store) = cache else {
        return Ok(None);
    };
    let Some(record) = cache_store.get_sealed_record(key) else {
        return Ok(None);
    };
    let pack_payload = match (
        cache_store.sealed_pack_path(record),
        record.pack_offset,
        record.pack_file.as_ref(),
    ) {
        (Some(path), Some(offset), Some(_)) if path.exists() => {
            let end = offset
                .checked_add(record.encrypted_size)
                .ok_or_else(|| anyhow::anyhow!("sealed pack range overflow"))?;
            if fs::metadata(&path)?.len() >= end {
                Some(PayloadSource::CachedRange {
                    path,
                    offset,
                    len: record.encrypted_size,
                })
            } else {
                None
            }
        }
        _ => None,
    };
    let path = cache_store.sealed_block_path(record);
    let (payload, source) = if let Some(payload) = pack_payload {
        (payload, SealedPayloadSource::Pack)
    } else if path.exists() {
        (
            PayloadSource::CachedFile {
                path,
                len: record.encrypted_size,
            },
            SealedPayloadSource::File,
        )
    } else {
        return Ok(None);
    };
    Ok(Some((
        V2BlockEntry {
            block_id: record.block_id,
            raw_size: record.raw_size,
            compressed_size: record.compressed_size,
            encrypted_size: record.encrypted_size,
            archive_offset: 0,
            nonce: record.nonce,
            codec: record.codec.clone(),
            kind: sealed_kind(&record.kind)?,
        },
        payload,
        source,
    )))
}

fn prepared_memory_payload(ciphertext: Vec<u8>, stats: &mut BlockStats) -> PayloadSource {
    stats.payload_source_memory_bytes += ciphertext.len() as u64;
    PayloadSource::Memory(ciphertext)
}

fn write_sealed_payload(
    cache: Option<&mut CacheStore>,
    key: &[u8; 32],
    meta: SealedPayloadMeta,
    ciphertext: &[u8],
) -> anyhow::Result<()> {
    if let Some(cache_store) = cache {
        cache_store.insert_sealed(
            key,
            SealedCacheRecord {
                block_id: meta.block_id,
                nonce: meta.nonce,
                raw_size: meta.raw_size,
                compressed_size: meta.compressed_size,
                encrypted_size: ciphertext.len() as u64,
                sealed_file: sealed_cache_file(key),
                codec: "zstd".to_string(),
                kind: sealed_kind_name(meta.kind).to_string(),
                pack_file: None,
                pack_offset: None,
            },
            ciphertext,
        )?;
    }
    Ok(())
}

fn sealed_kind(name: &str) -> anyhow::Result<BlockKind> {
    match name {
        "batch" => Ok(BlockKind::Batch),
        "single" => Ok(BlockKind::Single),
        "chunk" => Ok(BlockKind::Chunk),
        other => anyhow::bail!("unsupported sealed block kind: {other}"),
    }
}

fn sealed_kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Batch => "batch",
        BlockKind::Single => "single",
        BlockKind::Chunk => "chunk",
    }
}

fn protect_payload(
    mode: EncryptionMode,
    key: Option<&[u8; crypto::KEY_LEN]>,
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    match mode {
        EncryptionMode::Password => crypto::encrypt(
            key.ok_or_else(|| anyhow::anyhow!("missing encryption key"))?,
            nonce,
            plaintext,
        ),
        EncryptionMode::None => Ok(plaintext.to_vec()),
    }
}

fn protect_payload_timed(
    mode: EncryptionMode,
    key: Option<&[u8; crypto::KEY_LEN]>,
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
    elapsed_ms: &mut u128,
) -> anyhow::Result<Vec<u8>> {
    let started = Instant::now();
    let result = protect_payload(mode, key, nonce, plaintext);
    *elapsed_ms += started.elapsed().as_millis();
    result
}

fn unprotect_payload(
    mode: EncryptionMode,
    key: Option<&[u8; crypto::KEY_LEN]>,
    nonce: &[u8; NONCE_LEN],
    payload: &[u8],
) -> anyhow::Result<Vec<u8>> {
    match mode {
        EncryptionMode::Password => crypto::decrypt(
            key.ok_or_else(|| anyhow::anyhow!("missing decryption key"))?,
            nonce,
            payload,
        ),
        EncryptionMode::None => Ok(payload.to_vec()),
    }
}

fn required_password(password: &Option<String>) -> anyhow::Result<&str> {
    password
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("password is required for this archive"))
}

fn worker_count(requested: Option<usize>) -> usize {
    requested.unwrap_or_else(rayon::current_num_threads).max(1)
}

fn read_file_slice(file: &crate::ScannedFile, offset: u64, len: u64) -> anyhow::Result<Vec<u8>> {
    let mut input = fs::File::open(&file.absolute_path)?;
    input.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0_u8; len as usize];
    input.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn slice_block<'a>(
    raw: &'a [u8],
    offset: u64,
    len: u64,
    relative_path: &str,
) -> anyhow::Result<&'a [u8]> {
    let start = offset as usize;
    let end = start + len as usize;
    if end > raw.len() {
        anyhow::bail!("file slice exceeds block bounds for {relative_path}");
    }
    Ok(&raw[start..end])
}

fn decode_v2_manifest(bytes: &[u8]) -> anyhow::Result<V2Manifest> {
    match bincode::deserialize(bytes) {
        Ok(manifest) => Ok(manifest),
        Err(_) => {
            let legacy: V2ManifestLegacy = bincode::deserialize(bytes)?;
            Ok(legacy.into_current())
        }
    }
}

fn upsert_path_cache(
    cache: Option<&mut CacheStore>,
    file: &crate::ScannedFile,
    chunk_size: u64,
    chunks: &[PathChunkRecord],
) -> anyhow::Result<()> {
    if let Some(cache_store) = cache {
        cache_store.upsert_path_record(PathCacheRecord {
            relative_path: file.relative_path.clone(),
            size: file.size,
            mtime_ns: file.mtime_ns,
            permissions: file.permissions,
            content_hash: file.content_hash,
            last_seen_unix_ns: unix_ns(SystemTime::now()),
            chunk_size: if chunks.is_empty() {
                None
            } else {
                Some(chunk_size)
            },
            chunks: chunks.to_vec(),
        })?;
    }
    Ok(())
}

fn root_hash(files: &[(&String, [u8; 32])]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in files {
        hasher.update(path.as_bytes());
        hasher.update(hash);
    }
    *hasher.finalize().as_bytes()
}

fn archive_len(
    manifest_len: u64,
    payload_lengths: impl IntoIterator<Item = u64>,
) -> anyhow::Result<u64> {
    payload_lengths.into_iter().try_fold(
        (HEADER_FIXED_LEN as u64)
            .checked_add(manifest_len)
            .ok_or_else(|| anyhow::anyhow!("archive length overflow"))?,
        |total, len| {
            total
                .checked_add(len)
                .ok_or_else(|| anyhow::anyhow!("archive length overflow"))
        },
    )
}

fn write_header(
    mut writer: impl Write,
    magic: &[u8; 8],
    header: &ArchiveHeader,
) -> anyhow::Result<()> {
    writer.write_all(magic)?;
    writer.write_all(&header.version.to_le_bytes())?;
    writer.write_all(&header.kdf.memory_cost_kib.to_le_bytes())?;
    writer.write_all(&header.kdf.time_cost.to_le_bytes())?;
    writer.write_all(&header.kdf.parallelism.to_le_bytes())?;
    let flags = match header.encryption {
        EncryptionMode::Password => HEADER_FLAG_PASSWORD,
        EncryptionMode::None => HEADER_FLAG_NONE,
    };
    writer.write_all(&flags.to_le_bytes())?;
    writer.write_all(&header.salt)?;
    writer.write_all(&header.manifest_nonce)?;
    writer.write_all(&header.manifest_len.to_le_bytes())?;
    Ok(())
}

fn read_header_after_magic(
    mut reader: impl Read,
    expected_version: u32,
) -> anyhow::Result<ArchiveHeader> {
    let version = read_u32(&mut reader)?;
    if version != expected_version {
        anyhow::bail!("unsupported hig archive version: {version}");
    }
    let kdf = KdfParams {
        memory_cost_kib: read_u32(&mut reader)?,
        time_cost: read_u32(&mut reader)?,
        parallelism: read_u32(&mut reader)?,
    };
    let flags = read_u32(&mut reader)?;
    let encryption = match flags {
        HEADER_FLAG_PASSWORD => EncryptionMode::Password,
        HEADER_FLAG_NONE if expected_version == VERSION_V2 => EncryptionMode::None,
        other => anyhow::bail!("unsupported archive header flags: {other:#x}"),
    };
    let mut salt = [0_u8; SALT_LEN];
    reader.read_exact(&mut salt)?;
    let mut manifest_nonce = [0_u8; NONCE_LEN];
    reader.read_exact(&mut manifest_nonce)?;
    let manifest_len = read_u64(&mut reader)?;
    Ok(ArchiveHeader {
        version,
        kdf,
        salt,
        manifest_nonce,
        manifest_len,
        encryption,
    })
}

fn read_u32(mut reader: impl Read) -> anyhow::Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(mut reader: impl Read) -> anyhow::Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn checked_target(root: &Path, relative_path: &str) -> anyhow::Result<PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("unsafe archive path: {relative_path}");
    }
    Ok(root.join(relative))
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn manifest_roundtrip() {
        let manifest = Manifest {
            version: VERSION_V1,
            files: vec![FileEntry {
                relative_path: "a.txt".to_string(),
                size: 3,
                mtime_secs: 1,
                permissions: 0o644,
                content_hash: [1; 32],
                block_id: [2; 32],
            }],
            blocks: vec![BlockEntry {
                block_id: [2; 32],
                content_hash: [1; 32],
                original_size: 3,
                compressed_size: 4,
                encrypted_size: 20,
                archive_offset: 100,
                nonce: [3; NONCE_LEN],
                codec: "zstd".to_string(),
            }],
            root_hash: [4; 32],
        };
        let bytes = bincode::serialize(&manifest).unwrap();
        let decoded: Manifest = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn v2_manifest_roundtrip() {
        let manifest = V2Manifest {
            version: VERSION_V2,
            files: vec![V2FileEntry {
                relative_path: "a.txt".to_string(),
                size: 3,
                mtime_ns: 1,
                permissions: 0o644,
                content_hash: [1; 32],
                block_id: [2; 32],
                block_offset: 4,
                block_len: 3,
                layout: Some(FileLayout::InlineBlock {
                    block_id: [2; 32],
                    offset: 4,
                    len: 3,
                }),
            }],
            blocks: vec![V2BlockEntry {
                block_id: [2; 32],
                raw_size: 7,
                compressed_size: 8,
                encrypted_size: 24,
                archive_offset: 100,
                nonce: [3; NONCE_LEN],
                codec: "zstd".to_string(),
                kind: BlockKind::Batch,
            }],
            root_hash: [4; 32],
        };
        let bytes = bincode::serialize(&manifest).unwrap();
        let decoded: V2Manifest = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn archive_length_checks_overflow() {
        assert_eq!(
            archive_len(10, [20, 30]).unwrap(),
            HEADER_FIXED_LEN as u64 + 60
        );
        assert!(archive_len(u64::MAX, []).is_err());
        assert!(archive_len(0, [u64::MAX]).is_err());
    }

    #[test]
    fn pack_unpack_and_cache_hit() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::create_dir_all(input.join("nested")).unwrap();
        fs::File::create(input.join("a.txt"))
            .unwrap()
            .write_all(b"hello")
            .unwrap();
        fs::File::create(input.join("nested/b.txt"))
            .unwrap()
            .write_all(b"world")
            .unwrap();

        let options = PackOptions {
            input_dir: input.clone(),
            output_file: output.clone(),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        };
        let first = pack(options.clone()).unwrap();
        let second = pack(PackOptions {
            output_file: temp.path().join("out2.hig"),
            ..options
        })
        .unwrap();
        assert_eq!(first.cache.hits, 0);
        assert_eq!(second.cache.hits, 2);
        assert_eq!(second.scan.hashed_files, 2);
        assert_eq!(second.scan.metadata_hash_reuses, 0);

        unpack(UnpackOptions {
            archive_file: output,
            output_dir: restored.clone(),
            password: Some("pw".to_string()),
            overwrite: false,
        })
        .unwrap();
        assert_eq!(fs::read(restored.join("a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(restored.join("nested/b.txt")).unwrap(), b"world");
    }

    #[test]
    fn wrong_password_fails_without_output() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"secret").unwrap();
        pack(PackOptions {
            input_dir: input,
            output_file: output.clone(),
            password: Some("right".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: Some("wrong".to_string()),
                overwrite: false,
            })
            .is_err()
        );
        assert!(!restored.exists());
    }

    #[test]
    fn tampered_archive_fails_without_partial_output() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"first").unwrap();
        fs::write(input.join("b.txt"), b"second").unwrap();
        pack(PackOptions {
            input_dir: input,
            output_file: output.clone(),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();

        let mut bytes = fs::read(&output).unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 0xff;
        fs::write(&output, bytes).unwrap();

        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: Some("pw".to_string()),
                overwrite: false,
            })
            .is_err()
        );
        assert!(!restored.exists());
    }

    #[test]
    fn output_archive_inside_input_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = input.join("out.hig");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"hello").unwrap();
        fs::write(&output, b"old archive placeholder").unwrap();

        let report = pack(PackOptions {
            input_dir: input,
            output_file: output,
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();

        assert_eq!(report.input_files, 1);
    }

    #[test]
    fn trust_metadata_reuses_hashes_during_pack() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"hello").unwrap();
        fs::write(input.join("b.txt"), b"world").unwrap();
        let first = pack(PackOptions {
            input_dir: input.clone(),
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(first.scan.hashed_files, 2);

        let second = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("second.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: true,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(second.cache.hits, 2);
        assert_eq!(second.scan.hashed_files, 0);
        assert_eq!(second.scan.metadata_hash_reuses, 2);
    }

    #[test]
    fn trust_metadata_misses_changed_file() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"hello").unwrap();
        fs::write(input.join("b.txt"), b"world").unwrap();
        pack(PackOptions {
            input_dir: input.clone(),
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(input.join("b.txt"), b"world changed").unwrap();

        let second = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("second.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: true,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(second.scan.metadata_hash_reuses, 1);
        assert_eq!(second.scan.hashed_files, 1);
    }

    #[test]
    fn higv2_batches_small_files_and_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"alpha").unwrap();
        fs::write(input.join("b.txt"), b"beta").unwrap();
        fs::write(input.join("empty.txt"), b"").unwrap();

        let report = pack(PackOptions {
            input_dir: input.clone(),
            output_file: output.clone(),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.blocks.batch_blocks, 1);
        assert_eq!(report.blocks.single_blocks, 0);
        assert_eq!(report.blocks.batched_files, 3);

        unpack(UnpackOptions {
            archive_file: output,
            output_dir: restored.clone(),
            password: Some("pw".to_string()),
            overwrite: false,
        })
        .unwrap();
        assert_eq!(fs::read(restored.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(restored.join("b.txt")).unwrap(), b"beta");
        assert_eq!(fs::read(restored.join("empty.txt")).unwrap(), b"");
    }

    #[test]
    fn higv2_no_batch_uses_single_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"alpha").unwrap();
        fs::write(input.join("b.txt"), b"beta").unwrap();

        let report = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("out.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions {
                enabled: false,
                ..BatchOptions::default()
            },
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.blocks.batch_blocks, 0);
        assert_eq!(report.blocks.single_blocks, 2);
    }

    #[test]
    fn higv2_mixes_batch_and_single_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("small.txt"), b"small").unwrap();
        fs::write(input.join("large.bin"), vec![7_u8; 70_000]).unwrap();

        let report = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("out.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.blocks.batch_blocks, 1);
        assert_eq!(report.blocks.single_blocks, 1);
        assert_eq!(report.blocks.batched_files, 1);
    }

    #[test]
    fn higv2_batch_cache_hit_with_trust_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"alpha").unwrap();
        fs::write(input.join("b.txt"), b"beta").unwrap();
        let options = PackOptions {
            input_dir: input.clone(),
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        };
        let first = pack(options.clone()).unwrap();
        assert_eq!(first.blocks.batch_cache_misses, 1);
        let second = pack(PackOptions {
            output_file: temp.path().join("second.hig"),
            trust_metadata: true,
            ..options
        })
        .unwrap();
        assert_eq!(second.blocks.batch_cache_hits, 1);
        assert_eq!(second.scan.metadata_hash_reuses, 2);
    }

    #[test]
    fn higv2_wrong_password_fails_without_output() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"secret").unwrap();
        pack(PackOptions {
            input_dir: input,
            output_file: output.clone(),
            password: Some("right".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: Some("wrong".to_string()),
                overwrite: false,
            })
            .is_err()
        );
        assert!(!restored.exists());
    }

    #[test]
    fn higv2_tamper_fails_without_partial_output() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"first").unwrap();
        fs::write(input.join("b.txt"), b"second").unwrap();
        pack(PackOptions {
            input_dir: input,
            output_file: output.clone(),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        let mut bytes = fs::read(&output).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&output, bytes).unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: Some("pw".to_string()),
                overwrite: false,
            })
            .is_err()
        );
        assert!(!restored.exists());
    }

    #[test]
    fn v2_legacy_manifest_without_layout_decodes() {
        let legacy = V2ManifestLegacy {
            version: VERSION_V2,
            files: vec![V2FileEntryLegacy {
                relative_path: "a.txt".to_string(),
                size: 3,
                mtime_ns: 1,
                permissions: 0o644,
                content_hash: [1; 32],
                block_id: [2; 32],
                block_offset: 4,
                block_len: 3,
            }],
            blocks: vec![V2BlockEntry {
                block_id: [2; 32],
                raw_size: 7,
                compressed_size: 8,
                encrypted_size: 24,
                archive_offset: 100,
                nonce: [3; NONCE_LEN],
                codec: "zstd".to_string(),
                kind: BlockKind::Batch,
            }],
            root_hash: [4; 32],
        };
        let bytes = bincode::serialize(&legacy).unwrap();
        let decoded = decode_v2_manifest(&bytes).unwrap();
        assert_eq!(decoded.files[0].layout, None);
        assert_eq!(decoded.files[0].block_offset, 4);
    }

    #[test]
    fn planner_uses_cached_chunks_without_reading_file() {
        let hash = *blake3::hash(b"chunk").as_bytes();
        let files = vec![crate::ScannedFile {
            relative_path: "missing-large.bin".to_string(),
            absolute_path: PathBuf::from("/definitely/missing/hig-test.bin"),
            size: 16,
            mtime_secs: 1,
            mtime_ns: 1,
            permissions: 0o644,
            content_hash: hash,
            hash_source: crate::scan::HashSource::MetadataCache,
            cached_chunks: Some(vec![
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
            ]),
        }];
        let plans = plan_blocks(
            &files,
            BatchOptions::default(),
            ChunkOptions {
                enabled: true,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
        )
        .unwrap();
        assert_eq!(plans.len(), 2);
        assert!(matches!(
            plans[0],
            PlannedBlock::Chunk {
                from_metadata_cache: true,
                ..
            }
        ));
    }

    #[test]
    fn higv2_chunks_large_file_and_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("large.bin"), b"aaaaaaaabbbbbbbbccccccccd").unwrap();

        let report = pack(PackOptions {
            input_dir: input.clone(),
            output_file: output.clone(),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.blocks.chunked_files, 1);
        assert_eq!(report.blocks.chunk_blocks, 4);
        assert_eq!(report.blocks.chunk_cache_misses, 4);
        assert_eq!(report.blocks.batch_blocks, 0);
        assert_eq!(report.blocks.single_blocks, 0);

        unpack(UnpackOptions {
            archive_file: output,
            output_dir: restored.clone(),
            password: Some("pw".to_string()),
            overwrite: false,
        })
        .unwrap();
        assert_eq!(
            fs::read(restored.join("large.bin")).unwrap(),
            b"aaaaaaaabbbbbbbbccccccccd"
        );
    }

    #[test]
    fn higv2_chunk_cache_reuses_and_only_misses_changed_chunk() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("large.bin"), b"aaaaaaaabbbbbbbbccccccccd").unwrap();
        let options = PackOptions {
            input_dir: input.clone(),
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        };
        let first = pack(options.clone()).unwrap();
        assert_eq!(first.blocks.chunk_cache_misses, 4);

        let second = pack(PackOptions {
            output_file: temp.path().join("second.hig"),
            ..options.clone()
        })
        .unwrap();
        assert_eq!(second.blocks.chunk_cache_hits, 4);
        assert_eq!(second.blocks.chunk_cache_misses, 0);
        assert_eq!(second.scan.chunk_metadata_reuses, 0);
        assert_eq!(second.blocks.chunk_plan_cache_hits, 0);
        assert_eq!(second.blocks.chunk_plan_cache_misses, 4);

        let trusted = pack(PackOptions {
            output_file: temp.path().join("trusted.hig"),
            trust_metadata: true,
            ..options.clone()
        })
        .unwrap();
        assert_eq!(trusted.blocks.chunk_cache_hits, 4);
        assert_eq!(trusted.blocks.chunk_cache_misses, 0);
        assert_eq!(trusted.scan.hashed_files, 0);
        assert_eq!(trusted.scan.chunk_metadata_reuses, 1);
        assert_eq!(trusted.scan.trusted_bytes_skipped, 25);
        assert_eq!(trusted.blocks.chunk_plan_cache_hits, 4);
        assert_eq!(trusted.blocks.chunk_plan_cache_misses, 0);

        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(input.join("large.bin"), b"aaaaaaaaBBBBBBBBccccccccd").unwrap();
        let third = pack(PackOptions {
            output_file: temp.path().join("third.hig"),
            ..options.clone()
        })
        .unwrap();
        assert_eq!(third.blocks.chunk_cache_hits, 3);
        assert_eq!(third.blocks.chunk_cache_misses, 1);
        assert_eq!(third.blocks.chunk_bytes_compressed, 8);
        assert_eq!(third.scan.chunk_metadata_misses, 0);

        let fourth = pack(PackOptions {
            output_file: temp.path().join("fourth.hig"),
            trust_metadata: true,
            ..options
        })
        .unwrap();
        assert_eq!(fourth.scan.chunk_metadata_reuses, 1);
        assert_eq!(fourth.blocks.chunk_plan_cache_hits, 4);
    }

    #[test]
    fn higv2_fastest_reuses_sealed_chunks_without_reencrypting() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("large.bin"), b"aaaaaaaabbbbbbbbccccccccd").unwrap();
        let options = PackOptions {
            input_dir: input.clone(),
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
            speed: SpeedMode::Fastest,
            kdf_profile: crate::KdfProfile::FastBench,
            sealed_cache: true,
        };
        let first = pack(options.clone()).unwrap();
        assert_eq!(first.blocks.sealed_block_hits, 0);
        assert_eq!(first.blocks.sealed_block_misses, 4);
        assert!(input.join(".hig-cache/blocks").exists());

        let second = pack(PackOptions {
            output_file: temp.path().join("second.hig"),
            ..options.clone()
        })
        .unwrap();
        assert_eq!(second.scan.hashed_files, 0);
        assert_eq!(second.scan.trusted_bytes_skipped, 25);
        assert_eq!(second.blocks.sealed_block_hits, 4);
        assert_eq!(second.blocks.sealed_block_misses, 0);
        assert_eq!(second.blocks.reencrypted_cache_hits, 0);
        assert_eq!(second.blocks.payload_source_cache_files, 4);

        unpack(UnpackOptions {
            archive_file: temp.path().join("second.hig"),
            output_dir: restored.clone(),
            password: Some("pw".to_string()),
            overwrite: false,
        })
        .unwrap();
        assert_eq!(
            fs::read(restored.join("large.bin")).unwrap(),
            b"aaaaaaaabbbbbbbbccccccccd"
        );
    }

    #[test]
    fn higv2_fastest_password_change_misses_sealed_cache() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("large.bin"), b"aaaaaaaabbbbbbbbccccccccd").unwrap();
        let options = PackOptions {
            input_dir: input,
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
            speed: SpeedMode::Fastest,
            kdf_profile: crate::KdfProfile::FastBench,
            sealed_cache: true,
        };
        pack(options.clone()).unwrap();
        let changed_password = pack(PackOptions {
            output_file: temp.path().join("changed-password.hig"),
            password: Some("other".to_string()),
            ..options
        })
        .unwrap();
        assert_eq!(changed_password.blocks.sealed_block_hits, 0);
        assert_eq!(changed_password.blocks.sealed_block_misses, 4);
    }

    #[test]
    fn corrupted_sealed_cache_does_not_replace_existing_archive() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let cache_dir = temp.path().join("cache");
        let target = temp.path().join("target.hig");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("data.bin"), b"sealed cache integrity").unwrap();
        let options = PackOptions {
            input_dir: input,
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: Some(cache_dir.clone()),
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: true,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Fastest,
            kdf_profile: crate::KdfProfile::FastBench,
            sealed_cache: true,
        };
        pack(options.clone()).unwrap();
        let sealed = fs::read_dir(cache_dir.join("blocks"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|value| value.to_str()) == Some("sealed"))
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(sealed)
            .unwrap()
            .set_len(1)
            .unwrap();
        let pack_file = fs::read_dir(cache_dir.join("sealed-packs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|value| value.to_str()) == Some("pack"))
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(pack_file)
            .unwrap()
            .set_len(1)
            .unwrap();
        fs::write(&target, b"existing archive").unwrap();

        let result = pack(PackOptions {
            output_file: target.clone(),
            ..options
        });
        assert!(result.is_err());
        assert_eq!(fs::read(target).unwrap(), b"existing archive");
    }

    #[test]
    fn fastest_large_second_pack_uses_bounded_prefetch() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        let mut state = 0x1234_5678_u32;
        let mut content = vec![0_u8; 9 * 1024 * 1024];
        for byte in &mut content {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        fs::write(input.join("large.bin"), &content).unwrap();
        let options = PackOptions {
            input_dir: input,
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: Some(temp.path().join("cache")),
            threads: Some(4),
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: true,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 1024 * 1024,
                chunk_size: 1024 * 1024,
            },
            speed: SpeedMode::Fastest,
            kdf_profile: crate::KdfProfile::FastBench,
            sealed_cache: true,
        };
        pack(options.clone()).unwrap();
        let second_path = temp.path().join("second.hig");
        let second = pack(PackOptions {
            output_file: second_path.clone(),
            ..options
        })
        .unwrap();
        assert_eq!(
            second.writer_strategy,
            crate::WriterStrategy::PrefetchedCachedFiles
        );
        assert_eq!(second.blocks.sealed_block_hits, 9);
        assert_eq!(second.blocks.cache_pack_hits, 9);
        assert!(second.cached_range_open_count <= 1);
        assert!(second.prefetched_bytes >= 9 * 1024 * 1024);
        assert!(second.peak_pipeline_memory_bytes <= 64 * 1024 * 1024);

        unpack(UnpackOptions {
            archive_file: second_path,
            output_dir: restored.clone(),
            password: Some("pw".to_string()),
            overwrite: false,
        })
        .unwrap();
        assert_eq!(fs::read(restored.join("large.bin")).unwrap(), content);
    }

    #[test]
    fn higv2_no_chunk_uses_single_for_large_file() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("large.bin"), b"aaaaaaaabbbbbbbbccccccccd").unwrap();

        let report = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("out.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions {
                enabled: true,
                small_file_threshold: 8,
                max_batch_raw_bytes: 128,
            },
            chunk: ChunkOptions {
                enabled: false,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.blocks.chunk_blocks, 0);
        assert_eq!(report.blocks.single_blocks, 1);
    }

    #[test]
    fn higv2_trust_metadata_misses_after_normal_mtime_change() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("large.bin"), b"aaaaaaaabbbbbbbbccccccccd").unwrap();
        let options = PackOptions {
            input_dir: input.clone(),
            output_file: temp.path().join("first.hig"),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 16,
                chunk_size: 8,
            },
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        };
        pack(options.clone()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(input.join("large.bin"), b"aaaaaaaaBBBBBBBBccccccccd").unwrap();

        let trusted_after_change = pack(PackOptions {
            output_file: temp.path().join("trusted-after-change.hig"),
            trust_metadata: true,
            ..options
        })
        .unwrap();
        assert_eq!(trusted_after_change.scan.hashed_files, 1);
        assert_eq!(trusted_after_change.scan.chunk_metadata_reuses, 0);
        assert_eq!(trusted_after_change.scan.chunk_metadata_misses, 1);
        assert_eq!(trusted_after_change.blocks.chunk_plan_cache_hits, 0);
        assert_eq!(trusted_after_change.blocks.chunk_plan_cache_misses, 4);
        assert_eq!(trusted_after_change.blocks.chunk_cache_hits, 3);
        assert_eq!(trusted_after_change.blocks.chunk_cache_misses, 1);
    }

    #[test]
    fn higv2_mixes_batch_single_and_chunk_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let output = temp.path().join("out.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("small.txt"), b"small").unwrap();
        fs::write(input.join("middle.bin"), vec![7_u8; 20]).unwrap();
        fs::write(input.join("large.bin"), vec![9_u8; 40]).unwrap();

        let report = pack(PackOptions {
            input_dir: input,
            output_file: output.clone(),
            password: Some("pw".to_string()),
            encryption: EncryptionMode::Password,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions {
                enabled: true,
                small_file_threshold: 16,
                max_batch_raw_bytes: 128,
            },
            chunk: ChunkOptions {
                enabled: true,
                chunk_file_threshold: 32,
                chunk_size: 16,
            },
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.blocks.batch_blocks, 1);
        assert_eq!(report.blocks.single_blocks, 1);
        assert_eq!(report.blocks.chunked_files, 1);
        assert_eq!(report.blocks.chunk_blocks, 3);

        unpack(UnpackOptions {
            archive_file: output,
            output_dir: restored.clone(),
            password: Some("pw".to_string()),
            overwrite: false,
        })
        .unwrap();
        assert_eq!(fs::read(restored.join("small.txt")).unwrap(), b"small");
        assert_eq!(
            fs::read(restored.join("middle.bin")).unwrap(),
            vec![7_u8; 20]
        );
        assert_eq!(
            fs::read(restored.join("large.bin")).unwrap(),
            vec![9_u8; 40]
        );
    }

    #[test]
    fn higv2_none_roundtrips_without_kdf_or_crypto() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let archive_path = temp.path().join("none.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"plain but hashed").unwrap();

        let report = pack(PackOptions {
            input_dir: input,
            output_file: archive_path.clone(),
            password: None,
            encryption: EncryptionMode::None,
            cache_dir: None,
            threads: Some(2),
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        assert_eq!(report.encryption_mode, EncryptionMode::None);
        assert_eq!(report.timings.kdf_ms, 0);
        assert_eq!(report.timings.crypto_ms, 0);

        let mut archive = fs::File::open(&archive_path).unwrap();
        let mut magic = [0_u8; 8];
        archive.read_exact(&mut magic).unwrap();
        let header = read_header_after_magic(&mut archive, VERSION_V2).unwrap();
        assert_eq!(header.encryption, EncryptionMode::None);

        unpack(UnpackOptions {
            archive_file: archive_path,
            output_dir: restored.clone(),
            password: None,
            overwrite: false,
        })
        .unwrap();
        assert_eq!(
            fs::read(restored.join("a.txt")).unwrap(),
            b"plain but hashed"
        );
    }

    #[test]
    fn higv2_none_tamper_fails_without_partial_output() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let archive_path = temp.path().join("none.hig");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"tamper check").unwrap();
        pack(PackOptions {
            input_dir: input,
            output_file: archive_path.clone(),
            password: None,
            encryption: EncryptionMode::None,
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: crate::KdfProfile::Secure,
            sealed_cache: false,
        })
        .unwrap();
        let mut bytes = fs::read(&archive_path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&archive_path, bytes).unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: archive_path,
                output_dir: restored.clone(),
                password: None,
                overwrite: false,
            })
            .is_err()
        );
        assert!(!restored.exists());
    }
}
