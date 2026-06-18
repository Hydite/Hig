use crate::cache::{CacheStats, CacheStore, PathCacheRecord};
use crate::codec;
use crate::crypto::{self, KdfParams, NONCE_LEN, SALT_LEN};
use crate::scan::{ScanOptions, scan_dir, unix_ns};
use crate::{
    ArchiveFormat, BatchOptions, BlockStats, ChunkOptions, Compression, PackOptions, PackReport,
    UnpackOptions,
};
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
}

struct PreparedBlock {
    entry: BlockEntry,
    ciphertext: Vec<u8>,
}

struct PreparedV2Block {
    entry: V2BlockEntry,
    ciphertext: Vec<u8>,
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
    },
}

pub fn pack(options: PackOptions) -> anyhow::Result<PackReport> {
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
        },
    )?;
    let files = scan.files;
    let input_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let mut stats = CacheStats::default();

    let kdf = KdfParams::default();
    let salt = crypto::random_bytes::<SALT_LEN>();
    let key = crypto::derive_key(&options.password, &salt, &kdf)?;
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

    if let Some(parent) = options.output_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut out = fs::File::create(&options.output_file)?;
    write_header(
        &mut out,
        MAGIC_V1,
        &ArchiveHeader {
            version: VERSION_V1,
            kdf,
            salt,
            manifest_nonce,
            manifest_len: manifest_ciphertext.len() as u64,
        },
    )?;
    out.write_all(&manifest_ciphertext)?;
    for block in prepared {
        out.write_all(&block.ciphertext)?;
    }
    out.flush()?;
    let archive_bytes = fs::metadata(&options.output_file)?.len();

    Ok(PackReport {
        input_files: files.len(),
        input_bytes,
        archive_bytes,
        duration: started.elapsed(),
        cache: stats,
        scan: scan.stats,
        blocks: BlockStats {
            single_blocks: files.len(),
            ..BlockStats::default()
        },
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
        },
    )?;
    let files = scan.files;
    let input_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let plans = plan_blocks(&files, options.batch, options.chunk)?;
    let mut cache_stats = CacheStats::default();
    let mut block_stats = BlockStats::default();

    let kdf = KdfParams::default();
    let salt = crypto::random_bytes::<SALT_LEN>();
    let key = crypto::derive_key(&options.password, &salt, &kdf)?;
    let mut prepared = Vec::with_capacity(plans.len());
    let mut file_entries = Vec::with_capacity(files.len());
    let mut chunk_refs: std::collections::BTreeMap<usize, Vec<ChunkRef>> =
        std::collections::BTreeMap::new();

    for plan in plans {
        match plan {
            PlannedBlock::Single { file_index } => {
                block_stats.single_blocks += 1;
                let file = &files[file_index];
                let compressed = if let Some(cache_store) = cache.as_mut() {
                    if let Some(bytes) = cache_store.get(&file.content_hash)? {
                        cache_stats.hits += 1;
                        cache_stats.bytes_reused += file.size;
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
                let nonce = crypto::random_bytes::<NONCE_LEN>();
                let ciphertext = crypto::encrypt(&key, &nonce, &compressed)?;
                let block_id = *blake3::hash(&compressed).as_bytes();
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
                        encrypted_size: ciphertext.len() as u64,
                        archive_offset: 0,
                        nonce,
                        codec: "zstd".to_string(),
                        kind: BlockKind::Single,
                    },
                    ciphertext,
                });
                upsert_path_cache(cache.as_mut(), file)?;
            }
            PlannedBlock::Batch {
                file_indices,
                raw_size,
                batch_key,
            } => {
                block_stats.batch_blocks += 1;
                block_stats.batched_files += file_indices.len();
                let compressed = if let Some(cache_store) = cache.as_mut() {
                    if let Some(bytes) = cache_store.get_batch(&batch_key)? {
                        block_stats.batch_cache_hits += 1;
                        cache_stats.bytes_reused += raw_size;
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
                let nonce = crypto::random_bytes::<NONCE_LEN>();
                let ciphertext = crypto::encrypt(&key, &nonce, &compressed)?;
                let block_id = *blake3::hash(&compressed).as_bytes();
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
                    upsert_path_cache(cache.as_mut(), file)?;
                }
                prepared.push(PreparedV2Block {
                    entry: V2BlockEntry {
                        block_id,
                        raw_size,
                        compressed_size: compressed.len() as u64,
                        encrypted_size: ciphertext.len() as u64,
                        archive_offset: 0,
                        nonce,
                        codec: "zstd".to_string(),
                        kind: BlockKind::Batch,
                    },
                    ciphertext,
                });
            }
            PlannedBlock::Chunk {
                file_index,
                file_offset,
                len,
                chunk_hash,
            } => {
                block_stats.chunk_blocks += 1;
                let file = &files[file_index];
                let compressed = if let Some(cache_store) = cache.as_mut() {
                    if let Some(bytes) = cache_store.get_chunk(&chunk_hash)? {
                        cache_stats.hits += 1;
                        cache_stats.bytes_reused += len;
                        block_stats.chunk_cache_hits += 1;
                        block_stats.chunk_bytes_reused += len;
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
                let nonce = crypto::random_bytes::<NONCE_LEN>();
                let ciphertext = crypto::encrypt(&key, &nonce, &compressed)?;
                let block_id = *blake3::hash(&compressed).as_bytes();
                chunk_refs.entry(file_index).or_default().push(ChunkRef {
                    chunk_hash,
                    block_id,
                    file_offset,
                    len,
                });
                prepared.push(PreparedV2Block {
                    entry: V2BlockEntry {
                        block_id,
                        raw_size: len,
                        compressed_size: compressed.len() as u64,
                        encrypted_size: ciphertext.len() as u64,
                        archive_offset: 0,
                        nonce,
                        codec: "zstd".to_string(),
                        kind: BlockKind::Chunk,
                    },
                    ciphertext,
                });
            }
        }
    }
    for (file_index, chunks) in chunk_refs {
        block_stats.chunked_files += 1;
        let file = &files[file_index];
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
        upsert_path_cache(cache.as_mut(), file)?;
    }
    if let Some(cache_store) = cache.as_ref() {
        cache_store.save()?;
    }

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
    let manifest_ciphertext = crypto::encrypt(&key, &manifest_nonce, &manifest_compressed)?;

    if let Some(parent) = options.output_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut out = fs::File::create(&options.output_file)?;
    write_header(
        &mut out,
        MAGIC_V2,
        &ArchiveHeader {
            version: VERSION_V2,
            kdf,
            salt,
            manifest_nonce,
            manifest_len: manifest_ciphertext.len() as u64,
        },
    )?;
    out.write_all(&manifest_ciphertext)?;
    for block in prepared {
        out.write_all(&block.ciphertext)?;
    }
    out.flush()?;
    let archive_bytes = fs::metadata(&options.output_file)?.len();

    Ok(PackReport {
        input_files: files.len(),
        input_bytes,
        archive_bytes,
        duration: started.elapsed(),
        cache: cache_stats,
        scan: scan.stats,
        blocks: block_stats,
    })
}

fn unpack_v1(options: UnpackOptions, mut archive: fs::File) -> anyhow::Result<()> {
    let header = read_header_after_magic(&mut archive, VERSION_V1)?;
    let mut manifest_ciphertext = vec![0_u8; header.manifest_len as usize];
    archive.read_exact(&mut manifest_ciphertext)?;
    let key = crypto::derive_key(&options.password, &header.salt, &header.kdf)?;
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
    let key = crypto::derive_key(&options.password, &header.salt, &header.kdf)?;
    let manifest_compressed = crypto::decrypt(&key, &header.manifest_nonce, &manifest_ciphertext)?;
    let manifest_plain = codec::decompress_unknown(Compression::Zstd, &manifest_compressed)?;
    let manifest = decode_v2_manifest(&manifest_plain)?;

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
        let compressed = crypto::decrypt(&key, &block.nonce, &ciphertext)?;
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
            append_chunk_plans(files, &mut plans, index, chunk_options.chunk_size)?;
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
        });
        offset += len;
    }
    Ok(())
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
) -> anyhow::Result<()> {
    if let Some(cache_store) = cache {
        cache_store.upsert_path_record(PathCacheRecord {
            relative_path: file.relative_path.clone(),
            size: file.size,
            mtime_ns: file.mtime_ns,
            permissions: file.permissions,
            content_hash: file.content_hash,
            last_seen_unix_ns: unix_ns(SystemTime::now()),
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
    writer.write_all(&(SALT_LEN as u32).to_le_bytes())?;
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
    let salt_len = read_u32(&mut reader)? as usize;
    if salt_len != SALT_LEN {
        anyhow::bail!("unsupported salt length: {salt_len}");
    }
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
            password: "pw".to_string(),
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
            password: "right".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
        })
        .unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: "wrong".to_string(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
                password: "pw".to_string(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
        })
        .unwrap();
        assert_eq!(first.scan.hashed_files, 2);

        let second = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("second.hig"),
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: true,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(input.join("b.txt"), b"world changed").unwrap();

        let second = pack(PackOptions {
            input_dir: input,
            output_file: temp.path().join("second.hig"),
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: true,
            format: ArchiveFormat::HigV1,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
        })
        .unwrap();
        assert_eq!(report.blocks.batch_blocks, 1);
        assert_eq!(report.blocks.single_blocks, 0);
        assert_eq!(report.blocks.batched_files, 3);

        unpack(UnpackOptions {
            archive_file: output,
            output_dir: restored.clone(),
            password: "pw".to_string(),
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
            password: "pw".to_string(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
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
            password: "right".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
        })
        .unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: "wrong".to_string(),
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
            password: "pw".to_string(),
            cache_dir: None,
            threads: None,
            compression: Compression::Zstd,
            level: 1,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
        })
        .unwrap();
        let mut bytes = fs::read(&output).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&output, bytes).unwrap();
        assert!(
            unpack(UnpackOptions {
                archive_file: output,
                output_dir: restored.clone(),
                password: "pw".to_string(),
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
            password: "pw".to_string(),
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
            password: "pw".to_string(),
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
            password: "pw".to_string(),
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

        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(input.join("large.bin"), b"aaaaaaaaBBBBBBBBccccccccd").unwrap();
        let third = pack(PackOptions {
            output_file: temp.path().join("third.hig"),
            ..options
        })
        .unwrap();
        assert_eq!(third.blocks.chunk_cache_hits, 3);
        assert_eq!(third.blocks.chunk_cache_misses, 1);
        assert_eq!(third.blocks.chunk_bytes_compressed, 8);
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
            password: "pw".to_string(),
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
        })
        .unwrap();
        assert_eq!(report.blocks.chunk_blocks, 0);
        assert_eq!(report.blocks.single_blocks, 1);
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
            password: "pw".to_string(),
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
        })
        .unwrap();
        assert_eq!(report.blocks.batch_blocks, 1);
        assert_eq!(report.blocks.single_blocks, 1);
        assert_eq!(report.blocks.chunked_files, 1);
        assert_eq!(report.blocks.chunk_blocks, 3);

        unpack(UnpackOptions {
            archive_file: output,
            output_dir: restored.clone(),
            password: "pw".to_string(),
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
}
