use crate::cache::{CacheStats, CacheStore};
use crate::codec;
use crate::crypto::{self, KdfParams, NONCE_LEN, SALT_LEN};
use crate::scan::scan_dir;
use crate::{Compression, PackOptions, PackReport, UnpackOptions};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAGIC: &[u8; 8] = b"HIGV1\0\0\0";
const VERSION: u32 = 1;
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

pub fn pack(options: PackOptions) -> anyhow::Result<PackReport> {
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
    let files = scan_dir(&input_dir, &cache_dir, &options.output_file)?;
    let input_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let mut stats = CacheStats::default();
    let mut cache = if options.use_cache {
        Some(CacheStore::open(&cache_dir)?)
    } else {
        None
    };

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
    }

    let mut manifest = Manifest {
        version: VERSION,
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
        &ArchiveHeader {
            version: VERSION,
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
    })
}

pub fn unpack(options: UnpackOptions) -> anyhow::Result<()> {
    let mut archive = fs::File::open(&options.archive_file)?;
    let header = read_header(&mut archive)?;
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

fn root_hash(files: &[(&String, [u8; 32])]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in files {
        hasher.update(path.as_bytes());
        hasher.update(hash);
    }
    *hasher.finalize().as_bytes()
}

fn write_header(mut writer: impl Write, header: &ArchiveHeader) -> anyhow::Result<()> {
    writer.write_all(MAGIC)?;
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

fn read_header(mut reader: impl Read) -> anyhow::Result<ArchiveHeader> {
    let mut magic = [0_u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        anyhow::bail!("not a hig archive");
    }
    let version = read_u32(&mut reader)?;
    if version != VERSION {
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
            version: VERSION,
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
        };
        let first = pack(options.clone()).unwrap();
        let second = pack(PackOptions {
            output_file: temp.path().join("out2.hig"),
            ..options
        })
        .unwrap();
        assert_eq!(first.cache.hits, 0);
        assert_eq!(second.cache.hits, 2);

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
        })
        .unwrap();

        assert_eq!(report.input_files, 1);
    }
}
