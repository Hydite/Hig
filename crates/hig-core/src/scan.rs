use crate::ChunkOptions;
use crate::cache::{CacheStore, PathChunkRecord, reusable_path_chunks};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HashSource {
    Computed,
    MetadataCache,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    pub trust_metadata: bool,
    pub chunk: ChunkOptions,
}

#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    pub hashed_files: usize,
    pub metadata_hash_reuses: usize,
    pub scan_cache_hits: usize,
    pub scan_cache_misses: usize,
    pub chunk_metadata_reuses: usize,
    pub chunk_metadata_misses: usize,
    pub trusted_bytes_skipped: u64,
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

    let paths = WalkDir::new(&input_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let path = entry.path();
            !same_or_inside(path, &cache_dir) && !is_hig_cache(path) && path != output_file
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();

    let mut files = paths
        .into_par_iter()
        .map(|path| scan_file(&input_dir, path, cache, options))
        .collect::<anyhow::Result<Vec<_>>>()?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut stats = ScanStats::default();
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
    }
    Ok(ScanReport { files, stats })
}

fn scan_file(
    root: &Path,
    path: PathBuf,
    cache: Option<&CacheStore>,
    options: ScanOptions,
) -> anyhow::Result<ScannedFile> {
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
        return Ok(ScannedFile {
            relative_path,
            absolute_path: path,
            size,
            mtime_secs,
            mtime_ns,
            permissions,
            content_hash: record.content_hash,
            hash_source: HashSource::MetadataCache,
            cached_chunks,
        });
    }
    Ok(ScannedFile {
        relative_path,
        absolute_path: path.clone(),
        size,
        mtime_secs,
        mtime_ns,
        permissions,
        content_hash: *blake3::hash(&fs::read(path)?).as_bytes(),
        hash_source: HashSource::Computed,
        cached_chunks: None,
    })
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

fn is_hig_cache(path: &Path) -> bool {
    path.file_name()
        .map(|name| name == ".hig-cache")
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
            },
        )
        .unwrap();
        assert_eq!(second.stats.metadata_hash_reuses, 1);
        assert_eq!(second.stats.chunk_metadata_reuses, 1);
        assert_eq!(second.stats.trusted_bytes_skipped, 16);
        assert_eq!(second.files[0].cached_chunks.as_ref().unwrap().len(), 2);
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
            },
        )
        .unwrap();
        assert_eq!(second.stats.metadata_hash_reuses, 1);
        assert_eq!(second.stats.chunk_metadata_reuses, 0);
        assert_eq!(second.stats.chunk_metadata_misses, 1);
        assert!(second.files[0].cached_chunks.is_none());
    }
}
