mod archive;
mod cache;
mod codec;
mod crypto;
mod daemon;
mod pipeline;
mod scan;
mod session;
mod writer;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub use archive::{pack, unpack};
pub use cache::{CacheStats, PathChunkRecord};
pub use crypto::{derive_key, random_bytes};
pub use daemon::{daemon_status, run_daemon_server, stop_daemon};
pub use pipeline::{BufferPool, PipelineScheduler};
pub use scan::{ScanStats, ScannedFile};
pub use session::{
    SessionBinding, SessionLookup, SessionMaterial, clear_session, default_session_ttl,
    derive_session_binding, lookup_session, run_session_server, session_socket_path,
    session_status,
};

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub input_dir: PathBuf,
    pub output_file: PathBuf,
    pub password: Option<String>,
    pub encryption: EncryptionMode,
    pub cache_dir: Option<PathBuf>,
    pub threads: Option<usize>,
    pub compression: Compression,
    pub level: Option<i32>,
    pub use_cache: bool,
    pub trust_metadata: bool,
    pub format: ArchiveFormat,
    pub batch: BatchOptions,
    pub chunk: ChunkOptions,
    pub speed: SpeedMode,
    pub kdf_profile: KdfProfile,
    pub sealed_cache: bool,
    pub manifest_format: ManifestFormat,
    pub use_session: bool,
    pub session_required: bool,
    pub session_ttl_secs: Option<u64>,
    pub solid: SolidMode,
    pub pipeline: PipelineOptions,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeedMode {
    #[default]
    Balanced,
    Fastest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestFormat {
    #[default]
    Compact,
    Legacy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolidMode {
    #[default]
    Auto,
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonMode {
    #[default]
    Auto,
    Off,
    Required,
}

impl std::str::FromStr for DaemonMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "required" => Ok(Self::Required),
            other => anyhow::bail!("unsupported daemon mode: {other}"),
        }
    }
}

impl std::str::FromStr for SolidMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            other => anyhow::bail!("unsupported solid mode: {other}"),
        }
    }
}

impl std::str::FromStr for ManifestFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "compact" => Ok(Self::Compact),
            "legacy" => Ok(Self::Legacy),
            other => anyhow::bail!("unsupported manifest format: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionMode {
    #[default]
    Password,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineOptions {
    pub daemon_mode: DaemonMode,
    pub cpu_queue_small_first: bool,
    pub memory_budget_bytes: usize,
    pub io_prefetch_bytes: usize,
    pub cache_pack_enabled: bool,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            daemon_mode: DaemonMode::Auto,
            cpu_queue_small_first: true,
            memory_budget_bytes: 128 * 1024 * 1024,
            io_prefetch_bytes: 4 * 1024 * 1024,
            cache_pack_enabled: true,
        }
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub critical: PackCriticalTimings,
    pub metadata: ArchiveSizeBreakdown,
    pub session: SessionReport,
    pub l1: L1CacheReport,
    pub l2: L2CacheReport,
    pub pipeline: PipelineReport,
}

#[derive(Debug, Clone, Default)]
pub struct PipelineReport {
    pub daemon_used: bool,
    pub daemon_lookup_ms: u128,
    pub scheduler_queue_ms: u128,
    pub cpu_worker_wait_ms: u128,
    pub buffer_pool_hits: u64,
    pub buffer_pool_misses: u64,
    pub cache_pack_range_hits: u64,
    pub cache_pack_open_count: u64,
    pub hot_index_reuses: u64,
    pub hot_metadata_reuses: u64,
    pub pipeline_peak_memory_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionReport {
    pub session_used: bool,
    pub session_lookup_ms: u128,
    pub session_key_age_secs: u64,
    pub kdf_skipped_by_session: bool,
}

#[derive(Debug, Clone, Default)]
pub struct L1CacheReport {
    pub l1_index_hits: usize,
    pub l1_metadata_hits: usize,
    pub l1_scratch_reuses: usize,
    pub rayon_pool_reused: bool,
}

#[derive(Debug, Clone, Default)]
pub struct L2CacheReport {
    pub cache_index_format: String,
    pub cache_index_open_ms: u128,
    pub cache_index_commit_ms: u128,
    pub cache_shards_read: usize,
    pub cache_shards_written: usize,
    pub cache_shard_dirty_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct PackCriticalTimings {
    pub setup_ms: u128,
    pub cache_open_ms: u128,
    pub scan_kdf_wall_ms: u128,
    pub plan_ms: u128,
    pub block_prepare_ms: u128,
    pub cache_commit_ms: u128,
    pub manifest_build_ms: u128,
    pub output_write_ms: u128,
    pub cleanup_ms: u128,
    pub unattributed_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct ArchiveSizeBreakdown {
    pub header_bytes: u64,
    pub manifest_plain_bytes: u64,
    pub manifest_compressed_bytes: u64,
    pub manifest_protected_bytes: u64,
    pub payload_bytes: u64,
    pub total_archive_bytes: u64,
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
    pub compression_level_counts: BTreeMap<i32, usize>,
    pub legacy_cache_hits: usize,
    pub parameterized_cache_hits: usize,
    pub cache_policy_misses: usize,
    pub solid_groups: usize,
    pub solid_files: usize,
    pub solid_cache_hits: usize,
    pub solid_cache_misses: usize,
    pub solid_group_bytes: u64,
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
