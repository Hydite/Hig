use crate::adaptive_io::{AdaptiveIoController, IoDirection, IoPermit};
use crate::cache::{CacheStore, PathChunkRecord, reusable_path_chunks};
use crate::{ChunkOptions, Compression, codec};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const SMALL_SCAN_BATCH_FILES: usize = 8;
const SMALL_SCAN_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScannedFile {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub size: u64,
    pub mtime_secs: i64,
    pub mtime_ns: i128,
    pub permissions: u32,
    pub content_hash: [u8; 32],
    pub hash_source: HashSource,
    pub cached_chunks: Option<Vec<PathChunkRecord>>,
    #[serde(default, skip)]
    pub hot_chunks: Option<Vec<HotChunkRecord>>,
    #[serde(default, skip)]
    pub raw_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotChunkRecord {
    pub chunk_hash: [u8; 32],
    pub file_offset: u64,
    pub len: u64,
    pub balanced_level: i32,
    pub raw_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HashSource {
    Computed,
    MetadataCache,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    pub trust_metadata: bool,
    pub chunk: ChunkOptions,
    pub hot_raw_bytes_budget: usize,
    pub hot_raw_min_file_bytes: u64,
    pub probe_chunk_levels: bool,
    pub(crate) io_controller: Option<Arc<AdaptiveIoController>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    pub hashed_files: usize,
    pub metadata_hash_reuses: usize,
    pub scan_cache_hits: usize,
    pub scan_cache_misses: usize,
    pub chunk_metadata_reuses: usize,
    pub chunk_metadata_misses: usize,
    pub trusted_bytes_skipped: u64,
    #[serde(default)]
    pub hot_raw_bytes_budget: u64,
    pub hot_raw_bytes: u64,
    pub hot_chunk_raw_bytes: u64,
    pub hot_chunk_plans: usize,
    pub walk_us: u64,
    pub metadata_us: u64,
    pub hash_us: u64,
    #[serde(default)]
    pub read_us: u64,
    #[serde(default)]
    pub content_hash_us: u64,
    #[serde(default)]
    pub scan_wall_us: u64,
}

impl ScanStats {
    pub fn scan_cache_hit_rate(&self) -> f64 {
        let total = self.scan_cache_hits + self.scan_cache_misses;
        if total == 0 {
            0.0
        } else {
            self.scan_cache_hits as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanReport {
    pub files: Vec<ScannedFile>,
    pub stats: ScanStats,
}

pub fn scan_dir(
    input_dir: &Path,
    cache_dir: &Path,
    output_file: &Path,
    cache: Option<&CacheStore>,
    options: ScanOptions,
) -> anyhow::Result<ScanReport> {
    let scan_started = std::time::Instant::now();
    let input_dir = input_dir.canonicalize()?;
    let cache_dir = canonical_or_join(&input_dir, cache_dir);
    let output_file = output_file
        .parent()
        .map(|parent| parent.join(output_file.file_name().unwrap_or_default()))
        .unwrap_or_else(|| output_file.to_path_buf());
    let output_file = if output_file.exists() {
        output_file.canonicalize()?
    } else if output_file.is_absolute() {
        output_file
    } else {
        std::env::current_dir()?.join(output_file)
    };

    let walk_started = std::time::Instant::now();
    let paths = WalkDir::new(&input_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let path = entry.path();
            !same_or_inside(path, &cache_dir) && !is_hig_internal(path) && path != output_file
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    let walk_us = walk_started.elapsed().as_micros() as u64;

    let hot_raw_budget = Arc::new(AtomicUsize::new(options.hot_raw_bytes_budget));
    let outcomes = paths
        .par_chunks(SMALL_SCAN_BATCH_FILES)
        .map(|paths| {
            let mut io_batch = ScanIoBatch::new(options.io_controller.clone());
            let pending = paths
                .iter()
                .map(|path| {
                    scan_file_pending(
                        &input_dir,
                        path.clone(),
                        cache,
                        options.clone(),
                        &mut io_batch,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>();
            io_batch.finish();
            pending?
                .into_par_iter()
                .map(|pending| finish_scan_file(pending, options.clone(), hot_raw_budget.clone()))
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut files = outcomes
        .iter()
        .map(|outcome| outcome.file.clone())
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut stats = ScanStats {
        walk_us,
        metadata_us: outcomes.iter().map(|outcome| outcome.metadata_us).sum(),
        hash_us: outcomes.iter().map(|outcome| outcome.hash_us).sum(),
        read_us: outcomes.iter().map(|outcome| outcome.read_us).sum(),
        content_hash_us: outcomes.iter().map(|outcome| outcome.content_hash_us).sum(),
        scan_wall_us: scan_started.elapsed().as_micros() as u64,
        hot_raw_bytes_budget: options.hot_raw_bytes_budget as u64,
        ..ScanStats::default()
    };
    for file in &files {
        match file.hash_source {
            HashSource::Computed => {
                stats.hashed_files += 1;
                stats.scan_cache_misses += 1;
            }
            HashSource::MetadataCache => {
                stats.metadata_hash_reuses += 1;
                stats.scan_cache_hits += 1;
            }
        }
        if options.trust_metadata
            && options.chunk.enabled
            && file.size > 0
            && file.size >= options.chunk.chunk_file_threshold
        {
            if file.cached_chunks.is_some() {
                stats.chunk_metadata_reuses += 1;
                stats.trusted_bytes_skipped += file.size;
            } else {
                stats.chunk_metadata_misses += 1;
            }
        }
        if let Some(bytes) = &file.raw_bytes {
            stats.hot_raw_bytes += bytes.len() as u64;
        }
        if let Some(chunks) = &file.hot_chunks {
            stats.hot_chunk_plans += chunks.len();
            stats.hot_chunk_raw_bytes += chunks
                .iter()
                .filter_map(|chunk| chunk.raw_bytes.as_ref())
                .map(|bytes| bytes.len() as u64)
                .sum::<u64>();
        }
    }
    Ok(ScanReport { files, stats })
}

struct ScanOutcome {
    file: ScannedFile,
    metadata_us: u64,
    hash_us: u64,
    read_us: u64,
    content_hash_us: u64,
}

enum PendingScanFile {
    Cached(ScanOutcome),
    Raw(PendingRawScanFile),
}

struct PendingRawScanFile {
    relative_path: String,
    absolute_path: PathBuf,
    size: u64,
    mtime_secs: i64,
    mtime_ns: i128,
    permissions: u32,
    metadata_us: u64,
    read_us: u64,
    bytes: Vec<u8>,
}

fn scan_file_pending(
    root: &Path,
    path: PathBuf,
    cache: Option<&CacheStore>,
    options: ScanOptions,
    io_batch: &mut ScanIoBatch,
) -> anyhow::Result<PendingScanFile> {
    let metadata_started = std::time::Instant::now();
    let metadata = fs::metadata(&path)?;
    let relative_path = path
        .strip_prefix(root)?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let modified = metadata.modified().ok();
    let mtime_secs = modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let mtime_ns = modified.map(unix_ns).unwrap_or_default();
    let permissions = permissions(&metadata);
    let size = metadata.len();
    let metadata_us = metadata_started.elapsed().as_micros() as u64;
    if options.trust_metadata
        && let Some(record) = cache.and_then(|cache| cache.get_path_record(&relative_path))
        && record.size == size
        && record.mtime_ns == mtime_ns
        && record.permissions == permissions
    {
        let cached_chunks =
            if options.chunk.enabled && size > 0 && size >= options.chunk.chunk_file_threshold {
                reusable_path_chunks(record, size, options.chunk.chunk_size)
            } else {
                None
            };
        return Ok(PendingScanFile::Cached(ScanOutcome {
            file: ScannedFile {
                relative_path,
                absolute_path: path,
                size,
                mtime_secs,
                mtime_ns,
                permissions,
                content_hash: record.content_hash,
                hash_source: HashSource::MetadataCache,
                cached_chunks,
                hot_chunks: None,
                raw_bytes: None,
            },
            metadata_us,
            hash_us: 0,
            read_us: 0,
            content_hash_us: 0,
        }));
    }
    let read_started = std::time::Instant::now();
    let bytes = read_scan_file(
        &path,
        size,
        options.io_controller.as_ref(),
        "scan-read",
        io_batch,
    )?;
    let read_us = read_started.elapsed().as_micros() as u64;
    Ok(PendingScanFile::Raw(PendingRawScanFile {
        relative_path,
        absolute_path: path,
        size,
        mtime_secs,
        mtime_ns,
        permissions,
        metadata_us,
        read_us,
        bytes,
    }))
}

fn finish_scan_file(
    pending: PendingScanFile,
    options: ScanOptions,
    hot_raw_budget: Arc<AtomicUsize>,
) -> anyhow::Result<ScanOutcome> {
    let raw = match pending {
        PendingScanFile::Cached(outcome) => return Ok(outcome),
        PendingScanFile::Raw(raw) => raw,
    };
    let content_hash_started = std::time::Instant::now();
    let content_hash = *blake3::hash(&raw.bytes).as_bytes();
    let content_hash_us = content_hash_started.elapsed().as_micros() as u64;
    let hash_us = raw.read_us.saturating_add(content_hash_us);
    let keep_whole_raw = raw.size >= options.hot_raw_min_file_bytes
        && reserve_hot_raw_bytes(&hot_raw_budget, raw.bytes.len());
    let hot_chunks = if options.chunk.enabled
        && raw.size > 0
        && raw.size >= options.chunk.chunk_file_threshold
    {
        Some(compute_hot_chunks(
            &raw.bytes,
            options.chunk.chunk_size,
            options.probe_chunk_levels,
            &hot_raw_budget,
            !keep_whole_raw,
        )?)
    } else {
        None
    };
    let raw_bytes = if keep_whole_raw {
        Some(raw.bytes)
    } else {
        None
    };
    Ok(ScanOutcome {
        file: ScannedFile {
            relative_path: raw.relative_path,
            absolute_path: raw.absolute_path,
            size: raw.size,
            mtime_secs: raw.mtime_secs,
            mtime_ns: raw.mtime_ns,
            permissions: raw.permissions,
            content_hash,
            hash_source: HashSource::Computed,
            cached_chunks: None,
            hot_chunks,
            raw_bytes,
        },
        metadata_us: raw.metadata_us,
        hash_us,
        read_us: raw.read_us,
        content_hash_us,
    })
}

struct ScanIoBatch {
    controller: Option<Arc<AdaptiveIoController>>,
    permit: Option<IoPermit>,
    bytes: u64,
}

impl ScanIoBatch {
    fn new(controller: Option<Arc<AdaptiveIoController>>) -> Self {
        Self {
            controller,
            permit: None,
            bytes: 0,
        }
    }

    fn read_small(
        &mut self,
        path: &Path,
        expected_size: u64,
        stage: &'static str,
    ) -> anyhow::Result<Vec<u8>> {
        let mut input = File::open(path)?;
        let capacity = usize::try_from(expected_size).unwrap_or(0);
        let mut bytes = Vec::with_capacity(capacity);
        if self.permit.is_none() {
            let controller = self
                .controller
                .as_ref()
                .expect("small scan batch requires an I/O controller");
            self.permit = Some(controller.acquire(stage, IoDirection::Read, 0));
        }
        input.read_to_end(&mut bytes)?;
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }

    fn finish(&mut self) {
        if let Some(permit) = self.permit.take() {
            permit.finish_with_bytes(self.bytes);
            self.bytes = 0;
        }
    }
}

impl Drop for ScanIoBatch {
    fn drop(&mut self) {
        self.finish();
    }
}

fn read_scan_file(
    path: &Path,
    expected_size: u64,
    controller: Option<&Arc<AdaptiveIoController>>,
    stage: &'static str,
    io_batch: &mut ScanIoBatch,
) -> anyhow::Result<Vec<u8>> {
    if expected_size <= SMALL_SCAN_MAX_BYTES {
        if controller.is_some() {
            return io_batch.read_small(path, expected_size, stage);
        }
    } else {
        io_batch.finish();
        return read_file_adaptive(path, expected_size, controller, stage);
    }
    Ok(fs::read(path)?)
}

pub(crate) fn read_file_adaptive(
    path: &Path,
    expected_size: u64,
    controller: Option<&Arc<AdaptiveIoController>>,
    stage: &'static str,
) -> anyhow::Result<Vec<u8>> {
    let Some(controller) = controller else {
        return Ok(fs::read(path)?);
    };
    let mut input = File::open(path)?;
    let capacity = usize::try_from(expected_size).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    if expected_size <= 1024 * 1024 {
        let permit = controller.acquire(stage, IoDirection::Read, expected_size);
        input.read_to_end(&mut bytes)?;
        permit.finish_with_bytes(bytes.len() as u64);
        return Ok(bytes);
    }
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let permit = controller.acquire(stage, IoDirection::Read, buffer.len() as u64);
        let read = input.read(&mut buffer)?;
        permit.finish_with_bytes(read as u64);
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

fn compute_hot_chunks(
    bytes: &[u8],
    chunk_size: u64,
    probe_chunk_levels: bool,
    hot_raw_budget: &AtomicUsize,
    retain_chunk_raw: bool,
) -> anyhow::Result<Vec<HotChunkRecord>> {
    anyhow::ensure!(chunk_size > 0, "chunk size must be greater than zero");
    let chunk_size = usize::try_from(chunk_size)?;
    let mut chunks = Vec::with_capacity(bytes.len().div_ceil(chunk_size));
    for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
        let file_offset = u64::try_from(index.saturating_mul(chunk_size))?;
        let raw_bytes = if retain_chunk_raw && reserve_hot_raw_bytes(hot_raw_budget, chunk.len()) {
            Some(chunk.to_vec())
        } else {
            None
        };
        chunks.push(HotChunkRecord {
            chunk_hash: *blake3::hash(chunk).as_bytes(),
            file_offset,
            len: chunk.len() as u64,
            balanced_level: if probe_chunk_levels {
                balanced_chunk_level(chunk)
            } else {
                1
            },
            raw_bytes,
        });
    }
    Ok(chunks)
}

fn balanced_chunk_level(raw: &[u8]) -> i32 {
    let probe_len = raw.len().min(64 * 1024);
    if probe_len == 0 {
        return 1;
    }
    match codec::compress(Compression::Zstd, &raw[..probe_len], 1) {
        Ok(probe) if probe.len() * 100 <= probe_len * 90 => 3,
        _ => 1,
    }
}

fn reserve_hot_raw_bytes(budget: &AtomicUsize, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    budget
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
            remaining.checked_sub(len)
        })
        .is_ok()
}

pub fn unix_ns(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(err) => -(err.duration().as_nanos() as i128),
    }
}

fn canonical_or_join(root: &Path, path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn same_or_inside(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

fn is_hig_internal(path: &Path) -> bool {
    path.file_name()
        .map(|name| name == ".hig-cache" || name == ".hig")
        .unwrap_or(false)
}

#[cfg(unix)]
fn permissions(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn permissions(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn hash_distinguishes_content() {
        let temp = tempfile::tempdir().unwrap();
        let mut a = fs::File::create(temp.path().join("a.txt")).unwrap();
        let mut b = fs::File::create(temp.path().join("b.txt")).unwrap();
        a.write_all(b"same").unwrap();
        b.write_all(b"different").unwrap();
        let report = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions::default(),
        )
        .unwrap();
        let files = report.files;
        assert_ne!(files[0].content_hash, files[1].content_hash);
        assert_eq!(report.stats.hashed_files, 2);
    }

    #[test]
    fn trust_metadata_reuses_cached_hash() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("a.txt");
        fs::write(&path, b"same").unwrap();
        let first = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions::default(),
        )
        .unwrap();
        let file = first.files.first().unwrap();
        let mut cache = CacheStore::open(temp.path().join(".hig-cache")).unwrap();
        cache
            .upsert_path_record(crate::cache::PathCacheRecord {
                relative_path: file.relative_path.clone(),
                size: file.size,
                mtime_ns: file.mtime_ns,
                permissions: file.permissions,
                content_hash: file.content_hash,
                last_seen_unix_ns: 1,
                chunk_size: None,
                chunks: Vec::new(),
            })
            .unwrap();
        let second = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            Some(&cache),
            ScanOptions {
                trust_metadata: true,
                chunk: ChunkOptions::default(),
                hot_raw_bytes_budget: 0,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: false,
                io_controller: None,
            },
        )
        .unwrap();
        assert_eq!(second.stats.metadata_hash_reuses, 1);
        assert_eq!(second.stats.hashed_files, 0);
        assert_eq!(second.files[0].hash_source, HashSource::MetadataCache);
    }

    #[test]
    fn trust_metadata_reuses_cached_chunk_plan_for_large_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.bin");
        fs::write(&path, b"aaaaaaaabbbbbbbb").unwrap();
        let chunk = ChunkOptions {
            enabled: true,
            chunk_file_threshold: 16,
            chunk_size: 8,
        };
        let first = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                trust_metadata: false,
                chunk,
                hot_raw_bytes_budget: 0,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: true,
                io_controller: None,
            },
        )
        .unwrap();
        let file = first.files.first().unwrap();
        let first_hash = *blake3::hash(b"aaaaaaaa").as_bytes();
        let second_hash = *blake3::hash(b"bbbbbbbb").as_bytes();
        let mut cache = CacheStore::open(temp.path().join(".hig-cache")).unwrap();
        cache
            .upsert_path_record(crate::cache::PathCacheRecord {
                relative_path: file.relative_path.clone(),
                size: file.size,
                mtime_ns: file.mtime_ns,
                permissions: file.permissions,
                content_hash: file.content_hash,
                last_seen_unix_ns: 1,
                chunk_size: Some(8),
                chunks: vec![
                    PathChunkRecord {
                        chunk_hash: first_hash,
                        file_offset: 0,
                        len: 8,
                    },
                    PathChunkRecord {
                        chunk_hash: second_hash,
                        file_offset: 8,
                        len: 8,
                    },
                ],
            })
            .unwrap();

        let second = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            Some(&cache),
            ScanOptions {
                trust_metadata: true,
                chunk,
                hot_raw_bytes_budget: 0,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: false,
                io_controller: None,
            },
        )
        .unwrap();
        assert_eq!(second.stats.metadata_hash_reuses, 1);
        assert_eq!(second.stats.chunk_metadata_reuses, 1);
        assert_eq!(second.stats.trusted_bytes_skipped, 16);
        assert_eq!(second.files[0].cached_chunks.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn computed_scan_keeps_hot_raw_bytes_within_budget() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"aaaa").unwrap();
        fs::write(temp.path().join("b.txt"), b"bbbb").unwrap();
        let report = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                trust_metadata: false,
                chunk: ChunkOptions::default(),
                hot_raw_bytes_budget: 4,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: false,
                io_controller: None,
            },
        )
        .unwrap();
        assert_eq!(report.stats.hashed_files, 2);
        assert_eq!(report.stats.hot_raw_bytes_budget, 4);
        assert_eq!(report.stats.hot_raw_bytes, 4);
        assert_eq!(
            report
                .files
                .iter()
                .filter(|file| file.raw_bytes.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn computed_scan_can_skip_hot_raw_for_small_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("small.txt"), b"small").unwrap();
        fs::write(temp.path().join("large.bin"), vec![7_u8; 1024 * 1024]).unwrap();
        let report = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                hot_raw_bytes_budget: 2 * 1024 * 1024,
                hot_raw_min_file_bytes: 1024 * 1024,
                ..ScanOptions::default()
            },
        )
        .unwrap();

        assert_eq!(report.stats.hot_raw_bytes, 1024 * 1024);
        assert_eq!(
            report
                .files
                .iter()
                .find(|file| file.relative_path == "small.txt")
                .unwrap()
                .raw_bytes,
            None
        );
        assert!(
            report
                .files
                .iter()
                .find(|file| file.relative_path == "large.bin")
                .unwrap()
                .raw_bytes
                .is_some()
        );
    }

    #[test]
    fn computed_scan_produces_hot_chunk_plan_for_large_file() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("large.bin"), b"aaaaaaaabbbbbbbb").unwrap();
        let report = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                trust_metadata: false,
                chunk: ChunkOptions {
                    enabled: true,
                    chunk_file_threshold: 16,
                    chunk_size: 8,
                },
                hot_raw_bytes_budget: 8,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: true,
                io_controller: None,
            },
        )
        .unwrap();

        assert_eq!(report.stats.hot_chunk_plans, 2);
        assert_eq!(report.stats.hot_chunk_raw_bytes, 8);
        let chunks = report.files[0].hot_chunks.as_ref().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_hash, *blake3::hash(b"aaaaaaaa").as_bytes());
        assert_eq!(chunks[0].balanced_level, 1);
        assert_eq!(chunks[0].raw_bytes.as_deref(), Some(&b"aaaaaaaa"[..]));
        assert!(chunks[1].raw_bytes.is_none());
    }

    #[test]
    fn trust_metadata_chunk_plan_misses_when_chunk_size_differs() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.bin");
        fs::write(&path, b"aaaaaaaabbbbbbbb").unwrap();
        let chunk = ChunkOptions {
            enabled: true,
            chunk_file_threshold: 16,
            chunk_size: 8,
        };
        let first = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                trust_metadata: false,
                chunk,
                hot_raw_bytes_budget: 0,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: true,
                io_controller: None,
            },
        )
        .unwrap();
        let file = first.files.first().unwrap();
        let hash = *blake3::hash(b"aaaaaaaa").as_bytes();
        let mut cache = CacheStore::open(temp.path().join(".hig-cache")).unwrap();
        cache
            .upsert_path_record(crate::cache::PathCacheRecord {
                relative_path: file.relative_path.clone(),
                size: file.size,
                mtime_ns: file.mtime_ns,
                permissions: file.permissions,
                content_hash: file.content_hash,
                last_seen_unix_ns: 1,
                chunk_size: Some(4),
                chunks: vec![PathChunkRecord {
                    chunk_hash: hash,
                    file_offset: 0,
                    len: 16,
                }],
            })
            .unwrap();

        let second = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            Some(&cache),
            ScanOptions {
                trust_metadata: true,
                chunk,
                hot_raw_bytes_budget: 0,
                hot_raw_min_file_bytes: 0,
                probe_chunk_levels: false,
                io_controller: None,
            },
        )
        .unwrap();
        assert_eq!(second.stats.metadata_hash_reuses, 1);
        assert_eq!(second.stats.chunk_metadata_reuses, 0);
        assert_eq!(second.stats.chunk_metadata_misses, 1);
        assert!(second.files[0].cached_chunks.is_none());
    }

    #[test]
    fn adaptive_scan_reports_actual_read_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let content = vec![7_u8; 1024 * 1024 + 17];
        fs::write(temp.path().join("large.bin"), &content).unwrap();
        let controller = AdaptiveIoController::new(4);
        let report = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                io_controller: Some(controller.clone()),
                ..ScanOptions::default()
            },
        )
        .unwrap();

        assert!(report.stats.read_us > 0);
        assert!(report.stats.content_hash_us > 0);
        assert_eq!(
            report.stats.hash_us,
            report.stats.read_us + report.stats.content_hash_us
        );
        let adaptive = controller.report();
        let stage = adaptive.stages.get("scan-read").unwrap();
        assert_eq!(stage.bytes, content.len() as u64);
    }

    #[test]
    fn adaptive_scan_batches_small_reads_into_one_sample() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..8 {
            fs::write(
                temp.path().join(format!("{index}.txt")),
                vec![index as u8; 4096],
            )
            .unwrap();
        }
        let controller = AdaptiveIoController::new(4);
        let report = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
            None,
            ScanOptions {
                io_controller: Some(controller.clone()),
                ..ScanOptions::default()
            },
        )
        .unwrap();

        let adaptive = controller.report();
        let stage = adaptive.stages.get("scan-read").unwrap();
        assert_eq!(stage.bytes, 8 * 4096);
        assert_eq!(stage.samples, 1);
        assert_eq!(report.stats.hashed_files, 8);
    }
}
