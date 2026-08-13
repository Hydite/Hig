use bincode::Options;
use fastcdc::v2020::FastCDC;
use fs2::FileExt;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tree_sitter::{Language, Node, Parser};
use walkdir::WalkDir;

const REPOSITORY_SCHEMA: u16 = 1;
const OBJECT_MAGIC: &[u8; 4] = b"HRO1";
const OBJECT_HEADER_LEN: usize = 56;
const OBJECT_HASH_DOMAIN: &[u8] = b"hig-repository-object-v1\0";
const MAX_OBJECT_RAW_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OBJECT_STORED_BYTES: u64 = 72 * 1024 * 1024;
const PHASE1_CHUNK_BYTES: usize = 1024 * 1024;
const MICRO_CHUNK_MIN_BYTES: usize = 16 * 1024;
const MICRO_CHUNK_TARGET_BYTES: usize = 64 * 1024;
const MICRO_CHUNK_MAX_BYTES: usize = 256 * 1024;
const MIN_REVISION_PREFIX: usize = 8;
const SEMANTIC_PARSER_SCHEMA: u16 = 3;

pub const DEFAULT_REPOSITORY_EXCLUDES: &[&str] = &[
    ".git",
    ".hig",
    ".hig-cache",
    ".venv",
    "venv",
    ".build",
    "DerivedData",
    "node_modules",
    ".next",
    "dist",
    "build",
    "target",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepositoryObjectId([u8; 32]);

impl RepositoryObjectId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for RepositoryObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl fmt::Display for RepositoryObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for RepositoryObjectId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(value)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("repository object id must contain 32 bytes"))?;
        Ok(Self(bytes))
    }
}

impl Serialize for RepositoryObjectId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RepositoryObjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryConfig {
    pub schema: u16,
    pub repository_id: [u8; 16],
    pub created_unix_ns: i128,
    pub excludes: Vec<String>,
    pub chunking: RepositoryChunkingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryChunkingConfig {
    pub schema: u16,
    pub algorithm: String,
    pub target_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryInitReport {
    pub root: String,
    pub repository_dir: String,
    pub repository_id: [u8; 16],
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryMigrationReport {
    pub root: String,
    pub repository_dir: String,
    pub from_legacy: bool,
    pub active_branch: String,
    pub commit_id: Option<RepositoryObjectId>,
    pub objects_rewritten: u64,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryRefKind {
    Head,
    Branch,
    Tag,
    LegacyHead,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRef {
    pub name: String,
    pub kind: RepositoryRefKind,
    pub commit_id: RepositoryObjectId,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRefsReport {
    pub head: Option<RepositoryObjectId>,
    pub active_branch: Option<String>,
    pub refs: Vec<RepositoryRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryBranchReport {
    pub name: String,
    pub commit_id: RepositoryObjectId,
    pub active: bool,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryTagReport {
    pub name: String,
    pub commit_id: RepositoryObjectId,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRefDeleteReport {
    pub name: String,
    pub kind: RepositoryRefKind,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySnapshotReport {
    pub root: String,
    pub commit_id: RepositoryObjectId,
    pub parent_id: Option<RepositoryObjectId>,
    pub tree_id: RepositoryObjectId,
    pub created: bool,
    pub files: u64,
    pub input_bytes: u64,
    pub objects_written: u64,
    pub object_bytes_written: u64,
    pub chunks_reused: u64,
    pub chunks_written: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryCommitSummary {
    pub commit_id: RepositoryObjectId,
    pub parent_id: Option<RepositoryObjectId>,
    pub tree_id: RepositoryObjectId,
    pub created_unix_ns: i128,
    pub message: String,
    pub author: Option<String>,
    pub files: u64,
    pub input_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryChangeKind {
    Added,
    Deleted,
    Modified,
    Metadata,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryByteRange {
    pub old_start: u64,
    pub old_len: u64,
    pub new_start: u64,
    pub new_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryChange {
    pub path: String,
    pub previous_path: Option<String>,
    pub kind: RepositoryChangeKind,
    pub old_file: Option<RepositoryObjectId>,
    pub new_file: Option<RepositoryObjectId>,
    pub old_content_hash: Option<String>,
    pub new_content_hash: Option<String>,
    pub byte_ranges: Vec<RepositoryByteRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryDiffReport {
    pub from: Option<RepositoryObjectId>,
    pub to: RepositoryObjectId,
    pub added: u64,
    pub deleted: u64,
    pub modified: u64,
    pub metadata: u64,
    pub renamed: u64,
    pub changes: Vec<RepositoryChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryPathHistoryEntry {
    pub commit_id: RepositoryObjectId,
    pub parent_id: Option<RepositoryObjectId>,
    pub created_unix_ns: i128,
    pub path: String,
    pub previous_path: Option<String>,
    pub kind: RepositoryChangeKind,
    pub byte_ranges: Vec<RepositoryByteRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryPathHistoryReport {
    pub head: RepositoryObjectId,
    pub query_path: String,
    pub entries: Vec<RepositoryPathHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRestoreReport {
    pub commit_id: RepositoryObjectId,
    pub output_dir: String,
    pub selected_path: Option<String>,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRangeRestoreReport {
    pub commit_id: RepositoryObjectId,
    pub path: String,
    pub start: u64,
    pub len: u64,
    pub output_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryStoragePath {
    pub path: String,
    pub file_object: RepositoryObjectId,
    pub raw_bytes: u64,
    pub chunks: u64,
    pub unique_chunks: u64,
    pub stored_object_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryStorageTreeReport {
    pub commit_id: RepositoryObjectId,
    pub tree_id: RepositoryObjectId,
    pub files: u64,
    pub raw_bytes: u64,
    pub chunks: u64,
    pub unique_chunks: u64,
    pub stored_object_bytes: u64,
    pub paths: Vec<RepositoryStoragePath>,
    pub cache_provenance: Option<RepositoryCacheProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryCacheProvenance {
    pub project_id: [u8; 16],
    pub cache_dir: String,
    pub snapshot_generation: Option<u64>,
    pub cache_generation: Option<u64>,
    pub cache_index_format: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositorySemanticChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Moved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySymbol {
    pub symbol_id: String,
    pub language: String,
    pub path: String,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub content_hash: String,
    pub structural_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySymbolIndexReport {
    pub commit_id: RepositoryObjectId,
    pub symbols: Vec<RepositorySymbol>,
    pub parser_failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySymbolHistoryEntry {
    pub commit_id: RepositoryObjectId,
    pub parent_id: Option<RepositoryObjectId>,
    pub created_unix_ns: i128,
    pub symbol_id: String,
    pub previous_symbol_id: Option<String>,
    pub path: String,
    pub previous_path: Option<String>,
    pub qualified_name: String,
    pub previous_qualified_name: Option<String>,
    pub kind: RepositorySemanticChangeKind,
    pub start_byte: Option<u64>,
    pub end_byte: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySymbolHistoryReport {
    pub head: RepositoryObjectId,
    pub query: String,
    pub resolved_symbol_id: String,
    pub entries: Vec<RepositorySymbolHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySymbolRestoreReport {
    pub commit_id: RepositoryObjectId,
    pub symbol_id: String,
    pub qualified_name: String,
    pub path: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub output_file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryVerifyReport {
    pub refs: u64,
    pub commits: u64,
    pub trees: u64,
    pub files: u64,
    pub chunks: u64,
    pub change_indexes: u64,
    pub semantic_indexes: u64,
    pub compression_tree_indexes: u64,
    pub checked_objects: u64,
    pub checked_raw_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryGcReport {
    pub dry_run: bool,
    pub total_objects: u64,
    pub reachable_objects: u64,
    pub unreachable_objects: u64,
    pub unreachable_bytes: u64,
    pub removed_objects: u64,
    pub removed_bytes: u64,
    pub temporary_files: u64,
    pub temporary_bytes: u64,
    pub removed_temporary_files: u64,
    pub removed_temporary_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ObjectKind {
    Chunk = 1,
    File = 2,
    Tree = 3,
    Commit = 4,
    ChangeIndex = 5,
    SemanticIndex = 6,
    CompressionTreeIndex = 7,
}

impl ObjectKind {
    fn from_u8(value: u8) -> anyhow::Result<Self> {
        match value {
            1 => Ok(Self::Chunk),
            2 => Ok(Self::File),
            3 => Ok(Self::Tree),
            4 => Ok(Self::Commit),
            5 => Ok(Self::ChangeIndex),
            6 => Ok(Self::SemanticIndex),
            7 => Ok(Self::CompressionTreeIndex),
            _ => anyhow::bail!("unknown repository object kind {value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChunkReference {
    object_id: RepositoryObjectId,
    len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObject {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum RepositoryFileType {
    Regular,
    Symlink,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum TreeEntryKind {
    File,
    Tree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeEntry {
    name: String,
    kind: TreeEntryKind,
    object_id: RepositoryObjectId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeObject {
    schema: u16,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CommitObject {
    schema: u16,
    root_tree: RepositoryObjectId,
    parent: Option<RepositoryObjectId>,
    created_unix_ns: i128,
    message: String,
    author: Option<String>,
    files: u64,
    input_bytes: u64,
    change_index: Option<RepositoryObjectId>,
    semantic_index: Option<RepositoryObjectId>,
    compression_tree_index: Option<RepositoryObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChangeIndexObject {
    schema: u16,
    parent_commit: Option<RepositoryObjectId>,
    parent_index: Option<RepositoryObjectId>,
    changes: Vec<RepositoryChange>,
    path_history: BTreeMap<String, Vec<PathHistoryRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PathHistoryRecord {
    commit_id: Option<RepositoryObjectId>,
    parent_id: Option<RepositoryObjectId>,
    created_unix_ns: i128,
    path: String,
    previous_path: Option<String>,
    kind: RepositoryChangeKind,
    byte_ranges: Vec<RepositoryByteRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CompressionTreeIndexObject {
    schema: u16,
    root_tree: RepositoryObjectId,
    files: u64,
    raw_bytes: u64,
    chunks: u64,
    unique_chunks: u64,
    stored_object_bytes: u64,
    paths: Vec<RepositoryStoragePath>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SemanticIndexObject {
    schema: u16,
    parser_schema: u16,
    symbols: Vec<RepositorySymbol>,
    parser_failures: Vec<String>,
    histories: BTreeMap<String, Vec<SemanticHistoryRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SemanticHistoryRecord {
    commit_id: Option<RepositoryObjectId>,
    parent_id: Option<RepositoryObjectId>,
    created_unix_ns: i128,
    symbol_id: String,
    previous_symbol_id: Option<String>,
    path: String,
    previous_path: Option<String>,
    qualified_name: String,
    previous_qualified_name: Option<String>,
    kind: RepositorySemanticChangeKind,
    start_byte: Option<u64>,
    end_byte: Option<u64>,
}

#[derive(Debug, Default)]
struct DirectoryNode {
    files: BTreeMap<String, RepositoryObjectId>,
    directories: BTreeMap<String, DirectoryNode>,
}

#[derive(Debug, Default)]
struct StoreStats {
    objects_written: u64,
    object_bytes_written: u64,
    chunks_written: u64,
    chunks_reused: u64,
    new_objects: Vec<RepositoryObjectId>,
}

#[derive(Debug, Default)]
struct SourceTreePaths {
    directories: Vec<String>,
    files: Vec<SourceFilePath>,
}

#[derive(Debug)]
struct SourceFilePath {
    path: PathBuf,
    file_type: RepositoryFileType,
}

#[derive(Debug, Clone)]
struct Repository {
    root: PathBuf,
    state: PathBuf,
    config: RepositoryConfig,
}

#[derive(Debug, Clone)]
struct FileState {
    object_id: RepositoryObjectId,
    object: FileObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    size: u64,
    modified_ns: i128,
    permissions: u32,
}

pub struct RepositoryWatcher {
    root: PathBuf,
    debounce: Duration,
    receiver: Receiver<notify::Result<Event>>,
    _watcher: RecommendedWatcher,
    pending: bool,
    last_event: Option<Instant>,
}

impl RepositoryWatcher {
    pub fn start(start: &Path, debounce: Duration) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !debounce.is_zero(),
            "watch debounce must be greater than zero"
        );
        let repository = Repository::discover(start)?;
        let root = repository.root.clone();
        let (sender, receiver) = channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self {
            root,
            debounce,
            receiver,
            _watcher: watcher,
            pending: false,
            last_event: None,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn poll(
        &mut self,
        message: &str,
        author: Option<&str>,
    ) -> anyhow::Result<Option<RepositorySnapshotReport>> {
        let repository = Repository::discover(&self.root)?;
        loop {
            match self.receiver.try_recv() {
                Ok(Ok(event)) => {
                    if event.need_rescan()
                        || event.paths.iter().any(|path| {
                            path.strip_prefix(&self.root)
                                .map(|relative| {
                                    !relative.as_os_str().is_empty()
                                        && !is_excluded(relative, &repository.config.excludes)
                                })
                                .unwrap_or(false)
                        })
                    {
                        self.pending = true;
                        self.last_event = Some(Instant::now());
                    }
                }
                Ok(Err(error)) => return Err(error.into()),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("repository watcher disconnected")
                }
            }
        }
        if self.pending
            && self
                .last_event
                .is_some_and(|event| event.elapsed() >= self.debounce)
        {
            self.pending = false;
            self.last_event = None;
            return snapshot_repository(
                &self.root,
                message.to_string(),
                author.map(str::to_string),
            )
            .map(Some);
        }
        Ok(None)
    }
}

pub fn init_repository(root: &Path, excludes: Vec<String>) -> anyhow::Result<RepositoryInitReport> {
    let root = root.canonicalize()?;
    anyhow::ensure!(root.is_dir(), "repository root must be a directory");
    let state = repository_state_dir(&root);
    let config_path = state.join("config.json");
    if config_path.exists() {
        let repository = Repository::open_exact(&root)?;
        return Ok(RepositoryInitReport {
            root: root.display().to_string(),
            repository_dir: repository.state.display().to_string(),
            repository_id: repository.config.repository_id,
            created: false,
        });
    }

    fs::create_dir_all(state.join("objects"))?;
    fs::create_dir_all(state.join("refs").join("heads"))?;
    fs::create_dir_all(state.join("refs").join("tags"))?;
    fs::create_dir_all(state.join("locks"))?;
    let init_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("locks").join("write.lock"))?;
    init_lock.lock_exclusive()?;
    if config_path.exists() {
        let repository = Repository::open_exact(&root)?;
        return Ok(RepositoryInitReport {
            root: root.display().to_string(),
            repository_dir: repository.state.display().to_string(),
            repository_id: repository.config.repository_id,
            created: false,
        });
    }
    let excludes = normalize_excludes(excludes);
    let config = RepositoryConfig {
        schema: REPOSITORY_SCHEMA,
        repository_id: crate::random_bytes::<16>(),
        created_unix_ns: now_unix_ns(),
        excludes,
        chunking: RepositoryChunkingConfig {
            schema: 2,
            algorithm: "fastcdc-v2020".to_string(),
            target_bytes: MICRO_CHUNK_TARGET_BYTES as u64,
        },
    };
    atomic_write(&config_path, &serde_json::to_vec_pretty(&config)?)?;
    // Keep the symbolic active-branch selector outside refs/. The direct
    // refs/HEAD compatibility view remains readable by older HIG versions.
    atomic_write(&state.join("HEAD"), b"ref: refs/heads/main\n")?;
    sync_directory(&state)?;
    Ok(RepositoryInitReport {
        root: root.display().to_string(),
        repository_dir: state.display().to_string(),
        repository_id: config.repository_id,
        created: true,
    })
}

pub fn repository_refs(start: &Path) -> anyhow::Result<RepositoryRefsReport> {
    let repository = Repository::discover(start)?;
    repository.refs_report()
}

pub fn migrate_repository(start: &Path) -> anyhow::Result<RepositoryMigrationReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    let root = repository.root.display().to_string();
    let repository_dir = repository.state.display().to_string();
    if let Some(active_branch) = repository.active_branch()? {
        let commit_id = repository.read_branch(&active_branch)?;
        return Ok(RepositoryMigrationReport {
            root,
            repository_dir,
            from_legacy: false,
            active_branch,
            commit_id,
            objects_rewritten: 0,
            changed: false,
        });
    }

    let legacy_head = read_ref_value(&repository.state.join("refs").join("HEAD"))?;
    let commit_id = legacy_head;
    if let Some(commit_id) = commit_id {
        repository.ensure_commit(commit_id)?;
    }
    let main_path = repository.branch_path("main");
    if let Some(commit_id) = commit_id {
        if let Some(existing) = read_ref_value(&main_path)? {
            anyhow::ensure!(
                existing == commit_id,
                "legacy migration found conflicting refs/heads/main"
            );
        } else {
            repository.update_ref_path(&main_path, commit_id)?;
        }
    } else {
        anyhow::ensure!(
            !main_path.exists(),
            "legacy migration found refs/heads/main without a legacy HEAD"
        );
    }
    atomic_write(&repository.state.join("HEAD"), b"ref: refs/heads/main\n")?;
    sync_directory(&repository.state)?;
    Ok(RepositoryMigrationReport {
        root,
        repository_dir,
        from_legacy: true,
        active_branch: "main".to_string(),
        commit_id,
        objects_rewritten: 0,
        changed: true,
    })
}

pub fn create_repository_branch(
    start: &Path,
    name: &str,
    from_revision: Option<&str>,
) -> anyhow::Result<RepositoryBranchReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    validate_ref_component(name, "branch")?;
    let path = repository.branch_path(name);
    anyhow::ensure!(!path.exists(), "branch already exists: {name}");
    let commit_id = match from_revision {
        Some(revision) => repository.resolve_revision(revision)?,
        None => repository
            .read_head()?
            .ok_or_else(|| anyhow::anyhow!("repository has no snapshots; --from is required"))?,
    };
    repository.ensure_commit(commit_id)?;
    repository.update_branch(name, commit_id)?;
    Ok(RepositoryBranchReport {
        name: name.to_string(),
        commit_id,
        active: repository.active_branch()?.as_deref() == Some(name),
        created: true,
    })
}

pub fn switch_repository_branch(
    start: &Path,
    name: &str,
) -> anyhow::Result<RepositoryBranchReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    validate_ref_component(name, "branch")?;
    let commit_id = repository
        .read_branch(name)?
        .ok_or_else(|| anyhow::anyhow!("branch not found: {name}"))?;
    repository.write_symbolic_head(name)?;
    Ok(RepositoryBranchReport {
        name: name.to_string(),
        commit_id,
        active: true,
        created: false,
    })
}

pub fn delete_repository_branch(
    start: &Path,
    name: &str,
) -> anyhow::Result<RepositoryRefDeleteReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    validate_ref_component(name, "branch")?;
    anyhow::ensure!(
        repository.read_branch(name)?.is_some(),
        "branch not found: {name}"
    );
    anyhow::ensure!(
        repository.active_branch()?.as_deref() != Some(name),
        "cannot delete the active branch: {name}"
    );
    repository.delete_ref_file(&repository.branch_path(name))?;
    Ok(RepositoryRefDeleteReport {
        name: name.to_string(),
        kind: RepositoryRefKind::Branch,
        deleted: true,
    })
}

pub fn create_repository_tag(
    start: &Path,
    name: &str,
    from_revision: Option<&str>,
) -> anyhow::Result<RepositoryTagReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    validate_ref_component(name, "tag")?;
    let path = repository.tag_path(name);
    anyhow::ensure!(!path.exists(), "tag already exists: {name}");
    let commit_id = match from_revision {
        Some(revision) => repository.resolve_revision(revision)?,
        None => repository
            .read_head()?
            .ok_or_else(|| anyhow::anyhow!("repository has no snapshots; --from is required"))?,
    };
    repository.ensure_commit(commit_id)?;
    repository.update_ref_path(&path, commit_id)?;
    Ok(RepositoryTagReport {
        name: name.to_string(),
        commit_id,
        created: true,
    })
}

pub fn delete_repository_tag(
    start: &Path,
    name: &str,
) -> anyhow::Result<RepositoryRefDeleteReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    validate_ref_component(name, "tag")?;
    let path = repository.tag_path(name);
    anyhow::ensure!(path.exists(), "tag not found: {name}");
    repository.delete_ref_file(&path)?;
    Ok(RepositoryRefDeleteReport {
        name: name.to_string(),
        kind: RepositoryRefKind::Tag,
        deleted: true,
    })
}

pub fn repository_branch_names(start: &Path) -> anyhow::Result<Vec<RepositoryRef>> {
    Ok(repository_refs(start)?
        .refs
        .into_iter()
        .filter(|reference| matches!(reference.kind, RepositoryRefKind::Branch))
        .collect())
}

pub fn repository_tag_names(start: &Path) -> anyhow::Result<Vec<RepositoryRef>> {
    Ok(repository_refs(start)?
        .refs
        .into_iter()
        .filter(|reference| matches!(reference.kind, RepositoryRefKind::Tag))
        .collect())
}

pub fn snapshot_repository(
    start: &Path,
    message: String,
    author: Option<String>,
) -> anyhow::Result<RepositorySnapshotReport> {
    let mut repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    repository.upgrade_chunking()?;
    let parent = repository.read_head()?;
    let mut root_node = DirectoryNode::default();
    let mut stats = StoreStats::default();
    let mut files = 0_u64;
    let mut input_bytes = 0_u64;

    let source_paths = repository.source_tree_paths()?;
    for directory in &source_paths.directories {
        insert_directory(&mut root_node, directory)?;
    }
    for source in source_paths.files {
        let relative = source.path.strip_prefix(&repository.root)?;
        let relative = normalize_relative_path(relative)?;
        let (content, fingerprint) = stable_read_source(&source)?;
        let mut chunks = Vec::new();
        for bytes in repository_chunks(&content, repository.config.chunking.schema) {
            let (object_id, written, stored_bytes) =
                repository.put_raw(ObjectKind::Chunk, bytes)?;
            if written {
                stats.objects_written += 1;
                stats.object_bytes_written += stored_bytes;
                stats.chunks_written += 1;
                stats.new_objects.push(object_id);
            } else {
                stats.chunks_reused += 1;
            }
            chunks.push(ChunkReference {
                object_id,
                len: bytes.len() as u64,
            });
        }
        let file = FileObject {
            schema: 1,
            file_type: source.file_type,
            size: content.len() as u64,
            permissions: fingerprint.permissions,
            mtime_ns: fingerprint.modified_ns,
            content_hash: *blake3::hash(&content).as_bytes(),
            chunking_schema: repository.config.chunking.schema,
            chunks,
        };
        let (file_id, written, stored_bytes) = repository.put(ObjectKind::File, &file)?;
        if written {
            stats.objects_written += 1;
            stats.object_bytes_written += stored_bytes;
            stats.new_objects.push(file_id);
        }
        insert_file(&mut root_node, &relative, file_id)?;
        files += 1;
        input_bytes += content.len() as u64;
    }

    let tree_id = write_tree(&repository, &root_node, &mut stats)?;
    let mut parent_commit = None;
    if let Some(parent_id) = parent {
        let value: CommitObject = repository.read(parent_id, ObjectKind::Commit)?;
        let semantic_index_current = match value.semantic_index {
            Some(index_id) => repository
                .read::<SemanticIndexObject>(index_id, ObjectKind::SemanticIndex)
                .map(|index| index.parser_schema == SEMANTIC_PARSER_SCHEMA)
                .unwrap_or(false),
            None => false,
        };
        if value.root_tree == tree_id && semantic_index_current {
            return Ok(RepositorySnapshotReport {
                root: repository.root.display().to_string(),
                commit_id: parent_id,
                parent_id: value.parent,
                tree_id,
                created: false,
                files,
                input_bytes,
                objects_written: stats.objects_written,
                object_bytes_written: stats.object_bytes_written,
                chunks_reused: stats.chunks_reused,
                chunks_written: stats.chunks_written,
            });
        }
        parent_commit = Some(value);
    }

    let old_files = match &parent_commit {
        Some(commit) => flatten_tree(&repository, commit.root_tree)?,
        None => BTreeMap::new(),
    };
    let new_files = flatten_tree(&repository, tree_id)?;
    let changes = build_indexed_changes(&repository, &old_files, &new_files)?;
    let created_unix_ns = now_unix_ns();
    let path_history = build_committed_path_history(
        &repository,
        parent,
        parent_commit.as_ref(),
        created_unix_ns,
        &changes,
    )?;
    let change_index = ChangeIndexObject {
        schema: 1,
        parent_commit: parent,
        parent_index: parent_commit
            .as_ref()
            .and_then(|commit| commit.change_index),
        changes,
        path_history,
    };
    let (change_index_id, written, stored_bytes) =
        repository.put(ObjectKind::ChangeIndex, &change_index)?;
    account_object(&mut stats, change_index_id, written, stored_bytes);

    let compression_tree = build_compression_tree_index(&repository, tree_id, &new_files)?;
    let (compression_tree_index_id, written, stored_bytes) =
        repository.put(ObjectKind::CompressionTreeIndex, &compression_tree)?;
    account_object(&mut stats, compression_tree_index_id, written, stored_bytes);

    let semantic_index = build_semantic_index(
        &repository,
        parent,
        parent_commit.as_ref(),
        created_unix_ns,
        &new_files,
    )?;
    let (semantic_index_id, written, stored_bytes) =
        repository.put(ObjectKind::SemanticIndex, &semantic_index)?;
    account_object(&mut stats, semantic_index_id, written, stored_bytes);

    let commit = CommitObject {
        schema: 1,
        root_tree: tree_id,
        parent,
        created_unix_ns,
        message,
        author,
        files,
        input_bytes,
        change_index: Some(change_index_id),
        semantic_index: Some(semantic_index_id),
        compression_tree_index: Some(compression_tree_index_id),
    };
    let (commit_id, written, stored_bytes) = repository.put(ObjectKind::Commit, &commit)?;
    if written {
        stats.objects_written += 1;
        stats.object_bytes_written += stored_bytes;
        stats.new_objects.push(commit_id);
    }
    repository.sync_new_objects(&stats.new_objects)?;
    repository.publish_head(commit_id)?;
    Ok(RepositorySnapshotReport {
        root: repository.root.display().to_string(),
        commit_id,
        parent_id: parent,
        tree_id,
        created: true,
        files,
        input_bytes,
        objects_written: stats.objects_written,
        object_bytes_written: stats.object_bytes_written,
        chunks_reused: stats.chunks_reused,
        chunks_written: stats.chunks_written,
    })
}

pub fn repository_log(start: &Path, limit: usize) -> anyhow::Result<Vec<RepositoryCommitSummary>> {
    let repository = Repository::discover(start)?;
    let mut current = repository.read_head()?;
    let mut commits = Vec::new();
    while let Some(commit_id) = current {
        if commits.len() == limit {
            break;
        }
        let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
        commits.push(commit_summary(commit_id, &commit));
        current = commit.parent;
    }
    Ok(commits)
}

pub fn repository_diff(
    start: &Path,
    from_revision: Option<&str>,
    to_revision: Option<&str>,
) -> anyhow::Result<RepositoryDiffReport> {
    let repository = Repository::discover(start)?;
    let to = repository.resolve_revision(to_revision.unwrap_or("HEAD"))?;
    let to_commit: CommitObject = repository.read(to, ObjectKind::Commit)?;
    let from = match from_revision {
        Some(value) => Some(repository.resolve_revision(value)?),
        None => to_commit.parent,
    };
    let changes = if from == to_commit.parent {
        match to_commit.change_index {
            Some(index_id) => {
                repository
                    .read::<ChangeIndexObject>(index_id, ObjectKind::ChangeIndex)?
                    .changes
            }
            None => {
                let old = load_commit_files(&repository, from)?;
                let new = flatten_tree(&repository, to_commit.root_tree)?;
                build_indexed_changes(&repository, &old, &new)?
            }
        }
    } else {
        let old = load_commit_files(&repository, from)?;
        let new = flatten_tree(&repository, to_commit.root_tree)?;
        build_indexed_changes(&repository, &old, &new)?
    };
    let mut report = RepositoryDiffReport {
        from,
        to,
        added: 0,
        deleted: 0,
        modified: 0,
        metadata: 0,
        renamed: 0,
        changes,
    };
    for change in &report.changes {
        match change.kind {
            RepositoryChangeKind::Added => report.added += 1,
            RepositoryChangeKind::Deleted => report.deleted += 1,
            RepositoryChangeKind::Modified => report.modified += 1,
            RepositoryChangeKind::Metadata => report.metadata += 1,
            RepositoryChangeKind::Renamed => report.renamed += 1,
        }
    }
    Ok(report)
}

pub fn restore_repository(
    start: &Path,
    revision: &str,
    output_dir: &Path,
    selected_path: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<RepositoryRestoreReport> {
    let repository = Repository::discover(start)?;
    let commit_id = repository.resolve_revision(revision)?;
    let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
    let files = flatten_tree(&repository, commit.root_tree)?;
    let directories = tree_directories(&repository, commit.root_tree)?;
    let selected = selected_path.map(normalize_requested_path).transpose()?;
    let output_absolute = if output_dir.exists() {
        output_dir.canonicalize()?
    } else {
        absolute_path(output_dir)?
    };
    anyhow::ensure!(
        output_absolute != repository.root,
        "restore output cannot replace the repository root"
    );
    if output_dir.exists() && !overwrite {
        anyhow::bail!("restore output already exists; use --overwrite to replace it");
    }
    let stage = unique_sibling(output_dir, "hig-restore-stage");
    fs::create_dir_all(&stage)?;
    let restore_result = (|| -> anyhow::Result<(u64, u64)> {
        let mut restored_files = 0_u64;
        let mut restored_bytes = 0_u64;
        let mut selected_directory = false;
        for directory in &directories {
            if let Some(selected) = &selected
                && directory != selected
                && !directory.starts_with(&format!("{selected}/"))
            {
                continue;
            }
            if selected
                .as_ref()
                .is_some_and(|selected| selected == directory)
            {
                selected_directory = true;
            }
            fs::create_dir_all(safe_join(&stage, directory)?)?;
        }
        for (path, state) in &files {
            if let Some(selected) = &selected
                && path != selected
                && !path.starts_with(&format!("{selected}/"))
            {
                continue;
            }
            let destination = safe_join(&stage, path)?;
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            restore_file(&repository, state, &destination)?;
            restored_files += 1;
            restored_bytes += state.object.size;
        }
        anyhow::ensure!(
            selected.is_none() || restored_files > 0 || selected_directory,
            "selected path does not exist in revision"
        );
        Ok((restored_files, restored_bytes))
    })();
    let (restored_files, restored_bytes) = match restore_result {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
    };
    publish_restored_tree(&stage, output_dir, overwrite)?;
    Ok(RepositoryRestoreReport {
        commit_id,
        output_dir: output_dir.display().to_string(),
        selected_path: selected,
        files: restored_files,
        bytes: restored_bytes,
    })
}

pub fn restore_repository_range(
    start: &Path,
    revision: &str,
    path: &str,
    range_start: u64,
    range_len: Option<u64>,
    output_file: &Path,
    overwrite: bool,
) -> anyhow::Result<RepositoryRangeRestoreReport> {
    let repository = Repository::discover(start)?;
    let commit_id = repository.resolve_revision(revision)?;
    let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
    let normalized = normalize_requested_path(path)?;
    let files = flatten_tree(&repository, commit.root_tree)?;
    let state = files
        .get(&normalized)
        .ok_or_else(|| anyhow::anyhow!("path does not exist in revision"))?;
    anyhow::ensure!(
        state.object.file_type == RepositoryFileType::Regular,
        "range restore requires a regular file"
    );
    anyhow::ensure!(
        range_start <= state.object.size,
        "range start exceeds file size"
    );
    let available = state.object.size - range_start;
    let len = range_len.unwrap_or(available);
    anyhow::ensure!(len <= available, "range exceeds file size");
    anyhow::ensure!(
        !output_file.exists() || overwrite,
        "range output already exists; use --overwrite to replace it"
    );
    let output_absolute = absolute_output_path(output_file)?;
    anyhow::ensure!(
        !output_absolute.starts_with(&repository.state),
        "range output cannot replace repository metadata"
    );

    let bytes = reconstruct_file_range(&repository, state, range_start, len)?;
    atomic_replace_file(output_file, &bytes, overwrite)?;
    Ok(RepositoryRangeRestoreReport {
        commit_id,
        path: normalized,
        start: range_start,
        len,
        output_file: output_file.display().to_string(),
    })
}

pub fn repository_path_history(
    start: &Path,
    path: &str,
    limit: usize,
) -> anyhow::Result<RepositoryPathHistoryReport> {
    anyhow::ensure!(limit > 0, "history limit must be greater than zero");
    let repository = Repository::discover(start)?;
    let head = repository
        .read_head()?
        .ok_or_else(|| anyhow::anyhow!("repository has no snapshots"))?;
    let normalized = normalize_requested_path(path)?;
    let commit: CommitObject = repository.read(head, ObjectKind::Commit)?;
    let index_id = commit
        .change_index
        .ok_or_else(|| anyhow::anyhow!("HEAD does not contain a path-history index"))?;
    let index: ChangeIndexObject = repository.read(index_id, ObjectKind::ChangeIndex)?;
    let entries = index
        .path_history
        .get(&normalized)
        .into_iter()
        .flatten()
        .take(limit)
        .map(|entry| RepositoryPathHistoryEntry {
            commit_id: entry.commit_id.unwrap_or(head),
            parent_id: entry.parent_id,
            created_unix_ns: entry.created_unix_ns,
            path: entry.path.clone(),
            previous_path: entry.previous_path.clone(),
            kind: entry.kind,
            byte_ranges: entry.byte_ranges.clone(),
        })
        .collect();
    Ok(RepositoryPathHistoryReport {
        head,
        query_path: normalized,
        entries,
    })
}

pub fn repository_storage_tree(
    start: &Path,
    revision: &str,
) -> anyhow::Result<RepositoryStorageTreeReport> {
    let repository = Repository::discover(start)?;
    let commit_id = repository.resolve_revision(revision)?;
    let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
    let index_id = commit
        .compression_tree_index
        .ok_or_else(|| anyhow::anyhow!("revision does not contain a compression-tree index"))?;
    let index: CompressionTreeIndexObject =
        repository.read(index_id, ObjectKind::CompressionTreeIndex)?;
    anyhow::ensure!(
        index.root_tree == commit.root_tree,
        "compression-tree root mismatch"
    );
    Ok(RepositoryStorageTreeReport {
        commit_id,
        tree_id: index.root_tree,
        files: index.files,
        raw_bytes: index.raw_bytes,
        chunks: index.chunks,
        unique_chunks: index.unique_chunks,
        stored_object_bytes: index.stored_object_bytes,
        paths: index.paths,
        cache_provenance: capture_cache_provenance(&repository),
    })
}

fn capture_cache_provenance(repository: &Repository) -> Option<RepositoryCacheProvenance> {
    let (project_root, config) = crate::discover_project(&repository.root).ok().flatten()?;
    if project_root != repository.root {
        return None;
    }
    let cache_dir = crate::resolve_project_cache_dir(&project_root, &config);
    let (snapshot_generation, cache_generation, cache_index_format) = if cache_dir.exists() {
        let snapshot = crate::load_snapshot(&cache_dir, &config.project_id).ok();
        let cache = crate::cache::CacheStore::open(&cache_dir).ok();
        (
            snapshot.map(|value| value.generation),
            cache.as_ref().map(crate::cache::CacheStore::generation),
            cache.map(|value| value.index_format().to_string()),
        )
    } else {
        (None, None, None)
    };
    Some(RepositoryCacheProvenance {
        project_id: config.project_id,
        cache_dir: cache_dir.display().to_string(),
        snapshot_generation,
        cache_generation,
        cache_index_format,
    })
}

pub fn repository_symbols(
    start: &Path,
    revision: &str,
    selected_path: Option<&str>,
) -> anyhow::Result<RepositorySymbolIndexReport> {
    let repository = Repository::discover(start)?;
    let commit_id = repository.resolve_revision(revision)?;
    let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
    let index = read_semantic_index(&repository, &commit)?;
    let selected = selected_path.map(normalize_requested_path).transpose()?;
    let symbols = index
        .symbols
        .into_iter()
        .filter(|symbol| selected.as_ref().is_none_or(|path| &symbol.path == path))
        .collect();
    Ok(RepositorySymbolIndexReport {
        commit_id,
        symbols,
        parser_failures: index.parser_failures,
    })
}

pub fn repository_symbol_history(
    start: &Path,
    query: &str,
    limit: usize,
) -> anyhow::Result<RepositorySymbolHistoryReport> {
    anyhow::ensure!(limit > 0, "symbol history limit must be greater than zero");
    let repository = Repository::discover(start)?;
    let head = repository
        .read_head()?
        .ok_or_else(|| anyhow::anyhow!("repository has no snapshots"))?;
    let commit: CommitObject = repository.read(head, ObjectKind::Commit)?;
    let index = read_semantic_index(&repository, &commit)?;
    let resolved_symbol_id = if index.histories.contains_key(query) {
        query.to_string()
    } else {
        resolve_symbol(&index.symbols, query)?.symbol_id.clone()
    };
    let entries = index
        .histories
        .get(&resolved_symbol_id)
        .into_iter()
        .flatten()
        .take(limit)
        .map(|entry| RepositorySymbolHistoryEntry {
            commit_id: entry.commit_id.unwrap_or(head),
            parent_id: entry.parent_id,
            created_unix_ns: entry.created_unix_ns,
            symbol_id: entry.symbol_id.clone(),
            previous_symbol_id: entry.previous_symbol_id.clone(),
            path: entry.path.clone(),
            previous_path: entry.previous_path.clone(),
            qualified_name: entry.qualified_name.clone(),
            previous_qualified_name: entry.previous_qualified_name.clone(),
            kind: entry.kind,
            start_byte: entry.start_byte,
            end_byte: entry.end_byte,
        })
        .collect();
    Ok(RepositorySymbolHistoryReport {
        head,
        query: query.to_string(),
        resolved_symbol_id,
        entries,
    })
}

pub fn restore_repository_symbol(
    start: &Path,
    revision: &str,
    query: &str,
    output_file: &Path,
    overwrite: bool,
) -> anyhow::Result<RepositorySymbolRestoreReport> {
    let repository = Repository::discover(start)?;
    let commit_id = repository.resolve_revision(revision)?;
    let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
    let index = read_semantic_index(&repository, &commit)?;
    let symbol = resolve_symbol(&index.symbols, query)?;
    let files = flatten_tree(&repository, commit.root_tree)?;
    let state = files
        .get(&symbol.path)
        .ok_or_else(|| anyhow::anyhow!("semantic index references a missing path"))?;
    anyhow::ensure!(
        symbol.end_byte >= symbol.start_byte,
        "invalid symbol byte range"
    );
    let bytes = reconstruct_file_range(
        &repository,
        state,
        symbol.start_byte,
        symbol.end_byte - symbol.start_byte,
    )?;
    anyhow::ensure!(
        !output_file.exists() || overwrite,
        "symbol output already exists; use --overwrite to replace it"
    );
    let output_absolute = absolute_output_path(output_file)?;
    anyhow::ensure!(
        !output_absolute.starts_with(&repository.state),
        "symbol output cannot replace repository metadata"
    );
    atomic_replace_file(output_file, &bytes, overwrite)?;
    Ok(RepositorySymbolRestoreReport {
        commit_id,
        symbol_id: symbol.symbol_id.clone(),
        qualified_name: symbol.qualified_name.clone(),
        path: symbol.path.clone(),
        start_byte: symbol.start_byte,
        end_byte: symbol.end_byte,
        output_file: output_file.display().to_string(),
    })
}

pub fn verify_repository(start: &Path) -> anyhow::Result<RepositoryVerifyReport> {
    let repository = Repository::discover(start)?;
    let refs = repository.read_refs()?;
    let mut report = RepositoryVerifyReport {
        refs: refs.len() as u64,
        ..RepositoryVerifyReport::default()
    };
    let mut visited = BTreeSet::new();
    for object_id in refs.values() {
        anyhow::ensure!(
            repository.read_raw(*object_id)?.0 == ObjectKind::Commit,
            "repository ref does not point to a commit"
        );
        verify_reachable(&repository, *object_id, &mut visited, &mut report)?;
    }
    Ok(report)
}

pub fn gc_repository(start: &Path, dry_run: bool) -> anyhow::Result<RepositoryGcReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    let refs = repository.read_refs()?;
    let mut reachable = BTreeSet::new();
    let mut verify = RepositoryVerifyReport::default();
    for object_id in refs.values() {
        anyhow::ensure!(
            repository.read_raw(*object_id)?.0 == ObjectKind::Commit,
            "repository ref does not point to a commit"
        );
        verify_reachable(&repository, *object_id, &mut reachable, &mut verify)?;
    }
    let objects = repository.list_objects()?;
    let mut report = RepositoryGcReport {
        dry_run,
        total_objects: objects.len() as u64,
        reachable_objects: reachable.len() as u64,
        unreachable_objects: 0,
        unreachable_bytes: 0,
        removed_objects: 0,
        removed_bytes: 0,
        temporary_files: 0,
        temporary_bytes: 0,
        removed_temporary_files: 0,
        removed_temporary_bytes: 0,
    };
    for (object_id, path, bytes) in objects {
        if reachable.contains(&object_id) {
            continue;
        }
        report.unreachable_objects += 1;
        report.unreachable_bytes += bytes;
        if !dry_run {
            fs::remove_file(path)?;
            report.removed_objects += 1;
            report.removed_bytes += bytes;
        }
    }
    for (path, bytes) in repository.list_temporary_objects()? {
        report.temporary_files += 1;
        report.temporary_bytes += bytes;
        if !dry_run {
            fs::remove_file(path)?;
            report.removed_temporary_files += 1;
            report.removed_temporary_bytes += bytes;
        }
    }
    Ok(report)
}

impl Repository {
    fn discover(start: &Path) -> anyhow::Result<Self> {
        let absolute = if start.exists() {
            start.canonicalize()?
        } else {
            absolute_path(start)?
        };
        let mut current = if absolute.is_file() {
            absolute.parent().unwrap_or(&absolute).to_path_buf()
        } else {
            absolute
        };
        loop {
            if repository_config_path(&current).exists() {
                return Self::open_exact(&current);
            }
            if !current.pop() {
                break;
            }
        }
        anyhow::bail!("HIG repository not found; run `hig repo init`")
    }

    fn open_exact(root: &Path) -> anyhow::Result<Self> {
        let state = repository_state_dir(root);
        let config: RepositoryConfig =
            serde_json::from_slice(&fs::read(state.join("config.json"))?)?;
        anyhow::ensure!(
            config.schema == REPOSITORY_SCHEMA,
            "unsupported HIG repository schema {}",
            config.schema
        );
        Ok(Self {
            root: root.to_path_buf(),
            state,
            config,
        })
    }

    fn lock_writer(&self) -> anyhow::Result<File> {
        let path = self.state.join("locks").join("write.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn upgrade_chunking(&mut self) -> anyhow::Result<()> {
        let excludes = normalize_excludes(self.config.excludes.clone());
        let needs_chunking_upgrade = self.config.chunking.schema < 2;
        if !needs_chunking_upgrade && excludes == self.config.excludes {
            return Ok(());
        }
        self.config.excludes = excludes;
        if needs_chunking_upgrade {
            self.config.chunking = RepositoryChunkingConfig {
                schema: 2,
                algorithm: "fastcdc-v2020".to_string(),
                target_bytes: MICRO_CHUNK_TARGET_BYTES as u64,
            };
        }
        atomic_write(
            &self.state.join("config.json"),
            &serde_json::to_vec_pretty(&self.config)?,
        )
    }

    fn source_tree_paths(&self) -> anyhow::Result<SourceTreePaths> {
        let mut paths = SourceTreePaths::default();
        for entry in WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.path() == self.root
                    || entry
                        .path()
                        .strip_prefix(&self.root)
                        .map(|relative| !is_excluded(relative, &self.config.excludes))
                        .unwrap_or(false)
            })
        {
            let entry = entry?;
            if entry.path() == self.root {
                continue;
            }
            if entry.file_type().is_dir() {
                paths.directories.push(normalize_relative_path(
                    entry.path().strip_prefix(&self.root)?,
                )?);
            } else if entry.file_type().is_file() {
                paths.files.push(SourceFilePath {
                    path: entry.into_path(),
                    file_type: RepositoryFileType::Regular,
                });
            } else if entry.file_type().is_symlink() {
                paths.files.push(SourceFilePath {
                    path: entry.into_path(),
                    file_type: RepositoryFileType::Symlink,
                });
            }
        }
        Ok(paths)
    }

    fn put<T: Serialize>(
        &self,
        kind: ObjectKind,
        value: &T,
    ) -> anyhow::Result<(RepositoryObjectId, bool, u64)> {
        self.put_raw(kind, &serialize_canonical(value)?)
    }

    fn put_raw(
        &self,
        kind: ObjectKind,
        raw: &[u8],
    ) -> anyhow::Result<(RepositoryObjectId, bool, u64)> {
        anyhow::ensure!(
            raw.len() as u64 <= MAX_OBJECT_RAW_BYTES,
            "repository {kind:?} object exceeds raw size limit: {} > {} bytes",
            raw.len(),
            MAX_OBJECT_RAW_BYTES
        );
        let object_id = compute_object_id(kind, raw);
        let final_path = self.object_path(object_id);
        if final_path.exists() {
            let (stored_kind, existing) = self.read_raw(object_id)?;
            anyhow::ensure!(
                stored_kind == kind && existing == raw,
                "object id collision"
            );
            return Ok((object_id, false, 0));
        }
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let compressed = zstd::stream::encode_all(raw, 3)?;
        let mut bytes = Vec::with_capacity(OBJECT_HEADER_LEN + compressed.len());
        bytes.extend_from_slice(OBJECT_MAGIC);
        bytes.extend_from_slice(&REPOSITORY_SCHEMA.to_le_bytes());
        bytes.push(kind as u8);
        bytes.push(0);
        bytes.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
        bytes.extend_from_slice(blake3::hash(raw).as_bytes());
        bytes.extend_from_slice(&compressed);
        atomic_write_object(&final_path, &bytes)?;
        Ok((object_id, true, bytes.len() as u64))
    }

    fn sync_new_objects(&self, object_ids: &[RepositoryObjectId]) -> anyhow::Result<()> {
        object_ids.par_iter().try_for_each(|object_id| {
            File::open(self.object_path(*object_id))?.sync_all()?;
            Ok::<_, anyhow::Error>(())
        })?;
        let directories = object_ids
            .iter()
            .filter_map(|object_id| self.object_path(*object_id).parent().map(Path::to_path_buf))
            .collect::<BTreeSet<_>>();
        directories
            .par_iter()
            .try_for_each(|directory| sync_directory(directory))?;
        sync_directory(&self.state.join("objects"))?;
        Ok(())
    }

    fn read<T: for<'de> Deserialize<'de>>(
        &self,
        object_id: RepositoryObjectId,
        expected_kind: ObjectKind,
    ) -> anyhow::Result<T> {
        let (kind, raw) = self.read_raw(object_id)?;
        anyhow::ensure!(kind == expected_kind, "repository object kind mismatch");
        deserialize_canonical(&raw)
    }

    fn read_raw(&self, object_id: RepositoryObjectId) -> anyhow::Result<(ObjectKind, Vec<u8>)> {
        let path = self.object_path(object_id);
        anyhow::ensure!(
            fs::metadata(&path)?.len() <= MAX_OBJECT_STORED_BYTES,
            "repository object exceeds stored size limit"
        );
        let bytes = fs::read(path)?;
        anyhow::ensure!(
            bytes.len() >= OBJECT_HEADER_LEN,
            "repository object is truncated"
        );
        anyhow::ensure!(
            &bytes[..4] == OBJECT_MAGIC,
            "invalid repository object magic"
        );
        let schema = u16::from_le_bytes(bytes[4..6].try_into()?);
        anyhow::ensure!(
            schema == REPOSITORY_SCHEMA,
            "unsupported repository object schema"
        );
        let kind = ObjectKind::from_u8(bytes[6])?;
        anyhow::ensure!(bytes[7] == 0, "unsupported repository object flags");
        let raw_len = u64::from_le_bytes(bytes[8..16].try_into()?);
        let compressed_len = u64::from_le_bytes(bytes[16..24].try_into()?);
        anyhow::ensure!(
            raw_len <= MAX_OBJECT_RAW_BYTES,
            "repository object exceeds size limit"
        );
        anyhow::ensure!(
            OBJECT_HEADER_LEN as u64 + compressed_len == bytes.len() as u64,
            "invalid repository object length"
        );
        let mut decoder = zstd::stream::read::Decoder::new(&bytes[OBJECT_HEADER_LEN..])?;
        let mut raw = Vec::with_capacity(raw_len as usize);
        decoder
            .by_ref()
            .take(raw_len.saturating_add(1))
            .read_to_end(&mut raw)?;
        anyhow::ensure!(
            raw.len() as u64 == raw_len,
            "repository object raw length mismatch"
        );
        anyhow::ensure!(
            blake3::hash(&raw).as_bytes() == &bytes[24..56],
            "repository object checksum mismatch"
        );
        anyhow::ensure!(
            compute_object_id(kind, &raw) == object_id,
            "repository object id mismatch"
        );
        Ok((kind, raw))
    }

    fn object_path(&self, object_id: RepositoryObjectId) -> PathBuf {
        let hex = object_id.to_hex();
        self.state.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    fn stored_object_size(&self, object_id: RepositoryObjectId) -> anyhow::Result<u64> {
        Ok(fs::metadata(self.object_path(object_id))?.len())
    }

    fn read_head(&self) -> anyhow::Result<Option<RepositoryObjectId>> {
        if let Some(branch) = self.active_branch()? {
            return self.read_branch(&branch);
        }
        read_ref_value(&self.state.join("refs").join("HEAD"))
    }

    fn active_branch(&self) -> anyhow::Result<Option<String>> {
        let path = self.state.join("HEAD");
        let value = match fs::read_to_string(path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let target = value
            .trim()
            .strip_prefix("ref: refs/heads/")
            .ok_or_else(|| anyhow::anyhow!("invalid repository HEAD selector"))?;
        validate_ref_component(target, "branch")?;
        Ok(Some(target.to_string()))
    }

    fn branch_path(&self, name: &str) -> PathBuf {
        self.state.join("refs").join("heads").join(name)
    }

    fn tag_path(&self, name: &str) -> PathBuf {
        self.state.join("refs").join("tags").join(name)
    }

    fn read_branch(&self, name: &str) -> anyhow::Result<Option<RepositoryObjectId>> {
        validate_ref_component(name, "branch")?;
        read_ref_value(&self.branch_path(name))
    }

    fn update_branch(&self, name: &str, object_id: RepositoryObjectId) -> anyhow::Result<()> {
        validate_ref_component(name, "branch")?;
        self.update_ref_path(&self.branch_path(name), object_id)
    }

    fn write_symbolic_head(&self, branch: &str) -> anyhow::Result<()> {
        validate_ref_component(branch, "branch")?;
        let commit_id = self
            .read_branch(branch)?
            .ok_or_else(|| anyhow::anyhow!("branch not found: {branch}"))?;
        atomic_write(
            &self.state.join("HEAD"),
            format!("ref: refs/heads/{branch}\n").as_bytes(),
        )?;
        self.update_ref_path(&self.state.join("refs").join("HEAD"), commit_id)
    }

    fn publish_head(&self, object_id: RepositoryObjectId) -> anyhow::Result<()> {
        if let Some(branch) = self.active_branch()? {
            self.update_branch(&branch, object_id)?;
        }
        // Direct refs/HEAD is the compatibility view consumed by v1.10 and
        // older tooling. The symbolic selector is the source of truth for new
        // repositories, and both files are updated while the writer lock is held.
        self.update_ref_path(&self.state.join("refs").join("HEAD"), object_id)
    }

    fn update_ref_path(&self, path: &Path, object_id: RepositoryObjectId) -> anyhow::Result<()> {
        atomic_write(path, format!("{object_id}\n").as_bytes())
    }

    fn delete_ref_file(&self, path: &Path) -> anyhow::Result<()> {
        fs::remove_file(path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }

    fn ensure_commit(&self, object_id: RepositoryObjectId) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.read_raw(object_id)?.0 == ObjectKind::Commit,
            "repository ref does not point to a commit"
        );
        Ok(())
    }

    fn read_refs(&self) -> anyhow::Result<BTreeMap<String, RepositoryObjectId>> {
        let mut refs = BTreeMap::new();
        let root = self.state.join("refs");
        for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry
                .path()
                .strip_prefix(&root)?
                .to_string_lossy()
                .to_string();
            if (name == "HEAD" || name.starts_with("heads/") || name.starts_with("tags/"))
                && let Some(value) = read_ref_value(entry.path())?
            {
                refs.insert(name, value);
            }
        }
        Ok(refs)
    }

    fn refs_report(&self) -> anyhow::Result<RepositoryRefsReport> {
        let head = self.read_head()?;
        let active_branch = self.active_branch()?;
        let mut refs = Vec::new();
        if let Some(commit_id) = head {
            refs.push(RepositoryRef {
                name: "HEAD".to_string(),
                kind: if active_branch.is_some() {
                    RepositoryRefKind::Head
                } else {
                    RepositoryRefKind::LegacyHead
                },
                commit_id,
                active: true,
            });
        }
        for (kind, directory_name, display_kind) in [
            (RepositoryRefKind::Branch, "heads", "branch"),
            (RepositoryRefKind::Tag, "tags", "tag"),
        ] {
            let directory = self.state.join("refs").join(directory_name);
            if !directory.exists() {
                continue;
            }
            for entry in WalkDir::new(&directory).into_iter().filter_map(Result::ok) {
                if !entry.file_type().is_file() {
                    continue;
                }
                let name = entry
                    .path()
                    .strip_prefix(&directory)?
                    .to_string_lossy()
                    .to_string();
                validate_ref_component(&name, display_kind)?;
                if let Some(commit_id) = read_ref_value(entry.path())? {
                    refs.push(RepositoryRef {
                        name: name.clone(),
                        kind,
                        commit_id,
                        active: kind == RepositoryRefKind::Branch
                            && active_branch.as_deref() == Some(name.as_str()),
                    });
                }
            }
        }
        refs.sort_by(|left, right| {
            format!("{:?}:{}", left.kind, left.name)
                .cmp(&format!("{:?}:{}", right.kind, right.name))
        });
        Ok(RepositoryRefsReport {
            head,
            active_branch,
            refs,
        })
    }

    fn resolve_revision(&self, revision: &str) -> anyhow::Result<RepositoryObjectId> {
        let revision = revision.trim();
        anyhow::ensure!(!revision.is_empty(), "revision must not be empty");
        if revision.eq_ignore_ascii_case("head") {
            return self
                .read_head()?
                .ok_or_else(|| anyhow::anyhow!("repository has no snapshots"));
        }
        if let Some(object_id) = self.resolve_ref_alias(revision)? {
            return Ok(object_id);
        }
        let revision = revision.to_ascii_lowercase();
        anyhow::ensure!(
            revision.len() >= MIN_REVISION_PREFIX && revision.len() <= 64,
            "revision prefix must contain 8 to 64 hex characters"
        );
        anyhow::ensure!(
            revision.chars().all(|value| value.is_ascii_hexdigit()),
            "invalid revision"
        );
        if revision.len() == 64 {
            let object_id: RepositoryObjectId = revision.parse()?;
            let (kind, _) = self.read_raw(object_id)?;
            anyhow::ensure!(
                kind == ObjectKind::Commit,
                "revision does not identify a commit"
            );
            return Ok(object_id);
        }
        let mut matches = Vec::new();
        for (object_id, _, _) in self.list_objects()? {
            if object_id.to_hex().starts_with(&revision)
                && self.read_raw(object_id)?.0 == ObjectKind::Commit
            {
                matches.push(object_id);
            }
        }
        anyhow::ensure!(!matches.is_empty(), "revision not found");
        anyhow::ensure!(matches.len() == 1, "revision prefix is ambiguous");
        Ok(matches[0])
    }

    fn resolve_ref_alias(&self, revision: &str) -> anyhow::Result<Option<RepositoryObjectId>> {
        let explicit = if let Some(name) = revision.strip_prefix("refs/heads/") {
            Some((RepositoryRefKind::Branch, name))
        } else if let Some(name) = revision.strip_prefix("heads/") {
            Some((RepositoryRefKind::Branch, name))
        } else if let Some(name) = revision.strip_prefix("refs/tags/") {
            Some((RepositoryRefKind::Tag, name))
        } else {
            revision
                .strip_prefix("tags/")
                .map(|name| (RepositoryRefKind::Tag, name))
        };
        if let Some((kind, name)) = explicit {
            validate_ref_component(
                name,
                match kind {
                    RepositoryRefKind::Branch => "branch",
                    RepositoryRefKind::Tag => "tag",
                    _ => "ref",
                },
            )?;
            let object_id = match kind {
                RepositoryRefKind::Branch => self.read_branch(name)?,
                RepositoryRefKind::Tag => read_ref_value(&self.tag_path(name))?,
                _ => None,
            };
            return object_id
                .ok_or_else(|| anyhow::anyhow!("revision ref not found: {revision}"))
                .map(Some);
        }
        let branch = self.read_branch(revision)?;
        let tag = read_ref_value(&self.tag_path(revision))?;
        match (branch, tag) {
            (Some(_), Some(_)) => anyhow::bail!("revision ref is ambiguous: {revision}"),
            (Some(object_id), None) | (None, Some(object_id)) => Ok(Some(object_id)),
            (None, None) => Ok(None),
        }
    }

    fn list_objects(&self) -> anyhow::Result<Vec<(RepositoryObjectId, PathBuf, u64)>> {
        let root = self.state.join("objects");
        let mut objects = Vec::new();
        if !root.exists() {
            return Ok(objects);
        }
        for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(&root)?;
            let parts = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>();
            if parts.len() != 2
                || parts[0].len() != 2
                || parts[1].len() != 62
                || !parts
                    .iter()
                    .all(|part| part.chars().all(|value| value.is_ascii_hexdigit()))
            {
                continue;
            }
            let object_id: RepositoryObjectId = format!("{}{}", parts[0], parts[1]).parse()?;
            objects.push((
                object_id,
                entry.path().to_path_buf(),
                entry.metadata()?.len(),
            ));
        }
        Ok(objects)
    }

    fn list_temporary_objects(&self) -> anyhow::Result<Vec<(PathBuf, u64)>> {
        let root = self.state.join("objects");
        let mut files = Vec::new();
        if !root.exists() {
            return Ok(files);
        }
        for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if name.starts_with('.') && name.contains(".tmp.") {
                files.push((entry.path().to_path_buf(), entry.metadata()?.len()));
            }
        }
        Ok(files)
    }
}

fn write_tree(
    repository: &Repository,
    node: &DirectoryNode,
    stats: &mut StoreStats,
) -> anyhow::Result<RepositoryObjectId> {
    let mut entries = Vec::with_capacity(node.files.len() + node.directories.len());
    for (name, child) in &node.directories {
        let object_id = write_tree(repository, child, stats)?;
        entries.push(TreeEntry {
            name: name.clone(),
            kind: TreeEntryKind::Tree,
            object_id,
        });
    }
    for (name, object_id) in &node.files {
        entries.push(TreeEntry {
            name: name.clone(),
            kind: TreeEntryKind::File,
            object_id: *object_id,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let tree = TreeObject { schema: 1, entries };
    let (object_id, written, stored_bytes) = repository.put(ObjectKind::Tree, &tree)?;
    if written {
        stats.objects_written += 1;
        stats.object_bytes_written += stored_bytes;
        stats.new_objects.push(object_id);
    }
    Ok(object_id)
}

fn insert_file(
    root: &mut DirectoryNode,
    relative: &str,
    object_id: RepositoryObjectId,
) -> anyhow::Result<()> {
    let parts = relative.split('/').collect::<Vec<_>>();
    anyhow::ensure!(!parts.is_empty(), "empty repository path");
    let (name, directories) = parts.split_last().expect("checked above");
    validate_entry_name(name)?;
    let mut node = root;
    for directory in directories {
        validate_entry_name(directory)?;
        node = node
            .directories
            .entry((*directory).to_string())
            .or_default();
    }
    anyhow::ensure!(!node.directories.contains_key(*name), "path type collision");
    node.files.insert((*name).to_string(), object_id);
    Ok(())
}

fn insert_directory(root: &mut DirectoryNode, relative: &str) -> anyhow::Result<()> {
    let mut node = root;
    for directory in relative.split('/') {
        validate_entry_name(directory)?;
        anyhow::ensure!(!node.files.contains_key(directory), "path type collision");
        node = node.directories.entry(directory.to_string()).or_default();
    }
    Ok(())
}

fn flatten_tree(
    repository: &Repository,
    root: RepositoryObjectId,
) -> anyhow::Result<BTreeMap<String, FileState>> {
    let mut files = BTreeMap::new();
    flatten_tree_at(repository, root, "", &mut files)?;
    Ok(files)
}

fn flatten_tree_at(
    repository: &Repository,
    tree_id: RepositoryObjectId,
    prefix: &str,
    files: &mut BTreeMap<String, FileState>,
) -> anyhow::Result<()> {
    let tree: TreeObject = repository.read(tree_id, ObjectKind::Tree)?;
    anyhow::ensure!(tree.schema == 1, "unsupported tree schema");
    let mut previous = None;
    for entry in tree.entries {
        validate_entry_name(&entry.name)?;
        if let Some(previous) = &previous {
            anyhow::ensure!(previous < &entry.name, "tree entries are not canonical");
        }
        previous = Some(entry.name.clone());
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}/{}", entry.name)
        };
        match entry.kind {
            TreeEntryKind::File => {
                let object: FileObject = repository.read(entry.object_id, ObjectKind::File)?;
                anyhow::ensure!(object.schema == 1, "unsupported file object schema");
                anyhow::ensure!(
                    files
                        .insert(
                            path,
                            FileState {
                                object_id: entry.object_id,
                                object,
                            }
                        )
                        .is_none(),
                    "duplicate tree path"
                );
            }
            TreeEntryKind::Tree => flatten_tree_at(repository, entry.object_id, &path, files)?,
        }
    }
    Ok(())
}

fn tree_directories(
    repository: &Repository,
    root: RepositoryObjectId,
) -> anyhow::Result<Vec<String>> {
    let mut directories = Vec::new();
    tree_directories_at(repository, root, "", &mut directories)?;
    Ok(directories)
}

fn tree_directories_at(
    repository: &Repository,
    tree_id: RepositoryObjectId,
    prefix: &str,
    directories: &mut Vec<String>,
) -> anyhow::Result<()> {
    let tree: TreeObject = repository.read(tree_id, ObjectKind::Tree)?;
    for entry in tree.entries {
        validate_entry_name(&entry.name)?;
        if entry.kind != TreeEntryKind::Tree {
            continue;
        }
        let path = if prefix.is_empty() {
            entry.name
        } else {
            format!("{prefix}/{}", entry.name)
        };
        directories.push(path.clone());
        tree_directories_at(repository, entry.object_id, &path, directories)?;
    }
    Ok(())
}

fn restore_file(repository: &Repository, state: &FileState, path: &Path) -> anyhow::Result<()> {
    if state.object.file_type == RepositoryFileType::Symlink {
        let target = reconstruct_file_bytes(repository, state)?;
        create_symlink(&target, path)?;
        return Ok(());
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut written = 0_u64;
    for chunk in &state.object.chunks {
        let (kind, bytes) = repository.read_raw(chunk.object_id)?;
        anyhow::ensure!(
            kind == ObjectKind::Chunk,
            "file references a non-chunk object"
        );
        anyhow::ensure!(bytes.len() as u64 == chunk.len, "chunk length mismatch");
        file.write_all(&bytes)?;
        hasher.update(&bytes);
        written += bytes.len() as u64;
    }
    anyhow::ensure!(
        written == state.object.size,
        "restored file length mismatch"
    );
    anyhow::ensure!(
        hasher.finalize().as_bytes() == &state.object.content_hash,
        "restored file checksum mismatch"
    );
    file.sync_all()?;
    set_permissions(path, state.object.permissions)?;
    Ok(())
}

fn reconstruct_file_bytes(repository: &Repository, state: &FileState) -> anyhow::Result<Vec<u8>> {
    let mut content = Vec::with_capacity(state.object.size as usize);
    for chunk in &state.object.chunks {
        let (kind, bytes) = repository.read_raw(chunk.object_id)?;
        anyhow::ensure!(
            kind == ObjectKind::Chunk,
            "file references a non-chunk object"
        );
        anyhow::ensure!(bytes.len() as u64 == chunk.len, "chunk length mismatch");
        content.extend_from_slice(&bytes);
    }
    anyhow::ensure!(
        content.len() as u64 == state.object.size,
        "restored file length mismatch"
    );
    anyhow::ensure!(
        blake3::hash(&content).as_bytes() == &state.object.content_hash,
        "restored file checksum mismatch"
    );
    Ok(content)
}

fn repository_chunks(content: &[u8], schema: u16) -> Vec<&[u8]> {
    if schema < 2 {
        return content.chunks(PHASE1_CHUNK_BYTES).collect();
    }
    FastCDC::new(
        content,
        MICRO_CHUNK_MIN_BYTES,
        MICRO_CHUNK_TARGET_BYTES,
        MICRO_CHUNK_MAX_BYTES,
    )
    .map(|chunk| &content[chunk.offset..chunk.offset + chunk.length])
    .collect()
}

fn account_object(
    stats: &mut StoreStats,
    object_id: RepositoryObjectId,
    written: bool,
    stored_bytes: u64,
) {
    if written {
        stats.objects_written += 1;
        stats.object_bytes_written += stored_bytes;
        stats.new_objects.push(object_id);
    }
}

fn load_commit_files(
    repository: &Repository,
    commit_id: Option<RepositoryObjectId>,
) -> anyhow::Result<BTreeMap<String, FileState>> {
    match commit_id {
        Some(id) => {
            let commit: CommitObject = repository.read(id, ObjectKind::Commit)?;
            flatten_tree(repository, commit.root_tree)
        }
        None => Ok(BTreeMap::new()),
    }
}

fn build_indexed_changes(
    repository: &Repository,
    old: &BTreeMap<String, FileState>,
    new: &BTreeMap<String, FileState>,
) -> anyhow::Result<Vec<RepositoryChange>> {
    let paths = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let deleted = paths
        .iter()
        .filter(|path| old.contains_key(*path) && !new.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    let added = paths
        .iter()
        .filter(|path| new.contains_key(*path) && !old.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    let mut rename_from = BTreeMap::new();
    let mut renamed_old = BTreeSet::new();
    for new_path in &added {
        let new_state = &new[new_path];
        let candidates = deleted
            .iter()
            .filter(|old_path| {
                let old_state = &old[*old_path];
                !renamed_old.contains(*old_path)
                    && old_state.object.file_type == new_state.object.file_type
                    && old_state.object.content_hash == new_state.object.content_hash
            })
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            let old_path = candidates[0].clone();
            renamed_old.insert(old_path.clone());
            rename_from.insert(new_path.clone(), old_path);
        }
    }

    let mut changes = Vec::new();
    for path in paths {
        let previous = old.get(&path);
        let current = new.get(&path);
        if previous.is_some() && current.is_none() && renamed_old.contains(&path) {
            continue;
        }
        let (kind, previous_path, byte_ranges) = match (previous, current) {
            (None, Some(right)) => match rename_from.get(&path) {
                Some(old_path) => (
                    RepositoryChangeKind::Renamed,
                    Some(old_path.clone()),
                    Vec::new(),
                ),
                None => (
                    RepositoryChangeKind::Added,
                    None,
                    vec![RepositoryByteRange {
                        old_start: 0,
                        old_len: 0,
                        new_start: 0,
                        new_len: right.object.size,
                    }],
                ),
            },
            (Some(left), None) => (
                RepositoryChangeKind::Deleted,
                None,
                vec![RepositoryByteRange {
                    old_start: 0,
                    old_len: left.object.size,
                    new_start: 0,
                    new_len: 0,
                }],
            ),
            (Some(left), Some(right)) if left.object.content_hash != right.object.content_hash => {
                let old_bytes = reconstruct_file_bytes(repository, left)?;
                let new_bytes = reconstruct_file_bytes(repository, right)?;
                (
                    RepositoryChangeKind::Modified,
                    None,
                    byte_change_ranges(&old_bytes, &new_bytes),
                )
            }
            (Some(left), Some(right))
                if left.object.permissions != right.object.permissions
                    || left.object.mtime_ns != right.object.mtime_ns =>
            {
                (RepositoryChangeKind::Metadata, None, Vec::new())
            }
            (Some(_), Some(_)) | (None, None) => continue,
        };
        let old_state = previous.or_else(|| {
            previous_path
                .as_ref()
                .and_then(|old_path| old.get(old_path))
        });
        changes.push(RepositoryChange {
            path,
            previous_path,
            kind,
            old_file: old_state.map(|state| state.object_id),
            new_file: current.map(|state| state.object_id),
            old_content_hash: old_state.map(|state| hex::encode(state.object.content_hash)),
            new_content_hash: current.map(|state| hex::encode(state.object.content_hash)),
            byte_ranges,
        });
    }
    Ok(changes)
}

fn byte_change_ranges(old: &[u8], new: &[u8]) -> Vec<RepositoryByteRange> {
    if old == new {
        return Vec::new();
    }
    if old.len() == new.len() {
        let mut ranges = Vec::new();
        let mut cursor = 0;
        while cursor < old.len() {
            if old[cursor] == new[cursor] {
                cursor += 1;
                continue;
            }
            let start = cursor;
            while cursor < old.len() && old[cursor] != new[cursor] {
                cursor += 1;
            }
            ranges.push(RepositoryByteRange {
                old_start: start as u64,
                old_len: (cursor - start) as u64,
                new_start: start as u64,
                new_len: (cursor - start) as u64,
            });
        }
        return ranges;
    }

    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old[old.len() - max_suffix..]
        .iter()
        .rev()
        .zip(new[new.len() - max_suffix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    vec![RepositoryByteRange {
        old_start: prefix as u64,
        old_len: (old.len() - prefix - suffix) as u64,
        new_start: prefix as u64,
        new_len: (new.len() - prefix - suffix) as u64,
    }]
}

fn build_compression_tree_index(
    repository: &Repository,
    root_tree: RepositoryObjectId,
    files: &BTreeMap<String, FileState>,
) -> anyhow::Result<CompressionTreeIndexObject> {
    let mut paths = Vec::with_capacity(files.len());
    let mut global_chunks = BTreeSet::new();
    let mut stored_objects = BTreeSet::new();
    let mut raw_bytes = 0_u64;
    let mut chunks = 0_u64;
    for (path, state) in files {
        let unique = state
            .object
            .chunks
            .iter()
            .map(|chunk| chunk.object_id)
            .collect::<BTreeSet<_>>();
        let mut stored_object_bytes = repository.stored_object_size(state.object_id)?;
        stored_objects.insert(state.object_id);
        for chunk in &unique {
            stored_object_bytes += repository.stored_object_size(*chunk)?;
            stored_objects.insert(*chunk);
            global_chunks.insert(*chunk);
        }
        raw_bytes += state.object.size;
        chunks += state.object.chunks.len() as u64;
        paths.push(RepositoryStoragePath {
            path: path.clone(),
            file_object: state.object_id,
            raw_bytes: state.object.size,
            chunks: state.object.chunks.len() as u64,
            unique_chunks: unique.len() as u64,
            stored_object_bytes,
        });
    }
    let stored_object_bytes = stored_objects.into_iter().try_fold(0_u64, |total, id| {
        Ok::<_, anyhow::Error>(total + repository.stored_object_size(id)?)
    })?;
    Ok(CompressionTreeIndexObject {
        schema: 1,
        root_tree,
        files: files.len() as u64,
        raw_bytes,
        chunks,
        unique_chunks: global_chunks.len() as u64,
        stored_object_bytes,
        paths,
    })
}

fn read_semantic_index(
    repository: &Repository,
    commit: &CommitObject,
) -> anyhow::Result<SemanticIndexObject> {
    let index_id = commit
        .semantic_index
        .ok_or_else(|| anyhow::anyhow!("revision does not contain a semantic index"))?;
    let index: SemanticIndexObject = repository.read(index_id, ObjectKind::SemanticIndex)?;
    anyhow::ensure!(index.schema == 1, "unsupported semantic-index schema");
    anyhow::ensure!(
        (1..=SEMANTIC_PARSER_SCHEMA).contains(&index.parser_schema),
        "unsupported semantic parser schema"
    );
    Ok(index)
}

fn build_semantic_index(
    repository: &Repository,
    parent_id: Option<RepositoryObjectId>,
    parent_commit: Option<&CommitObject>,
    created_unix_ns: i128,
    files: &BTreeMap<String, FileState>,
) -> anyhow::Result<SemanticIndexObject> {
    let parent_index = match parent_commit.and_then(|commit| commit.semantic_index) {
        Some(index_id) => {
            Some(repository.read::<SemanticIndexObject>(index_id, ObjectKind::SemanticIndex)?)
        }
        None => None,
    };
    let mut symbols = Vec::new();
    let mut parser_failures = Vec::new();
    let mut failed_paths = BTreeSet::new();
    for (path, state) in files {
        if state.object.file_type != RepositoryFileType::Regular
            || semantic_language(path).is_none()
        {
            continue;
        }
        let content = reconstruct_file_bytes(repository, state)?;
        match parse_symbols(path, &content) {
            Ok((mut parsed, has_errors)) => {
                if has_errors {
                    failed_paths.insert(path.clone());
                    parser_failures.push(format!("{path}: syntax tree contains errors"));
                    continue;
                }
                symbols.append(&mut parsed);
            }
            Err(error) => {
                failed_paths.insert(path.clone());
                parser_failures.push(format!("{path}: {error}"));
            }
        }
    }
    symbols.sort_by(|left, right| {
        (&left.path, left.start_byte, &left.symbol_id).cmp(&(
            &right.path,
            right.start_byte,
            &right.symbol_id,
        ))
    });
    let old_symbols = parent_index
        .as_ref()
        .map(|index| index.symbols.as_slice())
        .unwrap_or_default();
    let changes = semantic_changes(old_symbols, &symbols, &failed_paths);
    let histories =
        build_semantic_histories(parent_index.as_ref(), parent_id, created_unix_ns, &changes);
    Ok(SemanticIndexObject {
        schema: 1,
        parser_schema: SEMANTIC_PARSER_SCHEMA,
        symbols,
        parser_failures,
        histories,
    })
}

fn semantic_language(path: &str) -> Option<(&'static str, Language)> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some(("rust", tree_sitter_rust::LANGUAGE.into())),
        "js" | "jsx" | "mjs" | "cjs" => {
            Some(("javascript", tree_sitter_javascript::LANGUAGE.into()))
        }
        "ts" | "mts" | "cts" => Some((
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        )),
        "tsx" => Some(("tsx", tree_sitter_typescript::LANGUAGE_TSX.into())),
        "py" | "pyi" => Some(("python", tree_sitter_python::LANGUAGE.into())),
        "swift" => Some(("swift", tree_sitter_swift::LANGUAGE.into())),
        _ => None,
    }
}

fn parse_symbols(path: &str, source: &[u8]) -> anyhow::Result<(Vec<RepositorySymbol>, bool)> {
    let (language_name, language) =
        semantic_language(path).ok_or_else(|| anyhow::anyhow!("unsupported semantic language"))?;
    let mut parser = Parser::new();
    parser.set_language(&language)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("parser returned no syntax tree"))?;
    let has_errors = tree.root_node().has_error();
    let mut symbols = Vec::new();
    collect_symbols(
        tree.root_node(),
        source,
        path,
        language_name,
        &mut Vec::new(),
        &mut symbols,
    );
    Ok((symbols, has_errors))
}

fn collect_symbols(
    node: Node<'_>,
    source: &[u8],
    path: &str,
    language: &str,
    scopes: &mut Vec<String>,
    output: &mut Vec<RepositorySymbol>,
) {
    let semantic_kind = semantic_node_kind(language, node.kind());
    let name_node = semantic_kind.and_then(|_| semantic_name_node(language, node));
    let mut pushed_scope = false;
    if let (Some(kind), Some(name_node)) = (semantic_kind, name_node)
        && let Ok(name) = name_node.utf8_text(source)
    {
        let name = name.trim();
        if !name.is_empty() {
            let qualified_name = if scopes.is_empty() {
                name.to_string()
            } else {
                format!("{}::{name}", scopes.join("::"))
            };
            let start = node.start_byte();
            let end = node.end_byte();
            if start <= name_node.start_byte() && name_node.end_byte() <= end && end <= source.len()
            {
                let content_hash = blake3::hash(&source[start..end]);
                let mut structural = blake3::Hasher::new();
                structural.update(b"hig-semantic-structure-v1\0");
                structural.update(&source[start..name_node.start_byte()]);
                structural.update(&source[name_node.end_byte()..end]);
                let signature_hash = semantic_signature_hash(node, source, name_node);
                let symbol_id =
                    semantic_symbol_id(language, kind, &qualified_name, &signature_hash);
                output.push(RepositorySymbol {
                    symbol_id,
                    language: language.to_string(),
                    path: path.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    qualified_name: qualified_name.clone(),
                    start_byte: start as u64,
                    end_byte: end as u64,
                    content_hash: content_hash.to_hex().to_string(),
                    structural_hash: structural.finalize().to_hex().to_string(),
                });
                scopes.push(name.to_string());
                pushed_scope = true;
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_symbols(child, source, path, language, scopes, output);
    }
    if pushed_scope {
        scopes.pop();
    }
}

fn semantic_node_kind<'a>(language: &str, kind: &'a str) -> Option<&'a str> {
    match (language, kind) {
        ("rust", "function_item") => Some("function"),
        ("rust", "struct_item") => Some("struct"),
        ("rust", "enum_item") => Some("enum"),
        ("rust", "trait_item") => Some("trait"),
        ("rust", "impl_item") => Some("impl"),
        ("javascript" | "typescript" | "tsx", "function_declaration") => Some("function"),
        ("javascript" | "typescript" | "tsx", "generator_function_declaration") => Some("function"),
        ("javascript" | "typescript" | "tsx", "method_definition") => Some("method"),
        ("javascript" | "typescript" | "tsx", "class_declaration") => Some("class"),
        ("python", "function_definition") => Some("function"),
        ("python", "class_definition") => Some("class"),
        ("swift", "function_declaration") => Some("function"),
        ("swift", "class_declaration") => Some("type"),
        ("swift", "protocol_declaration") => Some("protocol"),
        ("swift", "protocol_function_declaration") => Some("method"),
        _ => None,
    }
}

fn semantic_name_node<'tree>(language: &str, node: Node<'tree>) -> Option<Node<'tree>> {
    node.child_by_field_name("name").or_else(|| {
        if language == "rust" && node.kind() == "impl_item" {
            node.child_by_field_name("type")
        } else {
            None
        }
    })
}

fn semantic_signature_hash(node: Node<'_>, source: &[u8], name_node: Node<'_>) -> [u8; 32] {
    let signature_end = node
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or_else(|| node.end_byte());
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig-semantic-signature-v1\0");
    hasher.update(&source[node.start_byte()..name_node.start_byte()]);
    if name_node.end_byte() < signature_end {
        hasher.update(&source[name_node.end_byte()..signature_end]);
    }
    *hasher.finalize().as_bytes()
}

fn semantic_symbol_id(
    language: &str,
    kind: &str,
    qualified_name: &str,
    signature_hash: &[u8; 32],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig-semantic-symbol-v1\0");
    hasher.update(language.as_bytes());
    hasher.update(&[0]);
    hasher.update(kind.as_bytes());
    hasher.update(&[0]);
    hasher.update(qualified_name.as_bytes());
    hasher.update(&[0]);
    hasher.update(signature_hash);
    hasher.finalize().to_hex().to_string()
}

#[derive(Debug, Clone)]
struct SemanticChange {
    current: Option<RepositorySymbol>,
    previous: Option<RepositorySymbol>,
    kind: RepositorySemanticChangeKind,
}

fn semantic_changes(
    old: &[RepositorySymbol],
    new: &[RepositorySymbol],
    failed_paths: &BTreeSet<String>,
) -> Vec<SemanticChange> {
    let old_by_id = old
        .iter()
        .map(|symbol| (symbol.symbol_id.clone(), symbol))
        .collect::<BTreeMap<_, _>>();
    let new_by_id = new
        .iter()
        .map(|symbol| (symbol.symbol_id.clone(), symbol))
        .collect::<BTreeMap<_, _>>();
    let mut changes = Vec::new();
    let mut matched_old = BTreeSet::new();
    let mut matched_new = BTreeSet::new();
    for (symbol_id, current) in &new_by_id {
        if let Some(previous) = old_by_id.get(symbol_id) {
            matched_old.insert(symbol_id.clone());
            matched_new.insert(symbol_id.clone());
            if previous.path != current.path {
                changes.push(SemanticChange {
                    current: Some((*current).clone()),
                    previous: Some((*previous).clone()),
                    kind: RepositorySemanticChangeKind::Moved,
                });
            } else if previous.content_hash != current.content_hash {
                changes.push(SemanticChange {
                    current: Some((*current).clone()),
                    previous: Some((*previous).clone()),
                    kind: RepositorySemanticChangeKind::Modified,
                });
            }
        }
    }
    for current in new {
        if matched_new.contains(&current.symbol_id) {
            continue;
        }
        let candidates = old
            .iter()
            .filter(|previous| {
                !matched_old.contains(&previous.symbol_id)
                    && previous.language == current.language
                    && previous.kind == current.kind
                    && previous.structural_hash == current.structural_hash
            })
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            let previous = candidates[0];
            matched_old.insert(previous.symbol_id.clone());
            matched_new.insert(current.symbol_id.clone());
            changes.push(SemanticChange {
                current: Some(current.clone()),
                previous: Some(previous.clone()),
                kind: if previous.path == current.path {
                    RepositorySemanticChangeKind::Renamed
                } else {
                    RepositorySemanticChangeKind::Moved
                },
            });
        }
    }
    for current in new {
        if !matched_new.contains(&current.symbol_id) {
            changes.push(SemanticChange {
                current: Some(current.clone()),
                previous: None,
                kind: RepositorySemanticChangeKind::Added,
            });
        }
    }
    for previous in old {
        if !matched_old.contains(&previous.symbol_id) && !failed_paths.contains(&previous.path) {
            changes.push(SemanticChange {
                current: None,
                previous: Some(previous.clone()),
                kind: RepositorySemanticChangeKind::Deleted,
            });
        }
    }
    changes
}

fn build_semantic_histories(
    parent: Option<&SemanticIndexObject>,
    parent_id: Option<RepositoryObjectId>,
    created_unix_ns: i128,
    changes: &[SemanticChange],
) -> BTreeMap<String, Vec<SemanticHistoryRecord>> {
    let mut histories = parent
        .map(|index| index.histories.clone())
        .unwrap_or_default();
    for history in histories.values_mut() {
        for entry in history {
            if entry.commit_id.is_none() {
                entry.commit_id = parent_id;
            }
        }
    }
    for change in changes {
        let identity = change
            .current
            .as_ref()
            .or(change.previous.as_ref())
            .unwrap();
        let previous_id = change
            .previous
            .as_ref()
            .map(|symbol| symbol.symbol_id.clone());
        let current_id = change
            .current
            .as_ref()
            .map(|symbol| symbol.symbol_id.clone());
        let key = current_id
            .clone()
            .unwrap_or_else(|| previous_id.clone().unwrap());
        let prior = previous_id
            .as_ref()
            .and_then(|id| histories.get(id))
            .cloned()
            .unwrap_or_default();
        let record = SemanticHistoryRecord {
            commit_id: None,
            parent_id,
            created_unix_ns,
            symbol_id: key.clone(),
            previous_symbol_id: previous_id.clone().filter(|id| id != &key),
            path: identity.path.clone(),
            previous_path: change.previous.as_ref().map(|symbol| symbol.path.clone()),
            qualified_name: identity.qualified_name.clone(),
            previous_qualified_name: change
                .previous
                .as_ref()
                .map(|symbol| symbol.qualified_name.clone()),
            kind: change.kind,
            start_byte: change.current.as_ref().map(|symbol| symbol.start_byte),
            end_byte: change.current.as_ref().map(|symbol| symbol.end_byte),
        };
        let history = histories.entry(key.clone()).or_default();
        history.insert(0, record.clone());
        if matches!(
            change.kind,
            RepositorySemanticChangeKind::Renamed | RepositorySemanticChangeKind::Moved
        ) {
            history.extend(prior);
            if let Some(previous_id) = previous_id {
                histories.entry(previous_id).or_default().insert(0, record);
            }
        }
    }
    histories
}

fn resolve_symbol<'a>(
    symbols: &'a [RepositorySymbol],
    query: &str,
) -> anyhow::Result<&'a RepositorySymbol> {
    let matches = symbols
        .iter()
        .filter(|symbol| {
            symbol.symbol_id == query
                || symbol.qualified_name == query
                || symbol.name == query
                || symbol.symbol_id.starts_with(query)
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(!matches.is_empty(), "semantic symbol not found");
    anyhow::ensure!(matches.len() == 1, "semantic symbol query is ambiguous");
    Ok(matches[0])
}

fn build_committed_path_history(
    repository: &Repository,
    parent_id: Option<RepositoryObjectId>,
    parent_commit: Option<&CommitObject>,
    created_unix_ns: i128,
    changes: &[RepositoryChange],
) -> anyhow::Result<BTreeMap<String, Vec<PathHistoryRecord>>> {
    let mut paths = match parent_commit.and_then(|commit| commit.change_index) {
        Some(index_id) => {
            repository
                .read::<ChangeIndexObject>(index_id, ObjectKind::ChangeIndex)?
                .path_history
        }
        None => rebuild_legacy_path_history(repository, parent_id)?,
    };
    for history in paths.values_mut() {
        for entry in history {
            if entry.commit_id.is_none() {
                entry.commit_id = parent_id;
            }
        }
    }
    for change in changes {
        let entry = PathHistoryRecord {
            commit_id: None,
            parent_id,
            created_unix_ns,
            path: change.path.clone(),
            previous_path: change.previous_path.clone(),
            kind: change.kind,
            byte_ranges: change.byte_ranges.clone(),
        };
        let prior = change
            .previous_path
            .as_ref()
            .and_then(|path| paths.get(path))
            .cloned()
            .unwrap_or_default();
        let history = paths.entry(change.path.clone()).or_default();
        history.insert(0, entry.clone());
        if change.kind == RepositoryChangeKind::Renamed {
            history.extend(prior);
            if let Some(previous_path) = &change.previous_path {
                paths
                    .entry(previous_path.clone())
                    .or_default()
                    .insert(0, entry);
            }
        }
    }
    Ok(paths)
}

fn rebuild_legacy_path_history(
    repository: &Repository,
    head: Option<RepositoryObjectId>,
) -> anyhow::Result<BTreeMap<String, Vec<PathHistoryRecord>>> {
    let mut chain = Vec::new();
    let mut current = head;
    while let Some(commit_id) = current {
        let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
        current = commit.parent;
        chain.push((commit_id, commit));
    }
    chain.reverse();
    let mut paths = BTreeMap::<String, Vec<PathHistoryRecord>>::new();
    for (commit_id, commit) in chain {
        let old = load_commit_files(repository, commit.parent)?;
        let new = flatten_tree(repository, commit.root_tree)?;
        let changes = build_indexed_changes(repository, &old, &new)?;
        for change in changes {
            let entry = PathHistoryRecord {
                commit_id: Some(commit_id),
                parent_id: commit.parent,
                created_unix_ns: commit.created_unix_ns,
                path: change.path.clone(),
                previous_path: change.previous_path.clone(),
                kind: change.kind,
                byte_ranges: change.byte_ranges,
            };
            paths.entry(change.path).or_default().insert(0, entry);
        }
    }
    Ok(paths)
}

fn reconstruct_file_range(
    repository: &Repository,
    state: &FileState,
    start: u64,
    len: u64,
) -> anyhow::Result<Vec<u8>> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("range length overflow"))?;
    let mut output = Vec::with_capacity(len as usize);
    let mut chunk_start = 0_u64;
    for chunk in &state.object.chunks {
        let chunk_end = chunk_start + chunk.len;
        if chunk_end > start && chunk_start < end {
            let (kind, bytes) = repository.read_raw(chunk.object_id)?;
            anyhow::ensure!(
                kind == ObjectKind::Chunk,
                "file references non-chunk object"
            );
            anyhow::ensure!(bytes.len() as u64 == chunk.len, "chunk length mismatch");
            let local_start = start.saturating_sub(chunk_start) as usize;
            let local_end = (end.min(chunk_end) - chunk_start) as usize;
            output.extend_from_slice(&bytes[local_start..local_end]);
        }
        chunk_start = chunk_end;
        if chunk_start >= end {
            break;
        }
    }
    anyhow::ensure!(
        output.len() as u64 == len,
        "range reconstruction length mismatch"
    );
    Ok(output)
}

fn atomic_replace_file(path: &Path, bytes: &[u8], overwrite: bool) -> anyhow::Result<()> {
    if overwrite {
        return atomic_write(path, bytes);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = unique_sibling(path, "hig-range-stage");
    let result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&temp, path)?;
        fs::remove_file(&temp)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn verify_reachable(
    repository: &Repository,
    object_id: RepositoryObjectId,
    visited: &mut BTreeSet<RepositoryObjectId>,
    report: &mut RepositoryVerifyReport,
) -> anyhow::Result<()> {
    if !visited.insert(object_id) {
        return Ok(());
    }
    let (kind, raw) = repository.read_raw(object_id)?;
    report.checked_objects += 1;
    report.checked_raw_bytes += raw.len() as u64;
    match kind {
        ObjectKind::Chunk => report.chunks += 1,
        ObjectKind::File => {
            report.files += 1;
            let file: FileObject = deserialize_canonical(&raw)?;
            for chunk in file.chunks {
                verify_reachable(repository, chunk.object_id, visited, report)?;
            }
        }
        ObjectKind::Tree => {
            report.trees += 1;
            let tree: TreeObject = deserialize_canonical(&raw)?;
            for entry in tree.entries {
                verify_reachable(repository, entry.object_id, visited, report)?;
            }
        }
        ObjectKind::Commit => {
            report.commits += 1;
            let commit: CommitObject = deserialize_canonical(&raw)?;
            verify_reachable(repository, commit.root_tree, visited, report)?;
            if let Some(parent) = commit.parent {
                verify_reachable(repository, parent, visited, report)?;
            }
            for index in [
                commit.change_index,
                commit.semantic_index,
                commit.compression_tree_index,
            ]
            .into_iter()
            .flatten()
            {
                verify_reachable(repository, index, visited, report)?;
            }
        }
        ObjectKind::ChangeIndex => {
            report.change_indexes += 1;
            let index: ChangeIndexObject = deserialize_canonical(&raw)?;
            anyhow::ensure!(index.schema == 1, "unsupported change-index schema");
            if let Some(parent_index) = index.parent_index {
                verify_reachable(repository, parent_index, visited, report)?;
            }
        }
        ObjectKind::SemanticIndex => {
            report.semantic_indexes += 1;
            let index: SemanticIndexObject = deserialize_canonical(&raw)?;
            anyhow::ensure!(index.schema == 1, "unsupported semantic-index schema");
            anyhow::ensure!(
                (1..=SEMANTIC_PARSER_SCHEMA).contains(&index.parser_schema),
                "unsupported semantic parser schema"
            );
            for symbol in &index.symbols {
                anyhow::ensure!(
                    symbol.start_byte <= symbol.end_byte,
                    "invalid semantic symbol range"
                );
                normalize_requested_path(&symbol.path)?;
            }
        }
        ObjectKind::CompressionTreeIndex => {
            report.compression_tree_indexes += 1;
            let index: CompressionTreeIndexObject = deserialize_canonical(&raw)?;
            anyhow::ensure!(index.schema == 1, "unsupported compression-tree schema");
        }
    }
    Ok(())
}

fn commit_summary(commit_id: RepositoryObjectId, commit: &CommitObject) -> RepositoryCommitSummary {
    RepositoryCommitSummary {
        commit_id,
        parent_id: commit.parent,
        tree_id: commit.root_tree,
        created_unix_ns: commit.created_unix_ns,
        message: commit.message.clone(),
        author: commit.author.clone(),
        files: commit.files,
        input_bytes: commit.input_bytes,
    }
}

fn publish_restored_tree(stage: &Path, output: &Path, overwrite: bool) -> anyhow::Result<()> {
    if !output.exists() {
        fs::rename(stage, output)?;
        return Ok(());
    }
    anyhow::ensure!(overwrite, "restore output already exists");
    let backup = unique_sibling(output, "hig-restore-backup");
    fs::rename(output, &backup)?;
    match fs::rename(stage, output) {
        Ok(()) => {
            remove_path(&backup)?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&backup, output);
            Err(error.into())
        }
    }
}

fn stable_read(path: &Path) -> anyhow::Result<(Vec<u8>, FileFingerprint)> {
    for _ in 0..3 {
        let before = file_fingerprint(&fs::metadata(path)?)?;
        let bytes = fs::read(path)?;
        let after = file_fingerprint(&fs::metadata(path)?)?;
        if before == after && after.size == bytes.len() as u64 {
            return Ok((bytes, after));
        }
    }
    anyhow::bail!(
        "file changed while creating repository snapshot: {}",
        path.display()
    )
}

fn stable_read_source(source: &SourceFilePath) -> anyhow::Result<(Vec<u8>, FileFingerprint)> {
    match source.file_type {
        RepositoryFileType::Regular => stable_read(&source.path),
        RepositoryFileType::Symlink => {
            for _ in 0..3 {
                let before = file_fingerprint(&fs::symlink_metadata(&source.path)?)?;
                let target = fs::read_link(&source.path)?;
                let bytes = symlink_target_bytes(&target)?;
                let after = file_fingerprint(&fs::symlink_metadata(&source.path)?)?;
                if before == after {
                    return Ok((bytes, after));
                }
            }
            anyhow::bail!(
                "symbolic link changed while creating repository snapshot: {}",
                source.path.display()
            )
        }
    }
}

fn symlink_target_bytes(target: &Path) -> anyhow::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(target.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        Ok(target
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("symlink target must be valid UTF-8"))?
            .as_bytes()
            .to_vec())
    }
}

fn create_symlink(target: &[u8], path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        symlink(PathBuf::from(OsString::from_vec(target.to_vec())), path)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        let target = std::str::from_utf8(target)?;
        symlink_file(target, path)?;
    }
    #[cfg(not(any(unix, windows)))]
    anyhow::bail!("symlink restore is not supported on this platform");
    Ok(())
}

fn file_fingerprint(metadata: &fs::Metadata) -> anyhow::Result<FileFingerprint> {
    let modified_ns = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as i128)
        .unwrap_or_else(|error| -(error.duration().as_nanos() as i128));
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let permissions = u32::from(metadata.permissions().readonly());
    Ok(FileFingerprint {
        size: metadata.len(),
        modified_ns,
        permissions,
    })
}

fn set_permissions(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(mode != 0);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn compute_object_id(kind: ObjectKind, raw: &[u8]) -> RepositoryObjectId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OBJECT_HASH_DOMAIN);
    hasher.update(&[kind as u8]);
    hasher.update(&(raw.len() as u64).to_le_bytes());
    hasher.update(raw);
    RepositoryObjectId(*hasher.finalize().as_bytes())
}

fn serialize_canonical<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    Ok(bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .serialize(value)?)
}

fn deserialize_canonical<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> anyhow::Result<T> {
    Ok(bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .with_limit(MAX_OBJECT_RAW_BYTES)
        .deserialize(bytes)?)
}

fn normalize_excludes(excludes: Vec<String>) -> Vec<String> {
    let mut values = DEFAULT_REPOSITORY_EXCLUDES
        .iter()
        .map(|value| (*value).to_string())
        .chain(excludes)
        .collect::<BTreeSet<_>>();
    values.insert(".hig".to_string());
    values.into_iter().collect()
}

fn is_excluded(relative: &Path, excludes: &[String]) -> bool {
    relative.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        excludes.iter().any(|excluded| {
            excluded == value.as_ref() || (excluded == ".venv" && value.starts_with(".venv-"))
        })
    })
}

fn normalize_relative_path(path: &Path) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("repository paths must be valid UTF-8"))?;
                validate_entry_name(value)?;
                parts.push(value);
            }
            _ => anyhow::bail!("invalid repository path"),
        }
    }
    anyhow::ensure!(!parts.is_empty(), "empty repository path");
    Ok(parts.join("/"))
}

fn normalize_requested_path(path: &str) -> anyhow::Result<String> {
    let normalized = normalize_relative_path(Path::new(path))?;
    anyhow::ensure!(
        normalized != ".hig" && !normalized.starts_with(".hig/"),
        "cannot restore repository metadata"
    );
    Ok(normalized)
}

fn validate_entry_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains('/')
            && !name.contains('\\')
            && !name.contains('\0'),
        "unsafe repository tree entry"
    );
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let normalized = normalize_requested_path(relative)?;
    Ok(root.join(normalized))
}

fn validate_ref_component(name: &str, kind: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.starts_with('/')
            && !name.ends_with('/')
            && !name.contains("//")
            && !name.contains('\\')
            && !name.contains('\0')
            && name
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && name.chars().all(
                |value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '/' | '.')
            ),
        "invalid repository {kind} name"
    );
    Ok(())
}

fn read_ref_value(path: &Path) -> anyhow::Result<Option<RepositoryObjectId>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().parse()?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn repository_state_dir(root: &Path) -> PathBuf {
    root.join(".hig").join("repository")
}

fn repository_config_path(root: &Path) -> PathBuf {
    repository_state_dir(root).join("config.json")
}

fn absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn absolute_output_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let absolute = absolute_path(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow::anyhow!("range output has no parent directory"))?;
    let parent = if parent.exists() {
        parent.canonicalize()?
    } else {
        absolute_path(parent)?
    };
    let name = absolute
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("range output has no file name"))?;
    Ok(parent.join(name))
}

fn unique_sibling(path: &Path, kind: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("repository");
    parent.join(format!(
        ".{name}.{kind}.{}.{}",
        std::process::id(),
        hex::encode(crate::random_bytes::<8>())
    ))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = unique_sibling(path, "tmp");
    let result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn atomic_write_object(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = unique_sibling(path, "tmp");
    let result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

fn remove_path(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn now_unix_ns() -> i128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_diff_and_exact_restore_survive_one_byte_change() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"fn value() -> u8 { 1 }\n").unwrap();
        fs::write(root.join("README.md"), b"first\n").unwrap();
        init_repository(&root, Vec::new()).unwrap();

        let first =
            snapshot_repository(&root, "first".to_string(), Some("test".to_string())).unwrap();
        fs::write(root.join("src/lib.rs"), b"fn value() -> u8 { 2 }\n").unwrap();
        let second = snapshot_repository(&root, "second".to_string(), None).unwrap();

        assert_ne!(first.commit_id, second.commit_id);
        let diff = repository_diff(&root, Some(&first.commit_id.to_hex()), Some("HEAD")).unwrap();
        assert_eq!(diff.modified, 1);
        assert_eq!(diff.changes[0].path, "src/lib.rs");

        let restored_first = temp.path().join("restored-first");
        restore_repository(
            &root,
            &first.commit_id.to_hex(),
            &restored_first,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(restored_first.join("src/lib.rs")).unwrap(),
            b"fn value() -> u8 { 1 }\n"
        );

        let restored_second = temp.path().join("restored-second");
        restore_repository(&root, "HEAD", &restored_second, Some("src"), false).unwrap();
        assert_eq!(
            fs::read(restored_second.join("src/lib.rs")).unwrap(),
            b"fn value() -> u8 { 2 }\n"
        );
        assert!(!restored_second.join("README.md").exists());
    }

    #[test]
    fn unchanged_snapshot_does_not_advance_head() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"same").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let second = snapshot_repository(temp.path(), "ignored".to_string(), None).unwrap();
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.commit_id, second.commit_id);
        assert_eq!(repository_log(temp.path(), 10).unwrap().len(), 1);
    }

    #[test]
    fn verify_detects_corrupted_reachable_object() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"content").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        assert!(verify_repository(temp.path()).unwrap().checked_objects >= 4);
        fs::write(repository.object_path(snapshot.commit_id), b"broken").unwrap();
        assert!(verify_repository(temp.path()).is_err());
    }

    #[test]
    fn gc_removes_only_unreachable_objects() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"content").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let (orphan, _, _) = repository.put_raw(ObjectKind::Chunk, b"orphan").unwrap();
        assert!(repository.object_path(orphan).exists());
        let temporary = repository
            .object_path(orphan)
            .parent()
            .unwrap()
            .join(".interrupted.tmp.1234");
        fs::write(&temporary, b"partial").unwrap();

        let dry = gc_repository(temp.path(), true).unwrap();
        assert_eq!(dry.unreachable_objects, 1);
        assert_eq!(dry.temporary_files, 1);
        assert!(repository.object_path(orphan).exists());
        assert!(temporary.exists());
        let actual = gc_repository(temp.path(), false).unwrap();
        assert_eq!(actual.removed_objects, 1);
        assert_eq!(actual.removed_temporary_files, 1);
        assert!(!repository.object_path(orphan).exists());
        assert!(!temporary.exists());
        verify_repository(temp.path()).unwrap();
    }

    #[test]
    fn overwrite_restore_replaces_target_only_after_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let output = temp.path().join("output");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"history").unwrap();
        init_repository(&root, Vec::new()).unwrap();
        snapshot_repository(&root, "first".to_string(), None).unwrap();
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("old.txt"), b"old").unwrap();

        assert!(restore_repository(&root, "HEAD", &output, None, false).is_err());
        assert_eq!(fs::read(output.join("old.txt")).unwrap(), b"old");
        restore_repository(&root, "HEAD", &output, None, true).unwrap();
        assert!(!output.join("old.txt").exists());
        assert_eq!(fs::read(output.join("a.txt")).unwrap(), b"history");
    }

    #[test]
    fn failed_snapshot_does_not_publish_a_new_head() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"first-a").unwrap();
        fs::write(temp.path().join("z.txt"), b"stable-z").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(first.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        let stable_chunk = files["z.txt"].object.chunks[0].object_id;
        fs::write(repository.object_path(stable_chunk), b"corrupted").unwrap();
        fs::write(temp.path().join("a.txt"), b"second-a").unwrap();

        assert!(snapshot_repository(temp.path(), "second".to_string(), None).is_err());
        assert_eq!(repository.read_head().unwrap(), Some(first.commit_id));
    }

    #[test]
    fn empty_directories_are_preserved_by_restore() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("empty/nested")).unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "empty tree".to_string(), None).unwrap();
        let output = temp.path().join("../restored-empty");
        let output = output.parent().unwrap().join(format!(
            "restored-empty-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert!(output.join("empty/nested").is_dir());
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_preserved_by_restore() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("target.txt"), b"target").unwrap();
        symlink("target.txt", temp.path().join("link.txt")).unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "link".to_string(), None).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-symlink-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            fs::read_link(output.join("link.txt")).unwrap(),
            Path::new("target.txt")
        );
        assert_eq!(fs::read(output.join("link.txt")).unwrap(), b"target");
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn one_byte_change_reuses_unaffected_content_defined_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.bin");
        let original = vec![7_u8; PHASE1_CHUNK_BYTES * 3];
        fs::write(&path, &original).unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();

        let mut changed = original.clone();
        changed[PHASE1_CHUNK_BYTES + 17] = 8;
        fs::write(&path, &changed).unwrap();
        let second = snapshot_repository(temp.path(), "one byte".to_string(), None).unwrap();
        assert!(second.chunks_written <= 2);
        assert!(second.chunks_reused >= 10);

        let first_output = temp.path().join("restore-first");
        restore_repository(
            temp.path(),
            &first.commit_id.to_hex(),
            &first_output,
            None,
            false,
        )
        .unwrap();
        let second_output = temp.path().join("restore-second");
        restore_repository(
            temp.path(),
            &second.commit_id.to_hex(),
            &second_output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(fs::read(first_output.join("large.bin")).unwrap(), original);
        assert_eq!(fs::read(second_output.join("large.bin")).unwrap(), changed);
    }

    #[test]
    fn micro_index_and_range_restore_locate_one_changed_byte() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("code.txt"), b"abcdef").unwrap();
        init_repository(&root, Vec::new()).unwrap();
        snapshot_repository(&root, "first".to_string(), None).unwrap();
        fs::write(root.join("code.txt"), b"abcXef").unwrap();
        let second = snapshot_repository(&root, "second".to_string(), None).unwrap();

        let diff = repository_diff(&root, None, Some("HEAD")).unwrap();
        assert_eq!(diff.modified, 1);
        assert_eq!(
            diff.changes[0].byte_ranges,
            vec![RepositoryByteRange {
                old_start: 3,
                old_len: 1,
                new_start: 3,
                new_len: 1,
            }]
        );
        let output = temp.path().join("range.bin");
        let report = restore_repository_range(
            &root,
            &second.commit_id.to_hex(),
            "code.txt",
            2,
            Some(3),
            &output,
            false,
        )
        .unwrap();
        assert_eq!(report.len, 3);
        assert_eq!(fs::read(&output).unwrap(), b"cXe");
        assert!(
            restore_repository_range(&root, "HEAD", "code.txt", 0, None, &output, false).is_err()
        );
    }

    #[test]
    fn fastcdc_reuses_chunks_after_leading_insertion() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("data.bin");
        let mut state = 0x1234_5678_u64;
        let original = (0..(3 * 1024 * 1024))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect::<Vec<_>>();
        fs::write(&path, &original).unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();

        let mut changed = Vec::with_capacity(original.len() + 1);
        changed.push(42);
        changed.extend_from_slice(&original);
        fs::write(&path, &changed).unwrap();
        let second = snapshot_repository(temp.path(), "insert".to_string(), None).unwrap();
        assert!(second.chunks_reused >= first.chunks_written.saturating_sub(3));
        assert!(second.chunks_written <= 3);
    }

    #[test]
    fn rename_history_and_storage_tree_are_indexed() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old.txt"), b"rename me").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        fs::rename(temp.path().join("old.txt"), temp.path().join("new.txt")).unwrap();
        snapshot_repository(temp.path(), "rename".to_string(), None).unwrap();

        let diff = repository_diff(temp.path(), None, Some("HEAD")).unwrap();
        assert_eq!(diff.renamed, 1);
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);
        assert_eq!(diff.changes[0].previous_path.as_deref(), Some("old.txt"));
        let history = repository_path_history(temp.path(), "new.txt", 10).unwrap();
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].kind, RepositoryChangeKind::Renamed);
        assert_eq!(history.entries[1].kind, RepositoryChangeKind::Added);
        let storage = repository_storage_tree(temp.path(), "HEAD").unwrap();
        assert_eq!(storage.files, 1);
        assert_eq!(storage.raw_bytes, 9);
        assert!(storage.stored_object_bytes > 0);
        let verify = verify_repository(temp.path()).unwrap();
        assert_eq!(verify.change_indexes, 2);
        assert_eq!(verify.compression_tree_indexes, 2);
    }

    #[test]
    fn storage_tree_reports_project_and_cache_provenance() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("main.rs"), b"fn main() {}\n").unwrap();
        let config = crate::init_project(temp.path(), None, Vec::new()).unwrap();
        let cache_dir = crate::resolve_project_cache_dir(temp.path(), &config);
        let snapshot = crate::rebuild_snapshot(temp.path(), &cache_dir, &config).unwrap();
        assert_eq!(snapshot.generation, 1);

        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "with provenance".to_string(), None).unwrap();

        let storage = repository_storage_tree(temp.path(), "HEAD").unwrap();
        let provenance = storage
            .cache_provenance
            .expect("project cache provenance should be discoverable");
        assert_eq!(provenance.project_id, config.project_id);
        assert_eq!(provenance.snapshot_generation, Some(1));
        assert_eq!(provenance.cache_generation, Some(0));
        assert_eq!(provenance.cache_index_format.as_deref(), Some("empty"));
        assert_eq!(
            fs::canonicalize(&provenance.cache_dir).unwrap(),
            cache_dir.canonicalize().unwrap()
        );
    }

    #[test]
    fn watcher_debounces_changes_into_an_automatic_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("watched.txt"), b"first").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let mut watcher = RepositoryWatcher::start(temp.path(), Duration::from_millis(50)).unwrap();
        fs::write(temp.path().join("watched.txt"), b"second").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let second = loop {
            if let Some(report) = watcher.poll("automatic", Some("watcher")).unwrap()
                && report.created
            {
                break report;
            }
            assert!(
                Instant::now() < deadline,
                "watcher did not publish a snapshot"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        assert_eq!(second.parent_id, Some(first.commit_id));
        assert_eq!(repository_log(temp.path(), 10).unwrap().len(), 2);
    }

    #[test]
    fn semantic_history_restores_function_and_tracks_rename() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir_all(root.join("src")).unwrap();
        let first_source = b"fn alpha() -> u8 { 1 }\n\nstruct Thing;\nimpl Thing { fn method(&self) -> u8 { 3 } }\n";
        fs::write(root.join("src/lib.rs"), first_source).unwrap();
        init_repository(&root, Vec::new()).unwrap();
        let first = snapshot_repository(&root, "first".to_string(), None).unwrap();

        let second_source = b"fn alpha() -> u8 { 2 }\n\nstruct Thing;\nimpl Thing { fn method(&self) -> u8 { 3 } }\n";
        fs::write(root.join("src/lib.rs"), second_source).unwrap();
        snapshot_repository(&root, "modify".to_string(), None).unwrap();
        let third_source = b"fn beta() -> u8 { 2 }\n\nstruct Thing;\nimpl Thing { fn method(&self) -> u8 { 3 } }\n";
        fs::write(root.join("src/lib.rs"), third_source).unwrap();
        snapshot_repository(&root, "rename".to_string(), None).unwrap();

        let symbols = repository_symbols(&root, "HEAD", Some("src/lib.rs")).unwrap();
        assert!(
            symbols
                .symbols
                .iter()
                .any(|symbol| symbol.qualified_name == "beta")
        );
        assert!(
            symbols
                .symbols
                .iter()
                .any(|symbol| symbol.qualified_name == "Thing::method")
        );
        let history = repository_symbol_history(&root, "beta", 10).unwrap();
        assert_eq!(history.entries.len(), 3);
        assert_eq!(
            history.entries[0].kind,
            RepositorySemanticChangeKind::Renamed
        );
        assert_eq!(
            history.entries[1].kind,
            RepositorySemanticChangeKind::Modified
        );
        assert_eq!(history.entries[2].kind, RepositorySemanticChangeKind::Added);

        let output = temp.path().join("alpha.rs");
        let restored =
            restore_repository_symbol(&root, &first.commit_id.to_hex(), "alpha", &output, false)
                .unwrap();
        assert_eq!(restored.qualified_name, "alpha");
        assert_eq!(fs::read(output).unwrap(), b"fn alpha() -> u8 { 1 }");
        let verify = verify_repository(&root).unwrap();
        assert_eq!(verify.semantic_indexes, 3);
    }

    #[test]
    fn semantic_language_adapters_find_functions_classes_and_methods() {
        let cases: &[(&str, &[u8], &[&str])] = &[
            (
                "app.js",
                b"function run() { return 1; } class Box { get() { return 2; } }",
                &["run", "Box", "Box::get"],
            ),
            (
                "app.ts",
                b"function typed(value: number): number { return value; }",
                &["typed"],
            ),
            (
                "app.py",
                b"class Box:\n    def get(self):\n        return 2\n",
                &["Box", "Box::get"],
            ),
            (
                "App.swift",
                b"struct Box { func get() -> Int { 2 } }\nfunc run() { }\n",
                &["Box", "Box::get", "run"],
            ),
        ];
        for (path, source, expected) in cases {
            let (symbols, _) = parse_symbols(path, source).unwrap();
            for qualified_name in *expected {
                assert!(
                    symbols
                        .iter()
                        .any(|symbol| symbol.qualified_name == *qualified_name),
                    "{path} did not contain {qualified_name}"
                );
            }
        }

        let (overloads, _) = parse_symbols(
            "Overloads.swift",
            b"struct Box { func value(_ input: Int) -> Int { input } func value(_ input: String) -> String { input } }",
        )
        .unwrap();
        let overload_ids = overloads
            .iter()
            .filter(|symbol| symbol.qualified_name == "Box::value")
            .map(|symbol| symbol.symbol_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(overload_ids.len(), 2);
    }

    #[test]
    fn parser_failure_preserves_byte_history_without_false_symbol_deletion() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("lib.rs"), b"fn alpha() { }\n").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "valid".to_string(), None).unwrap();
        let first_symbols = repository_symbols(temp.path(), "HEAD", None).unwrap();
        let alpha_id = first_symbols.symbols[0].symbol_id.clone();

        fs::write(temp.path().join("lib.rs"), b"fn alpha(\n").unwrap();
        let broken = snapshot_repository(temp.path(), "broken source".to_string(), None).unwrap();
        assert!(broken.created);
        let symbols = repository_symbols(temp.path(), "HEAD", None).unwrap();
        assert!(symbols.symbols.is_empty());
        assert_eq!(symbols.parser_failures.len(), 1);
        let history = repository_symbol_history(temp.path(), &alpha_id, 10).unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].kind, RepositorySemanticChangeKind::Added);

        let output = temp.path().join("../broken-restore");
        let output = output.parent().unwrap().join(format!(
            "broken-restore-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(fs::read(output.join("lib.rs")).unwrap(), b"fn alpha(\n");
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn repository_defaults_exclude_virtual_environments_and_build_trees() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".venv-project")).unwrap();
        fs::create_dir_all(temp.path().join(".build/cache")).unwrap();
        fs::write(
            temp.path().join(".venv-project/dependency.py"),
            b"def dep(): pass\n",
        )
        .unwrap();
        fs::write(temp.path().join(".build/cache/object.bin"), b"build").unwrap();
        fs::write(temp.path().join("app.py"), b"def app(): pass\n").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot = snapshot_repository(temp.path(), "source only".to_string(), None).unwrap();
        assert_eq!(snapshot.files, 1);
        let symbols = repository_symbols(temp.path(), "HEAD", None).unwrap();
        assert_eq!(symbols.symbols.len(), 1);
        assert_eq!(symbols.symbols[0].qualified_name, "app");
    }

    #[test]
    fn semantic_schema_upgrade_creates_index_commit_without_rewriting_content() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("lib.rs"), b"fn value() -> u8 { 1 }\n").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let mut legacy_commit: CommitObject = repository
            .read(first.commit_id, ObjectKind::Commit)
            .unwrap();
        let mut legacy_index = read_semantic_index(&repository, &legacy_commit).unwrap();
        legacy_index.parser_schema = SEMANTIC_PARSER_SCHEMA - 1;
        let (legacy_index_id, _, _) = repository
            .put(ObjectKind::SemanticIndex, &legacy_index)
            .unwrap();
        legacy_commit.semantic_index = Some(legacy_index_id);
        let (legacy_commit_id, _, _) = repository.put(ObjectKind::Commit, &legacy_commit).unwrap();
        fs::remove_file(repository.state.join("HEAD")).unwrap();
        repository
            .update_ref_path(
                &repository.state.join("refs").join("HEAD"),
                legacy_commit_id,
            )
            .unwrap();

        let upgraded =
            snapshot_repository(temp.path(), "parser upgrade".to_string(), None).unwrap();
        assert!(upgraded.created);
        assert_eq!(upgraded.tree_id, first.tree_id);
        assert_eq!(upgraded.chunks_written, 0);
        let commit: CommitObject = repository
            .read(upgraded.commit_id, ObjectKind::Commit)
            .unwrap();
        let index = read_semantic_index(&repository, &commit).unwrap();
        assert_eq!(index.parser_schema, SEMANTIC_PARSER_SCHEMA);
    }

    #[test]
    fn new_repositories_use_main_branch_and_keep_legacy_head_view() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"first").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(repository.state.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        assert_eq!(
            fs::read_to_string(repository.state.join("refs").join("HEAD")).unwrap(),
            format!("{}\n", first.commit_id)
        );
        let refs = repository_refs(temp.path()).unwrap();
        assert_eq!(refs.active_branch.as_deref(), Some("main"));
        assert!(refs.refs.iter().any(|reference| {
            reference.kind == RepositoryRefKind::Branch
                && reference.name == "main"
                && reference.active
                && reference.commit_id == first.commit_id
        }));
    }

    #[test]
    fn branches_tags_and_revision_aliases_are_atomic_and_immutable() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"first").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();

        let feature = create_repository_branch(temp.path(), "feature/x", None).unwrap();
        assert_eq!(feature.commit_id, first.commit_id);
        assert_eq!(
            Repository::discover(temp.path())
                .unwrap()
                .resolve_revision("feature/x")
                .unwrap(),
            first.commit_id
        );
        switch_repository_branch(temp.path(), "feature/x").unwrap();
        fs::write(temp.path().join("a.txt"), b"feature").unwrap();
        let feature_commit = snapshot_repository(temp.path(), "feature".to_string(), None).unwrap();
        assert_eq!(feature_commit.parent_id, Some(first.commit_id));
        assert_eq!(
            repository_refs(temp.path())
                .unwrap()
                .active_branch
                .as_deref(),
            Some("feature/x")
        );

        switch_repository_branch(temp.path(), "main").unwrap();
        assert_eq!(
            Repository::discover(temp.path())
                .unwrap()
                .resolve_revision("refs/heads/main")
                .unwrap(),
            first.commit_id
        );
        let tag = create_repository_tag(temp.path(), "v1.0.0", Some("main")).unwrap();
        assert_eq!(tag.commit_id, first.commit_id);
        assert_eq!(
            Repository::discover(temp.path())
                .unwrap()
                .resolve_revision("tags/v1.0.0")
                .unwrap(),
            first.commit_id
        );
        assert!(create_repository_tag(temp.path(), "v1.0.0", None).is_err());
        assert!(delete_repository_branch(temp.path(), "main").is_err());
        delete_repository_branch(temp.path(), "feature/x").unwrap();
        delete_repository_tag(temp.path(), "v1.0.0").unwrap();
        assert!(
            repository_refs(temp.path())
                .unwrap()
                .refs
                .iter()
                .all(|reference| { reference.name != "feature/x" && reference.name != "v1.0.0" })
        );
    }

    #[test]
    fn legacy_direct_head_repositories_continue_to_snapshot_and_resolve() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"legacy").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        fs::remove_file(repository.state.join("HEAD")).unwrap();
        fs::remove_dir_all(repository.state.join("refs").join("heads")).unwrap();
        fs::write(temp.path().join("a.txt"), b"legacy second").unwrap();
        let second = snapshot_repository(temp.path(), "second".to_string(), None).unwrap();
        assert_eq!(second.parent_id, Some(first.commit_id));
        assert_eq!(
            Repository::discover(temp.path())
                .unwrap()
                .resolve_revision("HEAD")
                .unwrap(),
            second.commit_id
        );
        assert_eq!(repository_refs(temp.path()).unwrap().active_branch, None);
        assert!(
            repository_refs(temp.path())
                .unwrap()
                .refs
                .iter()
                .any(|reference| { reference.kind == RepositoryRefKind::LegacyHead })
        );
    }

    #[test]
    fn legacy_repository_migration_preserves_objects_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"legacy").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let before = repository
            .list_objects()
            .unwrap()
            .into_iter()
            .map(|(object_id, _, _)| object_id)
            .collect::<BTreeSet<_>>();
        fs::remove_file(repository.state.join("HEAD")).unwrap();
        fs::remove_dir_all(repository.state.join("refs").join("heads")).unwrap();

        let report = migrate_repository(temp.path()).unwrap();
        assert!(report.from_legacy);
        assert!(report.changed);
        assert_eq!(report.active_branch, "main");
        assert_eq!(report.commit_id, Some(first.commit_id));
        assert_eq!(report.objects_rewritten, 0);
        let migrated = Repository::discover(temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(migrated.state.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        assert_eq!(migrated.read_branch("main").unwrap(), Some(first.commit_id));
        let after = migrated
            .list_objects()
            .unwrap()
            .into_iter()
            .map(|(object_id, _, _)| object_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(before, after);

        let repeated = migrate_repository(temp.path()).unwrap();
        assert!(!repeated.from_legacy);
        assert!(!repeated.changed);
        assert_eq!(repeated.commit_id, Some(first.commit_id));
        assert_eq!(
            Repository::discover(temp.path())
                .unwrap()
                .list_objects()
                .unwrap()
                .into_iter()
                .map(|(object_id, _, _)| object_id)
                .collect::<BTreeSet<_>>(),
            before
        );
    }

    #[test]
    fn legacy_repository_migration_rejects_conflicting_main_branch() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"first").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();
        fs::write(temp.path().join("a.txt"), b"second").unwrap();
        let second = snapshot_repository(temp.path(), "second".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        fs::remove_file(repository.state.join("HEAD")).unwrap();
        fs::remove_dir_all(repository.state.join("refs").join("heads")).unwrap();
        atomic_write(
            &repository.state.join("refs").join("HEAD"),
            format!("{}\n", first.commit_id).as_bytes(),
        )
        .unwrap();
        atomic_write(
            &repository.state.join("refs").join("heads").join("main"),
            format!("{}\n", second.commit_id).as_bytes(),
        )
        .unwrap();

        assert!(migrate_repository(temp.path()).is_err());
        assert!(!repository.state.join("HEAD").exists());
        assert_eq!(
            read_ref_value(&repository.state.join("refs").join("HEAD")).unwrap(),
            Some(first.commit_id)
        );
    }
}
