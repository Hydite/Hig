mod archive;
mod cache;
mod codec;
mod crypto;
mod scan;

use std::path::PathBuf;
use std::time::Duration;

pub use archive::{pack, unpack};
pub use cache::CacheStats;
pub use scan::ScannedFile;

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
