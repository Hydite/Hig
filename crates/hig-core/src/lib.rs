mod archive;
mod cache;
mod codec;
mod crypto;
mod scan;
mod writer;

use std::path::PathBuf;
use std::time::Duration;

pub use archive::{pack, unpack};
pub use cache::{CacheStats, PathChunkRecord};
pub use scan::{ScanStats, ScannedFile};

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub input_dir: PathBuf,
    pub output_file: PathBuf,
    pub password: Option<String>,
    pub encryption: EncryptionMode,
    pub cache_dir: Option<PathBuf>,
    pub threads: Option<usize>,
    pub compression: Compression,
    pub level: i32,
    pub use_cache: bool,
    pub trust_metadata: bool,
    pub format: ArchiveFormat,
    pub batch: BatchOptions,
    pub chunk: ChunkOptions,
    pub speed: SpeedMode,
    pub kdf_profile: KdfProfile,
    pub sealed_cache: bool,
}

#[derive(Debug, Clone)]
pub struct UnpackOptions {
    pub archive_file: PathBuf,
    pub output_dir: PathBuf,
    pub password: Option<String>,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Zstd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArchiveFormat {
    HigV1,
    #[default]
    HigV2,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpeedMode {
    #[default]
    Balanced,
    Fastest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EncryptionMode {
    #[default]
    Password,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WriterStrategy {
    #[default]
    Buffered,
    PrefetchedCachedFiles,
    OrderedPipeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoOptions {
    pub writer_buffer_bytes: usize,
    pub transfer_chunk_bytes: usize,
    pub prefetch_depth: usize,
    pub pipeline_memory_bytes: usize,
}

impl Default for IoOptions {
    fn default() -> Self {
        Self {
            writer_buffer_bytes: 4 * 1024 * 1024,
            transfer_chunk_bytes: 1024 * 1024,
            prefetch_depth: 2,
            pipeline_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

impl std::str::FromStr for EncryptionMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "password" => Ok(Self::Password),
            "none" => Ok(Self::None),
            other => anyhow::bail!("unsupported encryption mode: {other}"),
        }
    }
}

impl std::str::FromStr for SpeedMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "balanced" => Ok(Self::Balanced),
            "fastest" => Ok(Self::Fastest),
            other => anyhow::bail!("unsupported speed mode: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KdfProfile {
    #[default]
    Secure,
    Interactive,
    FastBench,
}

impl std::str::FromStr for KdfProfile {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "secure" => Ok(Self::Secure),
            "interactive" => Ok(Self::Interactive),
            "fast-bench" => Ok(Self::FastBench),
            other => anyhow::bail!("unsupported KDF profile: {other}"),
        }
    }
}

impl std::str::FromStr for ArchiveFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "higv1" => Ok(Self::HigV1),
            "higv2" => Ok(Self::HigV2),
            other => anyhow::bail!("unsupported archive format: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchOptions {
    pub enabled: bool,
    pub small_file_threshold: u64,
    pub max_batch_raw_bytes: u64,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            small_file_threshold: 65_536,
            max_batch_raw_bytes: 4_194_304,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkOptions {
    pub enabled: bool,
    pub chunk_file_threshold: u64,
    pub chunk_size: u64,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            chunk_file_threshold: 8_388_608,
            chunk_size: 1_048_576,
        }
    }
}

impl std::str::FromStr for Compression {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "zstd" => Ok(Self::Zstd),
            other => anyhow::bail!("unsupported compression codec: {other}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackReport {
    pub input_files: usize,
    pub input_bytes: u64,
    pub archive_bytes: u64,
    pub duration: Duration,
    pub timings: PackTimings,
    pub cache: CacheStats,
    pub scan: ScanStats,
    pub blocks: BlockStats,
    pub speed: SpeedMode,
    pub kdf_profile: KdfProfile,
    pub encryption_mode: EncryptionMode,
    pub worker_count: usize,
    pub writer_strategy: WriterStrategy,
    pub archive_preallocated_bytes: u64,
    pub cached_payload_open_count: usize,
    pub cached_range_open_count: usize,
    pub cached_payload_read_bytes: u64,
    pub prefetched_bytes: u64,
    pub peak_pipeline_memory_bytes: u64,
    pub direct_write_count: usize,
    pub buffered_write_count: usize,
    pub preallocation_enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PackTimings {
    pub scan_ms: u128,
    pub plan_ms: u128,
    pub kdf_ms: u128,
    pub pack_blocks_ms: u128,
    pub manifest_ms: u128,
    pub write_ms: u128,
    pub kdf_overlapped_ms: u128,
    pub crypto_ms: u128,
    pub compression_ms: u128,
    pub read_ms: u128,
    pub payload_write_ms: u128,
    pub payload_read_ms: u128,
    pub writer_wait_ms: u128,
    pub output_flush_ms: u128,
    pub output_rename_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct BlockStats {
    pub batch_blocks: usize,
    pub single_blocks: usize,
    pub batched_files: usize,
    pub batch_cache_hits: usize,
    pub batch_cache_misses: usize,
    pub chunked_files: usize,
    pub chunk_blocks: usize,
    pub chunk_cache_hits: usize,
    pub chunk_cache_misses: usize,
    pub chunk_bytes_reused: u64,
    pub chunk_bytes_compressed: u64,
    pub chunk_plan_cache_hits: usize,
    pub chunk_plan_cache_misses: usize,
    pub sealed_block_hits: usize,
    pub sealed_block_misses: usize,
    pub sealed_bytes_reused: u64,
    pub reencrypted_cache_hits: usize,
    pub payload_source_cache_files: usize,
    pub payload_source_memory_bytes: u64,
    pub cache_pack_hits: usize,
    pub cache_pack_misses: usize,
    pub cache_pack_fallbacks: usize,
}

#[derive(Debug, Clone)]
pub struct BenchReport {
    pub first: PackReport,
    pub second: PackReport,
}

pub fn bench(mut options: PackOptions) -> anyhow::Result<BenchReport> {
    let first = pack(options.clone())?;
    let mut second_output = options.output_file.clone();
    let extension = second_output
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("hig");
    second_output.set_extension(format!("second.{extension}"));
    options.output_file = second_output;
    let second = pack(options)?;
    Ok(BenchReport { first, second })
}
