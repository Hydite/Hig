use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScannedFile {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub size: u64,
    pub mtime_secs: i64,
    pub permissions: u32,
    pub content_hash: [u8; 32],
}

pub fn scan_dir(
    input_dir: &Path,
    cache_dir: &Path,
    output_file: &Path,
) -> anyhow::Result<Vec<ScannedFile>> {
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
            !same_or_inside(path, &cache_dir) && path != output_file
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();

    let mut files = paths
        .into_par_iter()
        .map(|path| scan_file(&input_dir, path))
        .collect::<anyhow::Result<Vec<_>>>()?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn scan_file(root: &Path, path: PathBuf) -> anyhow::Result<ScannedFile> {
    let metadata = fs::metadata(&path)?;
    let relative_path = path
        .strip_prefix(root)?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let mtime_secs = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    Ok(ScannedFile {
        relative_path,
        absolute_path: path.clone(),
        size: metadata.len(),
        mtime_secs,
        permissions: permissions(&metadata),
        content_hash: *blake3::hash(&fs::read(path)?).as_bytes(),
    })
}

fn canonical_or_join(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn same_or_inside(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
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
        let files = scan_dir(
            temp.path(),
            &temp.path().join(".hig-cache"),
            &temp.path().join("out.hig"),
        )
        .unwrap();
        assert_ne!(files[0].content_hash, files[1].content_hash);
    }
}
