mod archive;
mod cache;
mod codec;
mod crypto;
mod scan;

use std::path::PathBuf;
use std::time::Duration;

pub use archive::{pack, unpack};
pub use cache::CacheStats;
pub use scan::{ScanStats, ScannedFile};

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub input_dir: PathBuf,
    pub output_file: PathBuf,
    pub password: String,
    pub cache_dir: Option<PathBuf>,
    pub threads: Option<usize>,
    pub compression: Compression,
    pub level: i32,
    pub use_cache: bool,
    pub trust_metadata: bool,
    pub format: ArchiveFormat,
    pub batch: BatchOptions,
    pub chunk: ChunkOptions,
}

#[derive(Debug, Clone)]
pub struct UnpackOptions {
    pub archive_file: PathBuf,
    pub output_dir: PathBuf,
    pub password: String,
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
    pub cache: CacheStats,
    pub scan: ScanStats,
    pub blocks: BlockStats,
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
