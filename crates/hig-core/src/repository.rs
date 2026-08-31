use anyhow::Context;
use bincode::Options;
use fastcdc::v2020::FastCDC;
use fs2::FileExt;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
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
const MAX_EXTENDED_ATTRIBUTES: usize = 1024;
const MAX_EXTENDED_ATTRIBUTE_NAME_BYTES: usize = 1024;
const MAX_EXTENDED_ATTRIBUTE_VALUE_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXTENDED_ATTRIBUTES_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_ACCESS_CONTROL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ALTERNATE_DATA_STREAMS: usize = 1024;
const MAX_ALTERNATE_DATA_STREAM_NAME_UNITS: usize = 296;
const REPOSITORY_WATCH_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(60);

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
    #[serde(default)]
    pub total_bytes: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RepositoryReplicationReport {
    pub repository_id: [u8; 16],
    pub commit_id: RepositoryObjectId,
    pub ref_name: String,
    pub reachable_objects: u64,
    pub objects_written: u64,
    pub objects_repaired: u64,
    pub object_bytes_written: u64,
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
struct FileObjectV1 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObjectV2 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObjectV3 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    allocated_extents: Vec<FileExtent>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObjectV4 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    allocated_extents: Option<Vec<FileExtent>>,
    extended_attributes: Vec<ExtendedAttribute>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObjectV5 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    allocated_extents: Option<Vec<FileExtent>>,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObjectV6 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    allocated_extents: Option<Vec<FileExtent>>,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileObjectV7 {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    allocated_extents: Option<Vec<FileExtent>>,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    alternate_data_streams: Vec<AlternateDataStream>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileObject {
    schema: u16,
    file_type: RepositoryFileType,
    size: u64,
    permissions: u32,
    mtime_ns: i128,
    content_hash: [u8; 32],
    chunking_schema: u16,
    hardlink_id: Option<[u8; 32]>,
    allocated_extents: Option<Vec<FileExtent>>,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    alternate_data_streams: Vec<AlternateDataStream>,
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct FileExtent {
    offset: u64,
    len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ExtendedAttribute {
    name: Vec<u8>,
    value: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum AccessControlMetadata {
    AppleExtended {
        text: Vec<u8>,
    },
    LinuxPosix {
        access: Option<Vec<u8>>,
        default: Option<Vec<u8>>,
    },
    WindowsSecurityDescriptor {
        sddl: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct OwnershipMetadata {
    user_id: u32,
    group_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AlternateDataStream {
    name: Vec<u16>,
    size: u64,
    content_hash: [u8; 32],
    chunks: Vec<ChunkReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedAlternateDataStream {
    name: Vec<u16>,
    content_hash: [u8; 32],
    content: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum RepositoryFileType {
    Regular,
    Symlink,
    SymlinkDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessControlNodeKind {
    Regular,
    Directory,
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
struct TreeObjectV1 {
    schema: u16,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeObjectV2 {
    schema: u16,
    permissions: u32,
    mtime_ns: i128,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeObjectV3 {
    schema: u16,
    permissions: u32,
    mtime_ns: i128,
    extended_attributes: Vec<ExtendedAttribute>,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeObjectV4 {
    schema: u16,
    permissions: u32,
    mtime_ns: i128,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeObjectV5 {
    schema: u16,
    permissions: u32,
    mtime_ns: i128,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeObjectV6 {
    schema: u16,
    permissions: u32,
    mtime_ns: i128,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    alternate_data_streams: Vec<AlternateDataStream>,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeObject {
    metadata: Option<DirectoryMetadata>,
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DirectoryMetadata {
    permissions: u32,
    mtime_ns: i128,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    alternate_data_streams: Vec<AlternateDataStream>,
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
    metadata: Option<DirectoryMetadata>,
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
    directories: Vec<SourceDirectoryPath>,
    files: Vec<SourceFilePath>,
}

#[derive(Debug)]
struct SourceDirectoryPath {
    path: PathBuf,
    relative: String,
    metadata: CapturedDirectoryMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedDirectoryMetadata {
    permissions: u32,
    mtime_ns: i128,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    ownership: Option<OwnershipMetadata>,
    alternate_data_streams: Vec<CapturedAlternateDataStream>,
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

#[derive(Debug, Clone)]
struct DirectoryState {
    path: String,
    metadata: Option<DirectoryMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    size: u64,
    modified_ns: i128,
    permissions: u32,
    ownership: Option<OwnershipMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceFileIdentity {
    volume: u64,
    file: u64,
    links: u64,
}

#[derive(Debug)]
struct StableSourceRead {
    logical_size: u64,
    content_hash: [u8; 32],
    data_extents: Vec<StableDataExtent>,
    allocated_extents: Option<Vec<FileExtent>>,
    extended_attributes: Vec<ExtendedAttribute>,
    access_control: Option<AccessControlMetadata>,
    alternate_data_streams: Vec<CapturedAlternateDataStream>,
    fingerprint: FileFingerprint,
    hardlink_id: Option<[u8; 32]>,
}

#[derive(Debug)]
struct StableDataExtent {
    offset: u64,
    content: Vec<u8>,
}

pub struct RepositoryWatcher {
    root: PathBuf,
    debounce: Duration,
    reconciliation_interval: Duration,
    receiver: Receiver<notify::Result<Event>>,
    _watcher: RecommendedWatcher,
    pending: bool,
    last_event: Option<Instant>,
    last_snapshot: Instant,
}

impl RepositoryWatcher {
    pub fn start(start: &Path, debounce: Duration) -> anyhow::Result<Self> {
        Self::start_with_reconciliation_interval(
            start,
            debounce,
            REPOSITORY_WATCH_RECONCILIATION_INTERVAL,
        )
    }

    fn start_with_reconciliation_interval(
        start: &Path,
        debounce: Duration,
        reconciliation_interval: Duration,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !debounce.is_zero(),
            "watch debounce must be greater than zero"
        );
        anyhow::ensure!(
            !reconciliation_interval.is_zero(),
            "watch reconciliation interval must be greater than zero"
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
            reconciliation_interval,
            receiver,
            _watcher: watcher,
            pending: false,
            last_event: None,
            last_snapshot: Instant::now(),
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
            return self.snapshot(message, author);
        }
        if self.last_snapshot.elapsed() >= self.reconciliation_interval {
            return self.snapshot(message, author);
        }
        Ok(None)
    }

    fn snapshot(
        &mut self,
        message: &str,
        author: Option<&str>,
    ) -> anyhow::Result<Option<RepositorySnapshotReport>> {
        let report =
            snapshot_repository(&self.root, message.to_string(), author.map(str::to_string));
        self.last_snapshot = Instant::now();
        report.map(Some)
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

    let source_paths = repository
        .source_tree_paths()
        .context("failed to enumerate repository source tree")?;
    for source in source_paths.files {
        let relative = source.path.strip_prefix(&repository.root)?;
        let relative = normalize_relative_path(relative)?;
        let stable = stable_read_source(&source).with_context(|| {
            format!(
                "failed to capture source metadata: {}",
                source.path.display()
            )
        })?;
        let mut chunks = Vec::new();
        for extent in &stable.data_extents {
            chunks.extend(store_content_chunks(
                &repository,
                &extent.content,
                &mut stats,
            )?);
        }
        let stream_bytes = stable
            .alternate_data_streams
            .iter()
            .map(|stream| stream.content.len() as u64)
            .sum::<u64>();
        let alternate_data_streams =
            store_alternate_data_streams(&repository, stable.alternate_data_streams, &mut stats)?;
        let (file_id, written, stored_bytes) = repository.put(
            ObjectKind::File,
            &FileObjectV7 {
                schema: 7,
                file_type: source.file_type,
                size: stable.logical_size,
                permissions: stable.fingerprint.permissions,
                mtime_ns: stable.fingerprint.modified_ns,
                content_hash: stable.content_hash,
                chunking_schema: repository.config.chunking.schema,
                hardlink_id: stable.hardlink_id,
                allocated_extents: stable.allocated_extents,
                extended_attributes: stable.extended_attributes,
                access_control: stable.access_control,
                ownership: stable.fingerprint.ownership,
                alternate_data_streams,
                chunks,
            },
        )?;
        if written {
            stats.objects_written += 1;
            stats.object_bytes_written += stored_bytes;
            stats.new_objects.push(file_id);
        }
        insert_file(&mut root_node, &relative, file_id)?;
        files += 1;
        input_bytes = input_bytes
            .checked_add(stable.logical_size)
            .and_then(|total| total.checked_add(stream_bytes))
            .ok_or_else(|| anyhow::anyhow!("repository input byte count overflows"))?;
    }

    for directory in source_paths.directories {
        let current =
            capture_directory_metadata(&directory.path, &fs::symlink_metadata(&directory.path)?)
                .with_context(|| {
                    format!(
                        "failed to capture directory metadata: {}",
                        directory.path.display()
                    )
                })?;
        anyhow::ensure!(
            current == directory.metadata,
            "directory changed while creating repository snapshot: {}",
            directory.path.display()
        );
        let stream_bytes = current
            .alternate_data_streams
            .iter()
            .map(|stream| stream.content.len() as u64)
            .sum::<u64>();
        let metadata = persist_directory_metadata(&repository, current, &mut stats)?;
        input_bytes = input_bytes
            .checked_add(stream_bytes)
            .ok_or_else(|| anyhow::anyhow!("repository input byte count overflows"))?;
        insert_directory(&mut root_node, &directory.relative, metadata)?;
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
        let mut restored_hardlinks = BTreeMap::<[u8; 32], (PathBuf, FileObject)>::new();
        for directory in &directories {
            if directory.path.is_empty() {
                continue;
            }
            if let Some(selected) = &selected
                && directory.path != *selected
                && !directory.path.starts_with(&format!("{selected}/"))
            {
                continue;
            }
            if selected
                .as_ref()
                .is_some_and(|selected| selected == &directory.path)
            {
                selected_directory = true;
            }
            fs::create_dir_all(safe_join(&stage, &directory.path)?)?;
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
            if let Some(hardlink_id) = state.object.hardlink_id {
                if let Some((existing, existing_object)) = restored_hardlinks.get(&hardlink_id) {
                    anyhow::ensure!(
                        existing_object == &state.object,
                        "hardlink group contains inconsistent file objects"
                    );
                    fs::hard_link(existing, &destination)?;
                    verify_hardlink_pair(existing, &destination)?;
                } else {
                    restore_file(&repository, state, &destination)?;
                    restored_hardlinks
                        .insert(hardlink_id, (destination.clone(), state.object.clone()));
                }
            } else {
                restore_file(&repository, state, &destination)?;
            }
            restored_files += 1;
            restored_bytes += state.object.size;
        }
        for directory in directories.iter().rev() {
            let should_restore = if directory.path.is_empty() {
                selected.is_none()
            } else {
                selected.as_ref().is_none_or(|selected| {
                    directory.path == *selected
                        || directory.path.starts_with(&format!("{selected}/"))
                })
            };
            if !should_restore {
                continue;
            }
            if let Some(metadata) = &directory.metadata {
                let destination = if directory.path.is_empty() {
                    stage.clone()
                } else {
                    safe_join(&stage, &directory.path)?
                };
                set_directory_metadata(&repository, &destination, metadata)?;
            }
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
    let mut validated_commits = BTreeSet::new();
    for object_id in refs.values() {
        validate_commit_hardlink_groups(&repository, *object_id, &mut validated_commits)?;
    }
    Ok(report)
}

pub(crate) fn repository_root_and_config(
    start: &Path,
) -> anyhow::Result<(PathBuf, RepositoryConfig)> {
    let repository = Repository::discover(start)?;
    Ok((repository.root, repository.config))
}

pub(crate) fn repository_revision_id(
    start: &Path,
    revision: &str,
) -> anyhow::Result<RepositoryObjectId> {
    Repository::discover(start)?.resolve_revision(revision)
}

pub(crate) fn replicate_repository_revision(
    source_start: &Path,
    revision: &str,
    destination_root: &Path,
    recovery_ref: &str,
) -> anyhow::Result<RepositoryReplicationReport> {
    replicate_or_repair_repository_revision(
        source_start,
        revision,
        destination_root,
        recovery_ref,
        false,
    )
}

pub(crate) fn repair_repository_revision(
    source_start: &Path,
    revision: &str,
    destination_root: &Path,
    recovery_ref: &str,
) -> anyhow::Result<RepositoryReplicationReport> {
    replicate_or_repair_repository_revision(
        source_start,
        revision,
        destination_root,
        recovery_ref,
        true,
    )
}

fn replicate_or_repair_repository_revision(
    source_start: &Path,
    revision: &str,
    destination_root: &Path,
    recovery_ref: &str,
    repair_corrupt: bool,
) -> anyhow::Result<RepositoryReplicationReport> {
    validate_ref_component(recovery_ref, "recovery")?;
    let source = Repository::discover(source_start)?;
    let _source_lock = source.lock_writer()?;
    let commit_id = source.resolve_revision(revision)?;
    source.ensure_commit(commit_id)?;

    let mut source_verify = RepositoryVerifyReport::default();
    let mut reachable = BTreeSet::new();
    verify_reachable(&source, commit_id, &mut reachable, &mut source_verify)?;
    validate_commit_hardlink_groups(&source, commit_id, &mut BTreeSet::new())?;

    let destination_state = repository_state_dir(destination_root);
    let destination_config_path = destination_state.join("config.json");
    fs::create_dir_all(destination_state.join("objects"))?;
    fs::create_dir_all(destination_state.join("refs").join("heads"))?;
    fs::create_dir_all(destination_state.join("refs").join("tags"))?;
    fs::create_dir_all(destination_state.join("locks"))?;
    if destination_config_path.exists() {
        let existing: RepositoryConfig =
            serde_json::from_slice(&fs::read(&destination_config_path)?)?;
        anyhow::ensure!(
            existing.repository_id == source.config.repository_id,
            "recovery destination repository identity mismatch"
        );
        anyhow::ensure!(
            existing.schema == source.config.schema,
            "recovery destination repository schema mismatch"
        );
    } else {
        atomic_write(
            &destination_config_path,
            &serde_json::to_vec_pretty(&source.config)?,
        )?;
        atomic_write(&destination_state.join("HEAD"), b"ref: refs/heads/main\n")?;
        sync_directory(&destination_state)?;
    }

    let destination = Repository::open_exact(destination_root)?;
    let _destination_lock = destination.lock_writer()?;
    let mut new_objects = Vec::new();
    let mut objects_repaired = 0_u64;
    let mut object_bytes_written = 0_u64;
    for object_id in &reachable {
        let (kind, raw) = source.read_raw(*object_id)?;
        if repair_corrupt && destination.object_path(*object_id).exists() {
            match destination.read_raw(*object_id) {
                Ok((existing_kind, existing_raw)) => {
                    anyhow::ensure!(
                        existing_kind == kind && existing_raw == raw,
                        "recovery object collision during repair"
                    );
                    continue;
                }
                Err(_) => {
                    destination.quarantine_object(*object_id)?;
                    objects_repaired = objects_repaired.saturating_add(1);
                }
            }
        }
        let (written_id, written, stored_bytes) = destination.put_raw(kind, &raw)?;
        anyhow::ensure!(written_id == *object_id, "replicated object id mismatch");
        if written {
            new_objects.push(*object_id);
            object_bytes_written = object_bytes_written.saturating_add(stored_bytes);
        }
    }
    destination.sync_new_objects(&new_objects)?;

    let mut destination_verify = RepositoryVerifyReport::default();
    let mut destination_reachable = BTreeSet::new();
    verify_reachable(
        &destination,
        commit_id,
        &mut destination_reachable,
        &mut destination_verify,
    )?;
    anyhow::ensure!(
        destination_reachable == reachable,
        "recovery destination reachable graph mismatch"
    );

    let ref_name = format!("recovery/{recovery_ref}");
    destination.update_ref_path(&destination.tag_path(&ref_name), commit_id)?;
    Ok(RepositoryReplicationReport {
        repository_id: source.config.repository_id,
        commit_id,
        ref_name: format!("tags/{ref_name}"),
        reachable_objects: reachable.len() as u64,
        objects_written: new_objects.len() as u64,
        objects_repaired,
        object_bytes_written,
    })
}

pub fn gc_repository(start: &Path, dry_run: bool) -> anyhow::Result<RepositoryGcReport> {
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    gc_repository_locked(&repository, dry_run, &BTreeSet::new())
}

pub(crate) fn gc_repository_excluding_recovery_refs(
    start: &Path,
    recovery_point_ids: &BTreeSet<String>,
    dry_run: bool,
) -> anyhow::Result<RepositoryGcReport> {
    for point_id in recovery_point_ids {
        validate_ref_component(point_id, "recovery point")?;
    }
    let repository = Repository::discover(start)?;
    let _lock = repository.lock_writer()?;
    if !dry_run {
        for point_id in recovery_point_ids {
            let path = repository.tag_path(&format!("recovery/{point_id}"));
            if path.exists() {
                repository.delete_ref_file(&path)?;
            }
        }
    }
    let excluded = recovery_point_ids
        .iter()
        .map(|point_id| format!("tags/recovery/{point_id}"))
        .collect();
    gc_repository_locked(&repository, dry_run, &excluded)
}

fn gc_repository_locked(
    repository: &Repository,
    dry_run: bool,
    excluded_refs: &BTreeSet<String>,
) -> anyhow::Result<RepositoryGcReport> {
    let mut refs = repository.read_refs()?;
    refs.retain(|name, _| !excluded_refs.contains(name));
    let mut reachable = BTreeSet::new();
    let mut verify = RepositoryVerifyReport::default();
    for object_id in refs.values() {
        anyhow::ensure!(
            repository.read_raw(*object_id)?.0 == ObjectKind::Commit,
            "repository ref does not point to a commit"
        );
        verify_reachable(repository, *object_id, &mut reachable, &mut verify)?;
    }
    let objects = repository.list_objects()?;
    let total_bytes = objects.iter().map(|(_, _, bytes)| *bytes).sum();
    let mut report = RepositoryGcReport {
        dry_run,
        total_objects: objects.len() as u64,
        total_bytes,
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
            let entry = entry.context("failed to walk repository source entry")?;
            if entry.file_type().is_dir() {
                let relative = if entry.path() == self.root {
                    String::new()
                } else {
                    normalize_relative_path(entry.path().strip_prefix(&self.root)?)?
                };
                paths.directories.push(SourceDirectoryPath {
                    path: entry.path().to_path_buf(),
                    relative,
                    metadata: capture_directory_metadata(
                        entry.path(),
                        &entry.metadata().with_context(|| {
                            format!(
                                "failed to stat source directory: {}",
                                entry.path().display()
                            )
                        })?,
                    )
                    .with_context(|| {
                        format!(
                            "failed to read source directory metadata: {}",
                            entry.path().display()
                        )
                    })?,
                });
            } else if entry.file_type().is_file() {
                paths.files.push(SourceFilePath {
                    path: entry.into_path(),
                    file_type: RepositoryFileType::Regular,
                });
            } else if entry.file_type().is_symlink() {
                paths.files.push(SourceFilePath {
                    file_type: repository_symlink_type(entry.path())?,
                    path: entry.into_path(),
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
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(self.object_path(*object_id))?
                .sync_all()?;
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

    fn quarantine_object(&self, object_id: RepositoryObjectId) -> anyhow::Result<PathBuf> {
        let source = self.object_path(object_id);
        anyhow::ensure!(
            source.exists(),
            "repository object is missing during quarantine"
        );
        let quarantine = self.state.join("quarantine").join(format!(
            "{}.{}.{}",
            object_id,
            std::process::id(),
            hex::encode(crate::random_bytes::<8>())
        ));
        fs::create_dir_all(
            quarantine
                .parent()
                .ok_or_else(|| anyhow::anyhow!("invalid repository quarantine path"))?,
        )?;
        fs::rename(&source, &quarantine)?;
        if let Some(parent) = source.parent() {
            sync_directory(parent)?;
        }
        sync_directory(
            quarantine
                .parent()
                .ok_or_else(|| anyhow::anyhow!("invalid repository quarantine path"))?,
        )?;
        Ok(quarantine)
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
            let name = normalize_relative_path(entry.path().strip_prefix(&root)?)?;
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
                let name = normalize_relative_path(entry.path().strip_prefix(&directory)?)?;
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
    let metadata = node
        .metadata
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("directory metadata is missing while writing tree"))?;
    let tree = TreeObjectV6 {
        schema: 6,
        permissions: metadata.permissions,
        mtime_ns: metadata.mtime_ns,
        extended_attributes: metadata.extended_attributes.clone(),
        access_control: metadata.access_control.clone(),
        ownership: metadata.ownership,
        alternate_data_streams: metadata.alternate_data_streams.clone(),
        entries,
    };
    let (object_id, written, stored_bytes) = repository.put(ObjectKind::Tree, &tree)?;
    if written {
        stats.objects_written += 1;
        stats.object_bytes_written += stored_bytes;
        stats.new_objects.push(object_id);
    }
    Ok(object_id)
}

fn persist_directory_metadata(
    repository: &Repository,
    captured: CapturedDirectoryMetadata,
    stats: &mut StoreStats,
) -> anyhow::Result<DirectoryMetadata> {
    Ok(DirectoryMetadata {
        permissions: captured.permissions,
        mtime_ns: captured.mtime_ns,
        extended_attributes: captured.extended_attributes,
        access_control: captured.access_control,
        ownership: captured.ownership,
        alternate_data_streams: store_alternate_data_streams(
            repository,
            captured.alternate_data_streams,
            stats,
        )?,
    })
}

fn store_alternate_data_streams(
    repository: &Repository,
    captured: Vec<CapturedAlternateDataStream>,
    stats: &mut StoreStats,
) -> anyhow::Result<Vec<AlternateDataStream>> {
    validate_captured_alternate_data_streams(&captured)?;
    captured
        .into_iter()
        .map(|stream| {
            let size = stream.content.len() as u64;
            let chunks = store_content_chunks(repository, &stream.content, stats)?;
            Ok(AlternateDataStream {
                name: stream.name,
                size,
                content_hash: stream.content_hash,
                chunks,
            })
        })
        .collect()
}

fn store_content_chunks(
    repository: &Repository,
    content: &[u8],
    stats: &mut StoreStats,
) -> anyhow::Result<Vec<ChunkReference>> {
    let mut chunks = Vec::new();
    for bytes in repository_chunks(content, repository.config.chunking.schema) {
        let (object_id, written, stored_bytes) = repository.put_raw(ObjectKind::Chunk, bytes)?;
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
    Ok(chunks)
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

fn insert_directory(
    root: &mut DirectoryNode,
    relative: &str,
    metadata: DirectoryMetadata,
) -> anyhow::Result<()> {
    if relative.is_empty() {
        anyhow::ensure!(root.metadata.is_none(), "duplicate root directory metadata");
        root.metadata = Some(metadata);
        return Ok(());
    }
    let mut node = root;
    for directory in relative.split('/') {
        validate_entry_name(directory)?;
        anyhow::ensure!(!node.files.contains_key(directory), "path type collision");
        node = node.directories.entry(directory.to_string()).or_default();
    }
    anyhow::ensure!(node.metadata.is_none(), "duplicate directory metadata");
    node.metadata = Some(metadata);
    Ok(())
}

fn deserialize_file_object(raw: &[u8]) -> anyhow::Result<FileObject> {
    anyhow::ensure!(raw.len() >= 2, "file object is truncated");
    let object = match u16::from_le_bytes([raw[0], raw[1]]) {
        1 => {
            let file: FileObjectV1 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 1, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: None,
                allocated_extents: None,
                extended_attributes: Vec::new(),
                access_control: None,
                ownership: None,
                alternate_data_streams: Vec::new(),
                chunks: file.chunks,
            }
        }
        2 => {
            let file: FileObjectV2 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 2, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: file.hardlink_id,
                allocated_extents: None,
                extended_attributes: Vec::new(),
                access_control: None,
                ownership: None,
                alternate_data_streams: Vec::new(),
                chunks: file.chunks,
            }
        }
        3 => {
            let file: FileObjectV3 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 3, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: file.hardlink_id,
                allocated_extents: Some(file.allocated_extents),
                extended_attributes: Vec::new(),
                access_control: None,
                ownership: None,
                alternate_data_streams: Vec::new(),
                chunks: file.chunks,
            }
        }
        4 => {
            let file: FileObjectV4 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 4, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: file.hardlink_id,
                allocated_extents: file.allocated_extents,
                extended_attributes: file.extended_attributes,
                access_control: None,
                ownership: None,
                alternate_data_streams: Vec::new(),
                chunks: file.chunks,
            }
        }
        5 => {
            let file: FileObjectV5 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 5, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: file.hardlink_id,
                allocated_extents: file.allocated_extents,
                extended_attributes: file.extended_attributes,
                access_control: file.access_control,
                ownership: None,
                alternate_data_streams: Vec::new(),
                chunks: file.chunks,
            }
        }
        6 => {
            let file: FileObjectV6 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 6, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: file.hardlink_id,
                allocated_extents: file.allocated_extents,
                extended_attributes: file.extended_attributes,
                access_control: file.access_control,
                ownership: file.ownership,
                alternate_data_streams: Vec::new(),
                chunks: file.chunks,
            }
        }
        7 => {
            let file: FileObjectV7 = deserialize_canonical(raw)?;
            anyhow::ensure!(file.schema == 7, "unsupported file object schema");
            FileObject {
                schema: file.schema,
                file_type: file.file_type,
                size: file.size,
                permissions: file.permissions,
                mtime_ns: file.mtime_ns,
                content_hash: file.content_hash,
                chunking_schema: file.chunking_schema,
                hardlink_id: file.hardlink_id,
                allocated_extents: file.allocated_extents,
                extended_attributes: file.extended_attributes,
                access_control: file.access_control,
                ownership: file.ownership,
                alternate_data_streams: file.alternate_data_streams,
                chunks: file.chunks,
            }
        }
        schema => anyhow::bail!("unsupported file object schema {schema}"),
    };
    anyhow::ensure!(
        object.file_type == RepositoryFileType::Regular || object.hardlink_id.is_none(),
        "symbolic-link object cannot declare a hardlink identity"
    );
    anyhow::ensure!(
        object.file_type == RepositoryFileType::Regular || object.allocated_extents.is_none(),
        "symbolic-link object cannot declare sparse extents"
    );
    anyhow::ensure!(
        object.schema >= 7 || object.file_type != RepositoryFileType::SymlinkDirectory,
        "directory symbolic-link type requires file schema 7"
    );
    anyhow::ensure!(
        object.file_type == RepositoryFileType::Regular || object.alternate_data_streams.is_empty(),
        "symbolic-link object cannot declare alternate data streams"
    );
    validate_extended_attributes(&object.extended_attributes)?;
    validate_access_control(object.access_control.as_ref())?;
    validate_alternate_data_streams(&object.alternate_data_streams)?;
    let chunk_bytes = object.chunks.iter().try_fold(0_u64, |total, chunk| {
        total
            .checked_add(chunk.len)
            .ok_or_else(|| anyhow::anyhow!("file chunk length overflow"))
    })?;
    let expected_chunk_bytes = match &object.allocated_extents {
        Some(extents) => validate_file_extents(object.size, extents)?,
        None => object.size,
    };
    anyhow::ensure!(
        chunk_bytes == expected_chunk_bytes,
        "file chunk lengths do not match allocated extent bytes"
    );
    validate_chunk_extent_mapping(
        object.size,
        object.allocated_extents.as_deref(),
        &object.chunks,
    )?;
    Ok(object)
}

fn validate_file_extents(size: u64, extents: &[FileExtent]) -> anyhow::Result<u64> {
    let mut previous_end = 0_u64;
    let mut allocated = 0_u64;
    for extent in extents {
        anyhow::ensure!(extent.len > 0, "sparse extent cannot be empty");
        anyhow::ensure!(extent.offset >= previous_end, "sparse extents overlap");
        let end = extent
            .offset
            .checked_add(extent.len)
            .ok_or_else(|| anyhow::anyhow!("sparse extent overflows"))?;
        anyhow::ensure!(end <= size, "sparse extent exceeds logical file size");
        allocated = allocated
            .checked_add(extent.len)
            .ok_or_else(|| anyhow::anyhow!("sparse allocated length overflows"))?;
        previous_end = end;
    }
    Ok(allocated)
}

fn validate_extended_attributes(attributes: &[ExtendedAttribute]) -> anyhow::Result<()> {
    anyhow::ensure!(
        attributes.len() <= MAX_EXTENDED_ATTRIBUTES,
        "too many extended attributes"
    );
    let mut previous: Option<&[u8]> = None;
    let mut total = 0_usize;
    for attribute in attributes {
        anyhow::ensure!(
            !attribute.name.is_empty(),
            "extended attribute name is empty"
        );
        anyhow::ensure!(
            attribute.name.len() <= MAX_EXTENDED_ATTRIBUTE_NAME_BYTES,
            "extended attribute name is too long"
        );
        anyhow::ensure!(
            !attribute.name.contains(&0),
            "extended attribute name contains NUL"
        );
        anyhow::ensure!(
            attribute.value.len() <= MAX_EXTENDED_ATTRIBUTE_VALUE_BYTES,
            "extended attribute value is too large"
        );
        if let Some(previous) = previous {
            anyhow::ensure!(
                previous < attribute.name.as_slice(),
                "extended attributes are not canonical"
            );
        }
        previous = Some(&attribute.name);
        total = total
            .checked_add(attribute.name.len())
            .and_then(|value| value.checked_add(attribute.value.len()))
            .ok_or_else(|| anyhow::anyhow!("extended attribute size overflows"))?;
        anyhow::ensure!(
            total <= MAX_EXTENDED_ATTRIBUTES_TOTAL_BYTES,
            "extended attributes exceed total size limit"
        );
    }
    Ok(())
}

fn validate_captured_alternate_data_streams(
    streams: &[CapturedAlternateDataStream],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        streams.len() <= MAX_ALTERNATE_DATA_STREAMS,
        "too many alternate data streams"
    );
    let mut previous: Option<&[u16]> = None;
    for stream in streams {
        validate_alternate_data_stream_name(&stream.name)?;
        if let Some(previous) = previous {
            anyhow::ensure!(
                previous < stream.name.as_slice(),
                "alternate data streams are not canonical"
            );
        }
        previous = Some(&stream.name);
        anyhow::ensure!(
            blake3::hash(&stream.content).as_bytes() == &stream.content_hash,
            "captured alternate data stream checksum mismatch"
        );
    }
    Ok(())
}

fn validate_alternate_data_streams(streams: &[AlternateDataStream]) -> anyhow::Result<()> {
    anyhow::ensure!(
        streams.len() <= MAX_ALTERNATE_DATA_STREAMS,
        "too many alternate data streams"
    );
    let mut previous: Option<&[u16]> = None;
    for stream in streams {
        validate_alternate_data_stream_name(&stream.name)?;
        if let Some(previous) = previous {
            anyhow::ensure!(
                previous < stream.name.as_slice(),
                "alternate data streams are not canonical"
            );
        }
        previous = Some(&stream.name);
        let chunk_bytes = stream.chunks.iter().try_fold(0_u64, |total, chunk| {
            total
                .checked_add(chunk.len)
                .ok_or_else(|| anyhow::anyhow!("alternate data stream length overflows"))
        })?;
        anyhow::ensure!(
            chunk_bytes == stream.size,
            "alternate data stream chunk lengths do not match its size"
        );
    }
    Ok(())
}

fn validate_alternate_data_stream_name(name: &[u16]) -> anyhow::Result<()> {
    const DATA_SUFFIX: &[u16] = &[
        b':' as u16,
        b'$' as u16,
        b'D' as u16,
        b'A' as u16,
        b'T' as u16,
        b'A' as u16,
    ];
    anyhow::ensure!(!name.is_empty(), "alternate data stream name is empty");
    anyhow::ensure!(
        name.len() <= MAX_ALTERNATE_DATA_STREAM_NAME_UNITS,
        "alternate data stream name is too long"
    );
    anyhow::ensure!(
        name[0] == b':' as u16,
        "alternate data stream name is invalid"
    );
    anyhow::ensure!(
        name.len() > DATA_SUFFIX.len() + 1 && name.ends_with(DATA_SUFFIX),
        "alternate data stream type is not $DATA"
    );
    let body = &name[1..name.len() - DATA_SUFFIX.len()];
    anyhow::ensure!(
        !body.iter().any(|unit| matches!(*unit, 0 | 47 | 58 | 92)),
        "alternate data stream name contains an unsafe character"
    );
    Ok(())
}

fn validate_access_control(access_control: Option<&AccessControlMetadata>) -> anyhow::Result<()> {
    match access_control {
        None => Ok(()),
        Some(AccessControlMetadata::AppleExtended { text }) => {
            anyhow::ensure!(!text.is_empty(), "Apple ACL text is empty");
            anyhow::ensure!(
                text.len() <= MAX_ACCESS_CONTROL_BYTES,
                "Apple ACL is too large"
            );
            anyhow::ensure!(!text.contains(&0), "Apple ACL text contains NUL");
            Ok(())
        }
        Some(AccessControlMetadata::LinuxPosix { access, default }) => {
            anyhow::ensure!(access.is_some() || default.is_some(), "Linux ACL is empty");
            let total = access.as_ref().map_or(0, Vec::len) + default.as_ref().map_or(0, Vec::len);
            anyhow::ensure!(total <= MAX_ACCESS_CONTROL_BYTES, "Linux ACL is too large");
            Ok(())
        }
        Some(AccessControlMetadata::WindowsSecurityDescriptor { sddl }) => {
            anyhow::ensure!(!sddl.is_empty(), "Windows security descriptor is empty");
            anyhow::ensure!(
                sddl.len() <= MAX_ACCESS_CONTROL_BYTES,
                "Windows ACL is too large"
            );
            anyhow::ensure!(!sddl.contains(&0), "Windows SDDL contains NUL");
            std::str::from_utf8(sddl)?;
            Ok(())
        }
    }
}

fn validate_chunk_extent_mapping(
    size: u64,
    sparse_extents: Option<&[FileExtent]>,
    chunks: &[ChunkReference],
) -> anyhow::Result<()> {
    let dense_extent = [FileExtent {
        offset: 0,
        len: size,
    }];
    let extents = match sparse_extents {
        Some(extents) => extents,
        None if size == 0 => &[],
        None => &dense_extent,
    };
    let mut extent_index = 0_usize;
    let mut consumed = 0_u64;
    for chunk in chunks {
        while extent_index < extents.len() && consumed == extents[extent_index].len {
            extent_index += 1;
            consumed = 0;
        }
        let extent = extents
            .get(extent_index)
            .ok_or_else(|| anyhow::anyhow!("file chunks exceed allocated extents"))?;
        anyhow::ensure!(
            chunk.len <= extent.len - consumed,
            "file chunk crosses an allocated extent boundary"
        );
        consumed += chunk.len;
    }
    while extent_index < extents.len() && consumed == extents[extent_index].len {
        extent_index += 1;
        consumed = 0;
    }
    anyhow::ensure!(
        extent_index == extents.len(),
        "file chunks do not fill allocated extents"
    );
    Ok(())
}

fn read_file_object(
    repository: &Repository,
    file_id: RepositoryObjectId,
) -> anyhow::Result<FileObject> {
    let (kind, raw) = repository.read_raw(file_id)?;
    anyhow::ensure!(kind == ObjectKind::File, "expected repository File object");
    deserialize_file_object(&raw)
}

fn deserialize_tree_object(raw: &[u8]) -> anyhow::Result<TreeObject> {
    anyhow::ensure!(raw.len() >= 2, "tree object is truncated");
    match u16::from_le_bytes([raw[0], raw[1]]) {
        1 => {
            let tree: TreeObjectV1 = deserialize_canonical(raw)?;
            anyhow::ensure!(tree.schema == 1, "unsupported tree schema");
            Ok(TreeObject {
                metadata: None,
                entries: tree.entries,
            })
        }
        2 => {
            let tree: TreeObjectV2 = deserialize_canonical(raw)?;
            anyhow::ensure!(tree.schema == 2, "unsupported tree schema");
            Ok(TreeObject {
                metadata: Some(DirectoryMetadata {
                    permissions: tree.permissions,
                    mtime_ns: tree.mtime_ns,
                    extended_attributes: Vec::new(),
                    access_control: None,
                    ownership: None,
                    alternate_data_streams: Vec::new(),
                }),
                entries: tree.entries,
            })
        }
        3 => {
            let tree: TreeObjectV3 = deserialize_canonical(raw)?;
            anyhow::ensure!(tree.schema == 3, "unsupported tree schema");
            validate_extended_attributes(&tree.extended_attributes)?;
            Ok(TreeObject {
                metadata: Some(DirectoryMetadata {
                    permissions: tree.permissions,
                    mtime_ns: tree.mtime_ns,
                    extended_attributes: tree.extended_attributes,
                    access_control: None,
                    ownership: None,
                    alternate_data_streams: Vec::new(),
                }),
                entries: tree.entries,
            })
        }
        4 => {
            let tree: TreeObjectV4 = deserialize_canonical(raw)?;
            anyhow::ensure!(tree.schema == 4, "unsupported tree schema");
            validate_extended_attributes(&tree.extended_attributes)?;
            validate_access_control(tree.access_control.as_ref())?;
            Ok(TreeObject {
                metadata: Some(DirectoryMetadata {
                    permissions: tree.permissions,
                    mtime_ns: tree.mtime_ns,
                    extended_attributes: tree.extended_attributes,
                    access_control: tree.access_control,
                    ownership: None,
                    alternate_data_streams: Vec::new(),
                }),
                entries: tree.entries,
            })
        }
        5 => {
            let tree: TreeObjectV5 = deserialize_canonical(raw)?;
            anyhow::ensure!(tree.schema == 5, "unsupported tree schema");
            validate_extended_attributes(&tree.extended_attributes)?;
            validate_access_control(tree.access_control.as_ref())?;
            Ok(TreeObject {
                metadata: Some(DirectoryMetadata {
                    permissions: tree.permissions,
                    mtime_ns: tree.mtime_ns,
                    extended_attributes: tree.extended_attributes,
                    access_control: tree.access_control,
                    ownership: tree.ownership,
                    alternate_data_streams: Vec::new(),
                }),
                entries: tree.entries,
            })
        }
        6 => {
            let tree: TreeObjectV6 = deserialize_canonical(raw)?;
            anyhow::ensure!(tree.schema == 6, "unsupported tree schema");
            validate_extended_attributes(&tree.extended_attributes)?;
            validate_access_control(tree.access_control.as_ref())?;
            validate_alternate_data_streams(&tree.alternate_data_streams)?;
            Ok(TreeObject {
                metadata: Some(DirectoryMetadata {
                    permissions: tree.permissions,
                    mtime_ns: tree.mtime_ns,
                    extended_attributes: tree.extended_attributes,
                    access_control: tree.access_control,
                    ownership: tree.ownership,
                    alternate_data_streams: tree.alternate_data_streams,
                }),
                entries: tree.entries,
            })
        }
        schema => anyhow::bail!("unsupported tree schema {schema}"),
    }
}

fn read_tree_object(
    repository: &Repository,
    tree_id: RepositoryObjectId,
) -> anyhow::Result<TreeObject> {
    let (kind, raw) = repository.read_raw(tree_id)?;
    anyhow::ensure!(kind == ObjectKind::Tree, "expected repository Tree object");
    deserialize_tree_object(&raw)
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
    let tree = read_tree_object(repository, tree_id)?;
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
                let object = read_file_object(repository, entry.object_id)?;
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

fn validate_commit_hardlink_groups(
    repository: &Repository,
    commit_id: RepositoryObjectId,
    validated_commits: &mut BTreeSet<RepositoryObjectId>,
) -> anyhow::Result<()> {
    if !validated_commits.insert(commit_id) {
        return Ok(());
    }
    let commit: CommitObject = repository.read(commit_id, ObjectKind::Commit)?;
    let files = flatten_tree(repository, commit.root_tree)?;
    let mut groups = BTreeMap::<[u8; 32], &FileObject>::new();
    for state in files.values() {
        let Some(hardlink_id) = state.object.hardlink_id else {
            continue;
        };
        if let Some(existing) = groups.get(&hardlink_id) {
            anyhow::ensure!(
                *existing == &state.object,
                "hardlink group contains inconsistent file objects"
            );
        } else {
            groups.insert(hardlink_id, &state.object);
        }
    }
    if let Some(parent) = commit.parent {
        validate_commit_hardlink_groups(repository, parent, validated_commits)?;
    }
    Ok(())
}

fn tree_directories(
    repository: &Repository,
    root: RepositoryObjectId,
) -> anyhow::Result<Vec<DirectoryState>> {
    let root_tree = read_tree_object(repository, root)?;
    let mut directories = vec![DirectoryState {
        path: String::new(),
        metadata: root_tree.metadata,
    }];
    tree_directories_entries_at(repository, root_tree.entries, "", &mut directories)?;
    Ok(directories)
}

fn tree_directories_entries_at(
    repository: &Repository,
    entries: Vec<TreeEntry>,
    prefix: &str,
    directories: &mut Vec<DirectoryState>,
) -> anyhow::Result<()> {
    for entry in entries {
        validate_entry_name(&entry.name)?;
        if entry.kind != TreeEntryKind::Tree {
            continue;
        }
        let path = if prefix.is_empty() {
            entry.name
        } else {
            format!("{prefix}/{}", entry.name)
        };
        let tree = read_tree_object(repository, entry.object_id)?;
        directories.push(DirectoryState {
            path: path.clone(),
            metadata: tree.metadata,
        });
        tree_directories_entries_at(repository, tree.entries, &path, directories)?;
    }
    Ok(())
}

fn restore_file(repository: &Repository, state: &FileState, path: &Path) -> anyhow::Result<()> {
    if state.object.file_type != RepositoryFileType::Regular {
        let target = reconstruct_file_bytes(repository, state)?;
        create_symlink(&target, path, state.object.file_type)?;
        restore_ownership(path, state.object.ownership)?;
        restore_extended_attributes(path, &state.object.extended_attributes)?;
        restore_access_control(
            path,
            AccessControlNodeKind::Symlink,
            state.object.access_control.as_ref(),
        )?;
        set_symlink_modified_time(path, state.object.mtime_ns)?;
        return Ok(());
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    if state.object.allocated_extents.is_some() {
        mark_file_sparse(&file)?;
    }
    file.set_len(state.object.size)?;
    let mut hasher = blake3::Hasher::new();
    let mut logical_position = 0_u64;
    let mut stored_bytes = 0_u64;
    for_each_file_data_chunk(repository, state, |offset, bytes| {
        anyhow::ensure!(offset >= logical_position, "file chunks overlap");
        hash_zeroes(&mut hasher, offset - logical_position);
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&bytes)?;
        hasher.update(&bytes);
        logical_position = offset + bytes.len() as u64;
        stored_bytes += bytes.len() as u64;
        Ok(())
    })?;
    hash_zeroes(&mut hasher, state.object.size - logical_position);
    let expected_stored = state
        .object
        .allocated_extents
        .as_ref()
        .map(|extents| validate_file_extents(state.object.size, extents))
        .transpose()?
        .unwrap_or(state.object.size);
    anyhow::ensure!(
        stored_bytes == expected_stored,
        "restored data length mismatch"
    );
    anyhow::ensure!(
        hasher.finalize().as_bytes() == &state.object.content_hash,
        "restored file checksum mismatch"
    );
    file.sync_all()?;
    if let Some(extents) = &state.object.allocated_extents {
        verify_restored_sparse_layout(&file, state.object.size, extents)?;
    }
    restore_ownership(path, state.object.ownership)?;
    restore_alternate_data_streams(repository, path, &state.object.alternate_data_streams)?;
    restore_extended_attributes(path, &state.object.extended_attributes)?;
    set_permissions(path, state.object.permissions)?;
    restore_access_control(
        path,
        AccessControlNodeKind::Regular,
        state.object.access_control.as_ref(),
    )?;
    anyhow::ensure!(
        metadata_permissions(&fs::metadata(path)?) == state.object.permissions,
        "restored file permissions changed while applying ACL"
    );
    set_file_modified_time(&file, state.object.mtime_ns)?;
    Ok(())
}

fn for_each_file_data_chunk(
    repository: &Repository,
    state: &FileState,
    mut callback: impl FnMut(u64, Vec<u8>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let dense_extent = [FileExtent {
        offset: 0,
        len: state.object.size,
    }];
    let extents = match &state.object.allocated_extents {
        Some(extents) => extents.as_slice(),
        None if state.object.size == 0 => &[],
        None => &dense_extent,
    };
    let mut extent_index = 0_usize;
    let mut offset_in_extent = 0_u64;
    for chunk in &state.object.chunks {
        while extent_index < extents.len() && offset_in_extent == extents[extent_index].len {
            extent_index += 1;
            offset_in_extent = 0;
        }
        let extent = extents
            .get(extent_index)
            .ok_or_else(|| anyhow::anyhow!("file chunks exceed allocated extents"))?;
        anyhow::ensure!(
            chunk.len <= extent.len - offset_in_extent,
            "file chunk crosses an allocated extent boundary"
        );
        let (kind, bytes) = repository.read_raw(chunk.object_id)?;
        anyhow::ensure!(
            kind == ObjectKind::Chunk,
            "file references a non-chunk object"
        );
        anyhow::ensure!(bytes.len() as u64 == chunk.len, "chunk length mismatch");
        callback(extent.offset + offset_in_extent, bytes)?;
        offset_in_extent += chunk.len;
    }
    while extent_index < extents.len() && offset_in_extent == extents[extent_index].len {
        extent_index += 1;
        offset_in_extent = 0;
    }
    anyhow::ensure!(
        extent_index == extents.len(),
        "file chunks do not fill allocated extents"
    );
    Ok(())
}

fn verify_hardlink_pair(existing: &Path, linked: &Path) -> anyhow::Result<()> {
    let existing_file = File::open(existing)?;
    let linked_file = File::open(linked)?;
    let existing_identity = source_file_identity(&existing_file, &existing_file.metadata()?)?;
    let linked_identity = source_file_identity(&linked_file, &linked_file.metadata()?)?;
    match (existing_identity, linked_identity) {
        (Some(existing), Some(linked)) => anyhow::ensure!(
            existing.volume == linked.volume
                && existing.file == linked.file
                && existing.links >= 2
                && linked.links >= 2,
            "restored hardlink identity verification failed"
        ),
        _ => anyhow::bail!("hardlink identity verification is unavailable on this platform"),
    }
    Ok(())
}

#[cfg(unix)]
fn mark_file_sparse(_file: &File) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn mark_file_sparse(file: &File) -> anyhow::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let mut returned = 0_u32;
    let result = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            null(),
            0,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn mark_file_sparse(_file: &File) -> anyhow::Result<()> {
    anyhow::bail!("sparse file restore is not supported on this platform")
}

fn verify_restored_sparse_layout(
    file: &File,
    size: u64,
    expected: &[FileExtent],
) -> anyhow::Result<()> {
    let actual = allocated_file_extents(file, size)?
        .ok_or_else(|| anyhow::anyhow!("destination cannot verify sparse file layout"))?;
    for extent in actual {
        let actual_end = extent.offset + extent.len;
        anyhow::ensure!(
            expected.iter().any(|expected| {
                let expected_end = expected.offset + expected.len;
                extent.offset >= expected.offset && actual_end <= expected_end
            }),
            "restored file allocated data inside a declared sparse hole"
        );
    }
    Ok(())
}

fn reconstruct_file_bytes(repository: &Repository, state: &FileState) -> anyhow::Result<Vec<u8>> {
    let size: usize = state
        .object
        .size
        .try_into()
        .map_err(|_| anyhow::anyhow!("file is too large to reconstruct in memory"))?;
    let mut content = vec![0_u8; size];
    for_each_file_data_chunk(repository, state, |offset, bytes| {
        let start: usize = offset
            .try_into()
            .map_err(|_| anyhow::anyhow!("file chunk offset exceeds address space"))?;
        let end = start
            .checked_add(bytes.len())
            .ok_or_else(|| anyhow::anyhow!("file chunk range overflows"))?;
        anyhow::ensure!(end <= content.len(), "file chunk exceeds logical size");
        content[start..end].copy_from_slice(&bytes);
        Ok(())
    })?;
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
                    || left.object.mtime_ns != right.object.mtime_ns
                    || left.object.hardlink_id != right.object.hardlink_id
                    || left.object.allocated_extents != right.object.allocated_extents
                    || left.object.extended_attributes != right.object.extended_attributes
                    || left.object.access_control != right.object.access_control
                    || left.object.ownership != right.object.ownership
                    || left.object.alternate_data_streams
                        != right.object.alternate_data_streams =>
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
            .chain(
                state
                    .object
                    .alternate_data_streams
                    .iter()
                    .flat_map(|stream| stream.chunks.iter()),
            )
            .map(|chunk| chunk.object_id)
            .collect::<BTreeSet<_>>();
        let mut stored_object_bytes = repository.stored_object_size(state.object_id)?;
        stored_objects.insert(state.object_id);
        for chunk in &unique {
            stored_object_bytes += repository.stored_object_size(*chunk)?;
            stored_objects.insert(*chunk);
            global_chunks.insert(*chunk);
        }
        let path_stream_bytes =
            state
                .object
                .alternate_data_streams
                .iter()
                .try_fold(0_u64, |total, stream| {
                    total
                        .checked_add(stream.size)
                        .ok_or_else(|| anyhow::anyhow!("repository storage byte count overflows"))
                })?;
        let path_raw_bytes = state
            .object
            .size
            .checked_add(path_stream_bytes)
            .ok_or_else(|| anyhow::anyhow!("repository path byte count overflows"))?;
        let stream_chunks =
            state
                .object
                .alternate_data_streams
                .iter()
                .try_fold(0_u64, |total, stream| {
                    total
                        .checked_add(stream.chunks.len() as u64)
                        .ok_or_else(|| anyhow::anyhow!("repository stream chunk count overflows"))
                })?;
        let path_chunks = (state.object.chunks.len() as u64)
            .checked_add(stream_chunks)
            .ok_or_else(|| anyhow::anyhow!("repository path chunk count overflows"))?;
        raw_bytes = raw_bytes
            .checked_add(path_raw_bytes)
            .ok_or_else(|| anyhow::anyhow!("repository storage byte count overflows"))?;
        chunks = chunks
            .checked_add(path_chunks)
            .ok_or_else(|| anyhow::anyhow!("repository storage chunk count overflows"))?;
        paths.push(RepositoryStoragePath {
            path: path.clone(),
            file_object: state.object_id,
            raw_bytes: path_raw_bytes,
            chunks: path_chunks,
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
    anyhow::ensure!(end <= state.object.size, "range exceeds logical file size");
    let output_len: usize = len
        .try_into()
        .map_err(|_| anyhow::anyhow!("range is too large for this platform"))?;
    let mut output = vec![0_u8; output_len];
    for_each_file_data_chunk(repository, state, |chunk_start, bytes| {
        let chunk_end = chunk_start + bytes.len() as u64;
        if chunk_end > start && chunk_start < end {
            let overlap_start = start.max(chunk_start);
            let overlap_end = end.min(chunk_end);
            let source_start = (overlap_start - chunk_start) as usize;
            let source_end = (overlap_end - chunk_start) as usize;
            let output_start = (overlap_start - start) as usize;
            let output_end = (overlap_end - start) as usize;
            output[output_start..output_end].copy_from_slice(&bytes[source_start..source_end]);
        }
        Ok(())
    })?;
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
            let file = deserialize_file_object(&raw)?;
            verify_alternate_data_stream_contents(repository, &file.alternate_data_streams)?;
            for chunk in file.chunks.into_iter().chain(
                file.alternate_data_streams
                    .into_iter()
                    .flat_map(|stream| stream.chunks),
            ) {
                verify_reachable(repository, chunk.object_id, visited, report)?;
            }
        }
        ObjectKind::Tree => {
            report.trees += 1;
            let tree = deserialize_tree_object(&raw)?;
            if let Some(metadata) = tree.metadata {
                verify_alternate_data_stream_contents(
                    repository,
                    &metadata.alternate_data_streams,
                )?;
                for chunk in metadata
                    .alternate_data_streams
                    .into_iter()
                    .flat_map(|stream| stream.chunks)
                {
                    verify_reachable(repository, chunk.object_id, visited, report)?;
                }
            }
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

fn stable_read(path: &Path) -> anyhow::Result<StableSourceRead> {
    for _ in 0..3 {
        let mut file = File::open(path)?;
        let before_metadata = file.metadata()?;
        anyhow::ensure!(before_metadata.is_file(), "repository source changed type");
        let before = file_fingerprint(&before_metadata)?;
        let before_identity = source_file_identity(&file, &before_metadata)?;
        let before_attributes = read_extended_attributes(path)?;
        let before_access_control = read_access_control(path, AccessControlNodeKind::Regular)?;
        let before_streams = read_alternate_data_streams(path)?;
        let before_ranges = allocated_file_extents(&file, before.size)?;
        let sparse_extents = before_ranges
            .as_ref()
            .filter(|extents| is_sparse_layout(before.size, extents))
            .cloned();
        let data_extents = read_source_data_extents(&mut file, before.size, &sparse_extents)?;
        let after_metadata = file.metadata()?;
        let after = file_fingerprint(&after_metadata)?;
        let after_identity = source_file_identity(&file, &after_metadata)?;
        let after_attributes = read_extended_attributes(path)?;
        let after_access_control = read_access_control(path, AccessControlNodeKind::Regular)?;
        let after_streams = read_alternate_data_streams(path)?;
        let after_ranges = allocated_file_extents(&file, after.size)?;
        let current_file = File::open(path)?;
        let current_metadata = current_file.metadata()?;
        let current = file_fingerprint(&current_metadata)?;
        let current_identity = source_file_identity(&current_file, &current_metadata)?;
        let current_attributes = read_extended_attributes(path)?;
        let current_access_control = read_access_control(path, AccessControlNodeKind::Regular)?;
        let current_streams = read_alternate_data_streams(path)?;
        let current_ranges = allocated_file_extents(&current_file, current.size)?;
        if before == after
            && after == current
            && before_identity == after_identity
            && after_identity == current_identity
            && before_attributes == after_attributes
            && after_attributes == current_attributes
            && before_access_control == after_access_control
            && after_access_control == current_access_control
            && before_streams == after_streams
            && after_streams == current_streams
            && before_ranges == after_ranges
            && after_ranges == current_ranges
        {
            let content_hash = hash_logical_data(current.size, &data_extents)?;
            return Ok(StableSourceRead {
                logical_size: current.size,
                content_hash,
                data_extents,
                allocated_extents: sparse_extents,
                extended_attributes: current_attributes,
                access_control: current_access_control,
                alternate_data_streams: current_streams,
                fingerprint: current,
                hardlink_id: current_identity.and_then(hardlink_id),
            });
        }
    }
    anyhow::bail!(
        "file changed while creating repository snapshot: {}",
        path.display()
    )
}

fn stable_read_source(source: &SourceFilePath) -> anyhow::Result<StableSourceRead> {
    match source.file_type {
        RepositoryFileType::Regular => stable_read(&source.path),
        RepositoryFileType::Symlink | RepositoryFileType::SymlinkDirectory => {
            for _ in 0..3 {
                let before = file_fingerprint(&fs::symlink_metadata(&source.path)?)?;
                let before_attributes = read_extended_attributes(&source.path)?;
                let before_access_control =
                    read_access_control(&source.path, AccessControlNodeKind::Symlink)?;
                let target = fs::read_link(&source.path)?;
                let bytes = symlink_target_bytes(&target)?;
                let after = file_fingerprint(&fs::symlink_metadata(&source.path)?)?;
                let after_attributes = read_extended_attributes(&source.path)?;
                let after_access_control =
                    read_access_control(&source.path, AccessControlNodeKind::Symlink)?;
                if before == after
                    && before_attributes == after_attributes
                    && before_access_control == after_access_control
                {
                    let logical_size = bytes.len() as u64;
                    return Ok(StableSourceRead {
                        logical_size,
                        content_hash: *blake3::hash(&bytes).as_bytes(),
                        data_extents: vec![StableDataExtent {
                            offset: 0,
                            content: bytes,
                        }],
                        allocated_extents: None,
                        extended_attributes: after_attributes,
                        access_control: after_access_control,
                        alternate_data_streams: Vec::new(),
                        fingerprint: after,
                        hardlink_id: None,
                    });
                }
            }
            anyhow::bail!(
                "symbolic link changed while creating repository snapshot: {}",
                source.path.display()
            )
        }
    }
}

fn read_source_data_extents(
    file: &mut File,
    logical_size: u64,
    sparse_extents: &Option<Vec<FileExtent>>,
) -> anyhow::Result<Vec<StableDataExtent>> {
    let extents = sparse_extents.clone().unwrap_or_else(|| {
        if logical_size == 0 {
            Vec::new()
        } else {
            vec![FileExtent {
                offset: 0,
                len: logical_size,
            }]
        }
    });
    let mut data = Vec::with_capacity(extents.len());
    for extent in extents {
        file.seek(SeekFrom::Start(extent.offset))?;
        let len: usize = extent
            .len
            .try_into()
            .map_err(|_| anyhow::anyhow!("file extent is too large for this platform"))?;
        let mut content = vec![0_u8; len];
        file.read_exact(&mut content)?;
        data.push(StableDataExtent {
            offset: extent.offset,
            content,
        });
    }
    Ok(data)
}

fn hash_logical_data(logical_size: u64, data: &[StableDataExtent]) -> anyhow::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut position = 0_u64;
    for extent in data {
        anyhow::ensure!(extent.offset >= position, "file data extents overlap");
        hash_zeroes(&mut hasher, extent.offset - position);
        hasher.update(&extent.content);
        position = extent
            .offset
            .checked_add(extent.content.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("file data extent overflows"))?;
        anyhow::ensure!(position <= logical_size, "file data exceeds logical size");
    }
    hash_zeroes(&mut hasher, logical_size - position);
    Ok(*hasher.finalize().as_bytes())
}

fn hash_zeroes(hasher: &mut blake3::Hasher, mut len: u64) {
    static ZEROES: [u8; 64 * 1024] = [0; 64 * 1024];
    while len > 0 {
        let take = len.min(ZEROES.len() as u64) as usize;
        hasher.update(&ZEROES[..take]);
        len -= take as u64;
    }
}

fn is_sparse_layout(size: u64, extents: &[FileExtent]) -> bool {
    size > 0 && (extents.len() != 1 || extents[0].offset != 0 || extents[0].len != size)
}

#[cfg(unix)]
fn allocated_file_extents(file: &File, size: u64) -> anyhow::Result<Option<Vec<FileExtent>>> {
    use std::os::fd::AsRawFd;

    if size == 0 {
        return Ok(Some(Vec::new()));
    }
    let size_offset: libc::off_t = size
        .try_into()
        .map_err(|_| anyhow::anyhow!("file is too large for sparse extent discovery"))?;
    let mut extents = Vec::new();
    let mut position: libc::off_t = 0;
    while position < size_offset {
        let data = unsafe { libc::lseek(file.as_raw_fd(), position, libc::SEEK_DATA) };
        if data < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                break;
            }
            if sparse_query_unsupported(&error) {
                return Ok(None);
            }
            return Err(error.into());
        }
        anyhow::ensure!(data >= position, "SEEK_DATA moved backwards");
        anyhow::ensure!(data < size_offset, "SEEK_DATA exceeded logical file size");
        let hole = unsafe { libc::lseek(file.as_raw_fd(), data, libc::SEEK_HOLE) };
        let hole = if hole < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                size_offset
            } else if sparse_query_unsupported(&error) {
                return Ok(None);
            } else {
                return Err(error.into());
            }
        } else {
            hole.min(size_offset)
        };
        anyhow::ensure!(hole > data, "SEEK_HOLE did not advance");
        extents.push(FileExtent {
            offset: data as u64,
            len: (hole - data) as u64,
        });
        position = hole;
    }
    validate_file_extents(size, &extents)?;
    Ok(Some(extents))
}

#[cfg(unix)]
fn sparse_query_unsupported(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EINVAL) | Some(libc::ENOTSUP)
    )
}

#[cfg(windows)]
fn allocated_file_extents(file: &File, size: u64) -> anyhow::Result<Option<Vec<FileExtent>>> {
    use std::ffi::c_void;
    use std::mem::{size_of, size_of_val};
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_FUNCTION, ERROR_MORE_DATA, ERROR_NOT_SUPPORTED,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        FILE_ALLOCATED_RANGE_BUFFER, FSCTL_QUERY_ALLOCATED_RANGES,
    };

    if size == 0 {
        return Ok(Some(Vec::new()));
    }
    let mut extents = Vec::new();
    let mut position = 0_u64;
    while position < size {
        let input = FILE_ALLOCATED_RANGE_BUFFER {
            FileOffset: position.try_into()?,
            Length: (size - position).try_into()?,
        };
        let mut output = [FILE_ALLOCATED_RANGE_BUFFER::default(); 256];
        let mut returned = 0_u32;
        let result = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_QUERY_ALLOCATED_RANGES,
                &input as *const _ as *const c_void,
                size_of::<FILE_ALLOCATED_RANGE_BUFFER>() as u32,
                output.as_mut_ptr() as *mut c_void,
                size_of_val(&output) as u32,
                &mut returned,
                null_mut(),
            )
        };
        let error = (result == 0).then(std::io::Error::last_os_error);
        if let Some(error) = &error {
            let code = error.raw_os_error().unwrap_or_default() as u32;
            if code == ERROR_INVALID_FUNCTION || code == ERROR_NOT_SUPPORTED {
                return Ok(None);
            }
            if code != ERROR_MORE_DATA {
                return Err(std::io::Error::from_raw_os_error(code as i32).into());
            }
        }
        anyhow::ensure!(
            returned as usize % size_of::<FILE_ALLOCATED_RANGE_BUFFER>() == 0,
            "invalid allocated-range response length"
        );
        let count = returned as usize / size_of::<FILE_ALLOCATED_RANGE_BUFFER>();
        anyhow::ensure!(count <= output.len(), "allocated-range response overflow");
        if count == 0 {
            break;
        }
        for range in &output[..count] {
            anyhow::ensure!(
                range.FileOffset >= 0 && range.Length > 0,
                "invalid allocated range"
            );
            let offset = range.FileOffset as u64;
            let end = offset
                .checked_add(range.Length as u64)
                .ok_or_else(|| anyhow::anyhow!("allocated range overflows"))?
                .min(size);
            if end > offset {
                extents.push(FileExtent {
                    offset,
                    len: end - offset,
                });
            }
        }
        let next = extents
            .last()
            .map(|extent| extent.offset + extent.len)
            .unwrap_or(size);
        anyhow::ensure!(next > position, "allocated-range query did not advance");
        position = next;
        if error.is_none() {
            break;
        }
    }
    let extents = coalesce_file_extents(extents)?;
    validate_file_extents(size, &extents)?;
    Ok(Some(extents))
}

#[cfg(windows)]
fn coalesce_file_extents(extents: Vec<FileExtent>) -> anyhow::Result<Vec<FileExtent>> {
    let mut canonical = Vec::<FileExtent>::new();
    for extent in extents {
        if let Some(previous) = canonical.last_mut() {
            let previous_end = previous
                .offset
                .checked_add(previous.len)
                .ok_or_else(|| anyhow::anyhow!("allocated range overflows"))?;
            anyhow::ensure!(extent.offset >= previous_end, "allocated ranges overlap");
            if extent.offset == previous_end {
                previous.len = previous
                    .len
                    .checked_add(extent.len)
                    .ok_or_else(|| anyhow::anyhow!("allocated range overflows"))?;
                continue;
            }
        }
        canonical.push(extent);
    }
    Ok(canonical)
}

#[cfg(not(any(unix, windows)))]
fn allocated_file_extents(_file: &File, _size: u64) -> anyhow::Result<Option<Vec<FileExtent>>> {
    Ok(None)
}

#[cfg(unix)]
fn source_file_identity(
    _file: &File,
    metadata: &fs::Metadata,
) -> anyhow::Result<Option<SourceFileIdentity>> {
    use std::os::unix::fs::MetadataExt;
    Ok(Some(SourceFileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
        links: metadata.nlink(),
    }))
}

#[cfg(windows)]
fn source_file_identity(
    file: &File,
    _metadata: &fs::Metadata,
) -> anyhow::Result<Option<SourceFileIdentity>> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let result =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if result == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let information = unsafe { information.assume_init() };
    Ok(Some(SourceFileIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        file: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
        links: u64::from(information.nNumberOfLinks),
    }))
}

#[cfg(not(any(unix, windows)))]
fn source_file_identity(
    _file: &File,
    _metadata: &fs::Metadata,
) -> anyhow::Result<Option<SourceFileIdentity>> {
    Ok(None)
}

fn hardlink_id(identity: SourceFileIdentity) -> Option<[u8; 32]> {
    if identity.links <= 1 {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig-repository-hardlink-v1\0");
    hasher.update(&identity.volume.to_le_bytes());
    hasher.update(&identity.file.to_le_bytes());
    Some(*hasher.finalize().as_bytes())
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

#[cfg(windows)]
fn repository_symlink_type(path: &Path) -> anyhow::Result<RepositoryFileType> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;

    let attributes = fs::symlink_metadata(path)?.file_attributes();
    Ok(if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        RepositoryFileType::SymlinkDirectory
    } else {
        RepositoryFileType::Symlink
    })
}

#[cfg(not(windows))]
fn repository_symlink_type(_path: &Path) -> anyhow::Result<RepositoryFileType> {
    Ok(RepositoryFileType::Symlink)
}

fn create_symlink(target: &[u8], path: &Path, file_type: RepositoryFileType) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let _ = file_type;
        symlink(PathBuf::from(OsString::from_vec(target.to_vec())), path)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let target = std::str::from_utf8(target)?;
        match file_type {
            RepositoryFileType::Symlink => symlink_file(target, path)?,
            RepositoryFileType::SymlinkDirectory => symlink_dir(target, path)?,
            RepositoryFileType::Regular => anyhow::bail!("regular file cannot use symlink restore"),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, path, file_type);
        anyhow::bail!("symlink restore is not supported on this platform");
    }
    Ok(())
}

fn file_fingerprint(metadata: &fs::Metadata) -> anyhow::Result<FileFingerprint> {
    Ok(FileFingerprint {
        size: metadata.len(),
        modified_ns: metadata_modified_ns(metadata)?,
        permissions: metadata_permissions(metadata),
        ownership: metadata_ownership(metadata),
    })
}

fn capture_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> anyhow::Result<CapturedDirectoryMetadata> {
    anyhow::ensure!(metadata.is_dir(), "repository directory changed type");
    Ok(CapturedDirectoryMetadata {
        permissions: metadata_permissions(metadata),
        mtime_ns: metadata_modified_ns(metadata)?,
        extended_attributes: read_extended_attributes(path)
            .context("failed to read directory extended attributes")?,
        access_control: read_access_control(path, AccessControlNodeKind::Directory)
            .context("failed to read directory ACL")?,
        ownership: metadata_ownership(metadata),
        alternate_data_streams: read_alternate_data_streams(path)
            .context("failed to read directory alternate data streams")?,
    })
}

fn metadata_modified_ns(metadata: &fs::Metadata) -> anyhow::Result<i128> {
    Ok(metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as i128)
        .unwrap_or_else(|error| -(error.duration().as_nanos() as i128)))
}

fn metadata_permissions(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        u32::from(metadata.permissions().readonly())
    }
}

#[cfg(unix)]
fn metadata_ownership(metadata: &fs::Metadata) -> Option<OwnershipMetadata> {
    use std::os::unix::fs::MetadataExt;

    Some(OwnershipMetadata {
        user_id: metadata.uid(),
        group_id: metadata.gid(),
    })
}

#[cfg(not(unix))]
fn metadata_ownership(_metadata: &fs::Metadata) -> Option<OwnershipMetadata> {
    None
}

#[cfg(not(windows))]
fn read_alternate_data_streams(_path: &Path) -> anyhow::Result<Vec<CapturedAlternateDataStream>> {
    Ok(Vec::new())
}

#[cfg(windows)]
fn read_alternate_data_streams(path: &Path) -> anyhow::Result<Vec<CapturedAlternateDataStream>> {
    windows_alternate_data_streams::read(path)
}

#[cfg(not(windows))]
fn restore_alternate_data_streams(
    _repository: &Repository,
    _path: &Path,
    expected: &[AlternateDataStream],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected.is_empty(),
        "alternate data streams are not supported on this destination"
    );
    Ok(())
}

#[cfg(windows)]
fn restore_alternate_data_streams(
    repository: &Repository,
    path: &Path,
    expected: &[AlternateDataStream],
) -> anyhow::Result<()> {
    validate_alternate_data_streams(expected)?;
    let expected = expected
        .iter()
        .map(|stream| {
            Ok(CapturedAlternateDataStream {
                name: stream.name.clone(),
                content_hash: stream.content_hash,
                content: reconstruct_alternate_data_stream(repository, stream)?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    windows_alternate_data_streams::restore(path, &expected)
}

fn reconstruct_alternate_data_stream(
    repository: &Repository,
    stream: &AlternateDataStream,
) -> anyhow::Result<Vec<u8>> {
    let capacity: usize = stream
        .size
        .try_into()
        .map_err(|_| anyhow::anyhow!("alternate data stream is too large for this platform"))?;
    let mut content = Vec::with_capacity(capacity);
    for chunk in &stream.chunks {
        let (kind, bytes) = repository.read_raw(chunk.object_id)?;
        anyhow::ensure!(kind == ObjectKind::Chunk, "expected stream Chunk object");
        anyhow::ensure!(
            bytes.len() as u64 == chunk.len,
            "alternate data stream chunk length mismatch"
        );
        content.extend_from_slice(&bytes);
    }
    anyhow::ensure!(
        content.len() as u64 == stream.size,
        "reconstructed alternate data stream length mismatch"
    );
    anyhow::ensure!(
        blake3::hash(&content).as_bytes() == &stream.content_hash,
        "reconstructed alternate data stream checksum mismatch"
    );
    Ok(content)
}

fn verify_alternate_data_stream_contents(
    repository: &Repository,
    streams: &[AlternateDataStream],
) -> anyhow::Result<()> {
    for stream in streams {
        reconstruct_alternate_data_stream(repository, stream)?;
    }
    Ok(())
}

#[cfg(windows)]
mod windows_alternate_data_streams {
    use super::{
        CapturedAlternateDataStream, validate_alternate_data_stream_name,
        validate_captured_alternate_data_streams,
    };
    use std::collections::BTreeSet;
    use std::ffi::{OsStr, OsString};
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_HANDLE_EOF, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES,
        HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FindClose, FindFirstStreamW,
        FindNextStreamW, FindStreamInfoStandard, WIN32_FIND_STREAM_DATA,
    };

    const DEFAULT_DATA_STREAM: &[u16] = &[
        b':' as u16,
        b':' as u16,
        b'$' as u16,
        b'D' as u16,
        b'A' as u16,
        b'T' as u16,
        b'A' as u16,
    ];

    pub(super) fn read(path: &Path) -> anyhow::Result<Vec<CapturedAlternateDataStream>> {
        let mut streams = Vec::new();
        for (name, declared_size) in enumerate(path)? {
            if name == DEFAULT_DATA_STREAM {
                continue;
            }
            validate_alternate_data_stream_name(&name)?;
            anyhow::ensure!(declared_size >= 0, "alternate data stream size is negative");
            let mut file = open_stream(path, &name, false)?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            anyhow::ensure!(
                content.len() as i64 == declared_size,
                "alternate data stream changed while being read"
            );
            streams.push(CapturedAlternateDataStream {
                name,
                content_hash: *blake3::hash(&content).as_bytes(),
                content,
            });
        }
        streams.sort_by(|left, right| left.name.cmp(&right.name));
        validate_captured_alternate_data_streams(&streams)?;
        Ok(streams)
    }

    pub(super) fn restore(
        path: &Path,
        expected: &[CapturedAlternateDataStream],
    ) -> anyhow::Result<()> {
        validate_captured_alternate_data_streams(expected)?;
        let expected_names = expected
            .iter()
            .map(|stream| stream.name.as_slice())
            .collect::<BTreeSet<_>>();
        for (name, _) in enumerate(path)? {
            if name != DEFAULT_DATA_STREAM && !expected_names.contains(name.as_slice()) {
                fs::remove_file(stream_path(path, &name))?;
            }
        }
        for stream in expected {
            let mut file = open_stream(path, &stream.name, true)?;
            file.write_all(&stream.content)?;
            file.sync_all()?;
        }
        anyhow::ensure!(
            read(path)? == expected,
            "restored alternate data streams failed exact verification"
        );
        Ok(())
    }

    fn enumerate(path: &Path) -> anyhow::Result<Vec<(Vec<u16>, i64)>> {
        let path = wide_null(path.as_os_str());
        let mut data = WIN32_FIND_STREAM_DATA::default();
        let handle = unsafe {
            FindFirstStreamW(
                path.as_ptr(),
                FindStreamInfoStandard,
                (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = std::io::Error::last_os_error();
            if error
                .raw_os_error()
                .is_some_and(|code| no_stream_error(code as u32))
            {
                return Ok(Vec::new());
            }
            return Err(error.into());
        }
        let handle = FindStreamHandle(handle);
        let mut streams = Vec::new();
        loop {
            streams.push((stream_name(&data)?, data.StreamSize));
            data = WIN32_FIND_STREAM_DATA::default();
            let next = unsafe {
                FindNextStreamW(handle.0, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast())
            };
            if next == 0 {
                let error = std::io::Error::last_os_error();
                if error
                    .raw_os_error()
                    .is_some_and(|code| end_of_stream_enumeration(code as u32))
                {
                    break;
                }
                return Err(error.into());
            }
        }
        Ok(streams)
    }

    fn no_stream_error(code: u32) -> bool {
        matches!(
            code,
            ERROR_HANDLE_EOF | ERROR_FILE_NOT_FOUND | ERROR_INVALID_PARAMETER
        )
    }

    fn end_of_stream_enumeration(code: u32) -> bool {
        matches!(code, ERROR_HANDLE_EOF | ERROR_NO_MORE_FILES)
    }

    fn stream_name(data: &WIN32_FIND_STREAM_DATA) -> anyhow::Result<Vec<u16>> {
        let len = data
            .cStreamName
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| anyhow::anyhow!("alternate data stream name is not terminated"))?;
        anyhow::ensure!(len > 0, "alternate data stream name is empty");
        Ok(data.cStreamName[..len].to_vec())
    }

    fn open_stream(path: &Path, name: &[u16], write: bool) -> anyhow::Result<File> {
        let mut options = OpenOptions::new();
        options
            .read(!write)
            .write(write)
            .create(write)
            .truncate(write)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        Ok(options.open(stream_path(path, name))?)
    }

    pub(super) fn stream_path(path: &Path, name: &[u16]) -> PathBuf {
        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide.extend_from_slice(name);
        PathBuf::from(OsString::from_wide(&wide))
    }

    fn wide_null(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    struct FindStreamHandle(HANDLE);

    impl Drop for FindStreamHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = FindClose(self.0);
            }
        }
    }
}

#[cfg(unix)]
fn read_extended_attributes(path: &Path) -> anyhow::Result<Vec<ExtendedAttribute>> {
    use std::os::unix::ffi::OsStrExt;

    let mut attributes = Vec::new();
    for name in xattr::list(path)? {
        let name = name.as_bytes().to_vec();
        if ignored_extended_attribute(&name) || access_control_attribute(&name) {
            continue;
        }
        let value = xattr::get(path, std::ffi::OsStr::from_bytes(&name))?
            .ok_or_else(|| anyhow::anyhow!("extended attribute disappeared during capture"))?;
        attributes.push(ExtendedAttribute { name, value });
    }
    attributes.sort_by(|left, right| left.name.cmp(&right.name));
    validate_extended_attributes(&attributes)?;
    Ok(attributes)
}

#[cfg(not(unix))]
fn read_extended_attributes(_path: &Path) -> anyhow::Result<Vec<ExtendedAttribute>> {
    Ok(Vec::new())
}

#[cfg(unix)]
fn restore_extended_attributes(path: &Path, expected: &[ExtendedAttribute]) -> anyhow::Result<()> {
    use std::collections::BTreeSet;
    use std::os::unix::ffi::OsStrExt;

    validate_extended_attributes(expected)?;
    let expected_names = expected
        .iter()
        .map(|attribute| attribute.name.as_slice())
        .collect::<BTreeSet<_>>();
    for name in xattr::list(path)? {
        let bytes = name.as_bytes();
        if ignored_extended_attribute(bytes)
            || access_control_attribute(bytes)
            || expected_names.contains(bytes)
        {
            continue;
        }
        xattr::remove(path, &name)?;
    }
    for attribute in expected {
        xattr::set(
            path,
            std::ffi::OsStr::from_bytes(&attribute.name),
            &attribute.value,
        )?;
    }
    anyhow::ensure!(
        read_extended_attributes(path)? == expected,
        "restored extended attributes failed exact verification"
    );
    Ok(())
}

#[cfg(not(unix))]
fn restore_extended_attributes(_path: &Path, expected: &[ExtendedAttribute]) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected.is_empty(),
        "extended attributes are not supported on this platform"
    );
    Ok(())
}

fn ignored_extended_attribute(name: &[u8]) -> bool {
    #[cfg(target_vendor = "apple")]
    {
        matches!(
            name,
            b"com.apple.provenance" | b"com.apple.macl" | b"com.apple.system.Security"
        )
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = name;
        false
    }
}

fn access_control_attribute(name: &[u8]) -> bool {
    matches!(
        name,
        b"system.posix_acl_access" | b"system.posix_acl_default"
    )
}

#[cfg(target_vendor = "apple")]
fn read_access_control(
    path: &Path,
    kind: AccessControlNodeKind,
) -> anyhow::Result<Option<AccessControlMetadata>> {
    apple_access_control::read(path, kind)
}

#[cfg(target_vendor = "apple")]
fn restore_access_control(
    path: &Path,
    kind: AccessControlNodeKind,
    expected: Option<&AccessControlMetadata>,
) -> anyhow::Result<()> {
    apple_access_control::restore(path, kind, expected)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_access_control(
    path: &Path,
    kind: AccessControlNodeKind,
) -> anyhow::Result<Option<AccessControlMetadata>> {
    use std::os::unix::ffi::OsStrExt;

    if kind == AccessControlNodeKind::Symlink {
        return Ok(None);
    }
    let access = xattr::get(
        path,
        std::ffi::OsStr::from_bytes(b"system.posix_acl_access"),
    )?;
    let default = if kind == AccessControlNodeKind::Directory {
        xattr::get(
            path,
            std::ffi::OsStr::from_bytes(b"system.posix_acl_default"),
        )?
    } else {
        None
    };
    if access.is_none() && default.is_none() {
        Ok(None)
    } else {
        let metadata = AccessControlMetadata::LinuxPosix { access, default };
        validate_access_control(Some(&metadata))?;
        Ok(Some(metadata))
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn restore_access_control(
    path: &Path,
    kind: AccessControlNodeKind,
    expected: Option<&AccessControlMetadata>,
) -> anyhow::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    if kind == AccessControlNodeKind::Symlink {
        anyhow::ensure!(
            expected.is_none(),
            "POSIX ACL cannot be applied to a symbolic link"
        );
        return Ok(());
    }
    let (expected_access, expected_default) = match expected {
        None => (None, None),
        Some(AccessControlMetadata::LinuxPosix { access, default }) => {
            (access.as_deref(), default.as_deref())
        }
        Some(_) => anyhow::bail!("ACL platform does not match Linux destination"),
    };
    let access_name = std::ffi::OsStr::from_bytes(b"system.posix_acl_access");
    let default_name = std::ffi::OsStr::from_bytes(b"system.posix_acl_default");
    apply_linux_acl_attribute(path, access_name, expected_access)?;
    if kind == AccessControlNodeKind::Directory {
        apply_linux_acl_attribute(path, default_name, expected_default)?;
    } else {
        anyhow::ensure!(
            expected_default.is_none(),
            "default ACL requires a directory"
        );
    }
    anyhow::ensure!(
        read_access_control(path, kind)?.as_ref() == expected,
        "restored Linux ACL failed exact verification"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn apply_linux_acl_attribute(
    path: &Path,
    name: &std::ffi::OsStr,
    value: Option<&[u8]>,
) -> anyhow::Result<()> {
    match value {
        Some(value) => xattr::set(path, name, value)?,
        None if xattr::get(path, name)?.is_some() => xattr::remove(path, name)?,
        None => {}
    }
    Ok(())
}

#[cfg(windows)]
fn read_access_control(
    path: &Path,
    _kind: AccessControlNodeKind,
) -> anyhow::Result<Option<AccessControlMetadata>> {
    windows_access_control::read(path).map(Some)
}

#[cfg(windows)]
fn restore_access_control(
    path: &Path,
    _kind: AccessControlNodeKind,
    expected: Option<&AccessControlMetadata>,
) -> anyhow::Result<()> {
    match expected {
        Some(expected) => windows_access_control::restore(path, expected),
        None => Ok(()),
    }
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn read_access_control(
    _path: &Path,
    _kind: AccessControlNodeKind,
) -> anyhow::Result<Option<AccessControlMetadata>> {
    Ok(None)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn restore_access_control(
    _path: &Path,
    _kind: AccessControlNodeKind,
    expected: Option<&AccessControlMetadata>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected.is_none(),
        "ACL platform is not supported by this destination"
    );
    Ok(())
}

#[cfg(windows)]
mod windows_access_control {
    use super::{AccessControlMetadata, MAX_ACCESS_CONTROL_BYTES, validate_access_control};
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT, SetSecurityInfo,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetSecurityDescriptorGroup, GetSecurityDescriptorOwner,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL, WRITE_DAC,
        WRITE_OWNER,
    };

    const SECURITY_INFORMATION: u32 =
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;

    pub(super) fn read(path: &Path) -> anyhow::Result<AccessControlMetadata> {
        let file = open_security_handle(path, READ_CONTROL)?;
        let mut descriptor = null_mut();
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(result as i32).into());
        }
        let mut text = null_mut();
        let mut text_len = 0_u32;
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                SECURITY_INFORMATION,
                &mut text,
                &mut text_len,
            )
        };
        let descriptor_free = unsafe { LocalFree(descriptor) };
        if converted == 0 {
            anyhow::ensure!(
                descriptor_free.is_null(),
                "failed to release security descriptor"
            );
            return Err(std::io::Error::last_os_error().into());
        }
        let text_len: usize = text_len.try_into()?;
        if text_len > MAX_ACCESS_CONTROL_BYTES {
            let text_free = unsafe { LocalFree(text.cast()) };
            anyhow::ensure!(
                descriptor_free.is_null(),
                "failed to release security descriptor"
            );
            anyhow::ensure!(text_free.is_null(), "failed to release SDDL text");
            anyhow::bail!("Windows security descriptor is too large");
        }
        let wide = unsafe { std::slice::from_raw_parts(text, text_len) };
        let decoded = String::from_utf16(wide);
        let text_free = unsafe { LocalFree(text.cast()) };
        anyhow::ensure!(
            descriptor_free.is_null(),
            "failed to release security descriptor"
        );
        anyhow::ensure!(text_free.is_null(), "failed to release SDDL text");
        let metadata = AccessControlMetadata::WindowsSecurityDescriptor {
            sddl: decoded?.into_bytes(),
        };
        validate_access_control(Some(&metadata))?;
        Ok(metadata)
    }

    pub(super) fn restore(path: &Path, expected: &AccessControlMetadata) -> anyhow::Result<()> {
        let sddl = match expected {
            AccessControlMetadata::WindowsSecurityDescriptor { sddl } => sddl,
            _ => anyhow::bail!("ACL platform does not match Windows destination"),
        };
        let mut wide = std::str::from_utf8(sddl)?
            .encode_utf16()
            .collect::<Vec<_>>();
        wide.push(0);
        let mut descriptor = null_mut();
        let mut descriptor_size = 0_u32;
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                &mut descriptor_size,
            )
        };
        if converted == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        anyhow::ensure!(
            descriptor_size > 0,
            "parsed Windows security descriptor is empty"
        );
        let result = apply_descriptor(path, descriptor);
        let descriptor_free = unsafe { LocalFree(descriptor) };
        anyhow::ensure!(
            descriptor_free.is_null(),
            "failed to release parsed security descriptor"
        );
        result?;
        anyhow::ensure!(
            read(path)? == *expected,
            "restored Windows ACL failed exact verification"
        );
        Ok(())
    }

    fn apply_descriptor(
        path: &Path,
        descriptor: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
    ) -> anyhow::Result<()> {
        let file = open_security_handle(path, READ_CONTROL | WRITE_DAC | WRITE_OWNER)?;
        let mut owner = null_mut();
        let mut group = null_mut();
        let mut dacl = null_mut();
        let mut defaulted = 0;
        let mut dacl_present = 0;
        anyhow::ensure!(
            unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) } != 0,
            "stored Windows owner SID is invalid"
        );
        anyhow::ensure!(
            unsafe { GetSecurityDescriptorGroup(descriptor, &mut group, &mut defaulted) } != 0,
            "stored Windows group SID is invalid"
        );
        anyhow::ensure!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut dacl_present, &mut dacl, &mut defaulted)
            } != 0,
            "stored Windows DACL is invalid"
        );
        let mut control = 0_u16;
        let mut revision = 0_u32;
        anyhow::ensure!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } != 0,
            "stored Windows security descriptor control is invalid"
        );
        let mut information = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION;
        if dacl_present != 0 {
            information |= DACL_SECURITY_INFORMATION;
            information |= if control & SE_DACL_PROTECTED != 0 {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
        }
        let result = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                information,
                owner,
                group,
                dacl,
                null_mut(),
            )
        };
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(result as i32).into());
        }
        Ok(())
    }

    fn open_security_handle(path: &Path, access: u32) -> anyhow::Result<File> {
        Ok(OpenOptions::new()
            .access_mode(access)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?)
    }
}

#[cfg(target_vendor = "apple")]
mod apple_access_control {
    use super::{AccessControlMetadata, AccessControlNodeKind, MAX_ACCESS_CONTROL_BYTES};
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;

    unsafe extern "C" {
        fn acl_get_file(path: *const c_char, kind: c_int) -> *mut c_void;
        fn acl_get_link_np(path: *const c_char, kind: c_int) -> *mut c_void;
        fn acl_set_file(path: *const c_char, kind: c_int, acl: *mut c_void) -> c_int;
        fn acl_set_link_np(path: *const c_char, kind: c_int, acl: *mut c_void) -> c_int;
        fn acl_to_text(acl: *mut c_void, len: *mut isize) -> *mut c_char;
        fn acl_from_text(text: *const c_char) -> *mut c_void;
        fn acl_init(count: c_int) -> *mut c_void;
        fn acl_free(value: *mut c_void) -> c_int;
    }

    pub(super) fn read(
        path: &Path,
        kind: AccessControlNodeKind,
    ) -> anyhow::Result<Option<AccessControlMetadata>> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let acl = unsafe {
            match kind {
                AccessControlNodeKind::Symlink => acl_get_link_np(path.as_ptr(), ACL_TYPE_EXTENDED),
                AccessControlNodeKind::Regular | AccessControlNodeKind::Directory => {
                    acl_get_file(path.as_ptr(), ACL_TYPE_EXTENDED)
                }
            }
        };
        if acl.is_null() {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::EINVAL) | Some(libc::ENOENT)
            ) {
                return Ok(None);
            }
            return Err(error.into());
        }
        let mut len = 0_isize;
        let text = unsafe { acl_to_text(acl, &mut len) };
        let acl_free_result = unsafe { acl_free(acl) };
        if text.is_null() {
            anyhow::ensure!(acl_free_result == 0, "failed to release captured ACL");
            return Err(std::io::Error::last_os_error().into());
        }
        if len < 0 || len as usize > MAX_ACCESS_CONTROL_BYTES {
            let text_free_result = unsafe { acl_free(text.cast()) };
            anyhow::ensure!(acl_free_result == 0, "failed to release captured ACL");
            anyhow::ensure!(text_free_result == 0, "failed to release ACL text");
            anyhow::bail!("Apple ACL text length is invalid");
        }
        let bytes = unsafe { CStr::from_ptr(text) }.to_bytes().to_vec();
        let text_free_result = unsafe { acl_free(text.cast()) };
        anyhow::ensure!(acl_free_result == 0, "failed to release captured ACL");
        anyhow::ensure!(text_free_result == 0, "failed to release ACL text");
        if bytes.iter().all(u8::is_ascii_whitespace) {
            Ok(None)
        } else {
            Ok(Some(AccessControlMetadata::AppleExtended { text: bytes }))
        }
    }

    pub(super) fn restore(
        path: &Path,
        kind: AccessControlNodeKind,
        expected: Option<&AccessControlMetadata>,
    ) -> anyhow::Result<()> {
        let current = read(path, kind)?;
        if current.as_ref() == expected {
            return Ok(());
        }
        let acl = match expected {
            Some(AccessControlMetadata::AppleExtended { text }) => {
                let text = CString::new(text.as_slice())?;
                let acl = unsafe { acl_from_text(text.as_ptr()) };
                anyhow::ensure!(!acl.is_null(), "stored Apple ACL text is invalid");
                acl
            }
            None => {
                let acl = unsafe { acl_init(0) };
                anyhow::ensure!(!acl.is_null(), "failed to allocate empty Apple ACL");
                acl
            }
            Some(_) => anyhow::bail!("ACL platform does not match Apple destination"),
        };
        let original_path = path;
        let path = CString::new(path.as_os_str().as_bytes())?;
        let result = unsafe {
            match kind {
                AccessControlNodeKind::Symlink => {
                    acl_set_link_np(path.as_ptr(), ACL_TYPE_EXTENDED, acl)
                }
                AccessControlNodeKind::Regular | AccessControlNodeKind::Directory => {
                    acl_set_file(path.as_ptr(), ACL_TYPE_EXTENDED, acl)
                }
            }
        };
        let error = (result != 0).then(std::io::Error::last_os_error);
        let free_result = unsafe { acl_free(acl) };
        anyhow::ensure!(free_result == 0, "failed to release restored ACL");
        if let Some(error) = error {
            return Err(error.into());
        }
        anyhow::ensure!(
            read(original_path, kind)?.as_ref() == expected,
            "restored Apple ACL failed exact verification"
        );
        Ok(())
    }
}

#[cfg(unix)]
fn restore_ownership(path: &Path, expected: Option<OwnershipMetadata>) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Some(expected) = expected else {
        return Ok(());
    };
    let current = metadata_ownership(&fs::symlink_metadata(path)?);
    if current == Some(expected) {
        return Ok(());
    }
    let original_path = path;
    let path = CString::new(path.as_os_str().as_bytes())?;
    let result = unsafe { libc::lchown(path.as_ptr(), expected.user_id, expected.group_id) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    anyhow::ensure!(
        metadata_ownership(&fs::symlink_metadata(original_path)?) == Some(expected),
        "restored Unix ownership failed exact verification"
    );
    Ok(())
}

#[cfg(not(unix))]
fn restore_ownership(_path: &Path, expected: Option<OwnershipMetadata>) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected.is_none(),
        "Unix ownership metadata is not supported on this destination"
    );
    Ok(())
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

fn set_file_modified_time(file: &File, unix_ns: i128) -> anyhow::Result<()> {
    let modified = system_time_from_unix_ns(unix_ns)?;
    file.set_times(std::fs::FileTimes::new().set_modified(modified))?;
    Ok(())
}

fn set_directory_metadata(
    repository: &Repository,
    path: &Path,
    metadata: &DirectoryMetadata,
) -> anyhow::Result<()> {
    restore_ownership(path, metadata.ownership)?;
    restore_alternate_data_streams(repository, path, &metadata.alternate_data_streams)?;
    restore_extended_attributes(path, &metadata.extended_attributes)?;
    set_permissions(path, metadata.permissions)?;
    restore_access_control(
        path,
        AccessControlNodeKind::Directory,
        metadata.access_control.as_ref(),
    )?;
    anyhow::ensure!(
        metadata_permissions(&fs::metadata(path)?) == metadata.permissions,
        "restored directory permissions changed while applying ACL"
    );
    set_directory_modified_time(path, metadata.mtime_ns)
}

#[cfg(unix)]
fn set_directory_modified_time(path: &Path, unix_ns: i128) -> anyhow::Result<()> {
    set_file_modified_time(&File::open(path)?, unix_ns)
}

#[cfg(windows)]
fn set_directory_modified_time(path: &Path, unix_ns: i128) -> anyhow::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let directory = OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    set_file_modified_time(&directory, unix_ns)
}

#[cfg(not(any(unix, windows)))]
fn set_directory_modified_time(path: &Path, unix_ns: i128) -> anyhow::Result<()> {
    set_file_modified_time(&File::open(path)?, unix_ns)
}

fn system_time_from_unix_ns(unix_ns: i128) -> anyhow::Result<SystemTime> {
    let magnitude: u128 = unix_ns.unsigned_abs();
    let seconds: u64 = (magnitude / 1_000_000_000)
        .try_into()
        .map_err(|_| anyhow::anyhow!("filesystem timestamp is out of range"))?;
    let nanos = (magnitude % 1_000_000_000) as u32;
    let duration = Duration::new(seconds, nanos);
    if unix_ns >= 0 {
        UNIX_EPOCH
            .checked_add(duration)
            .ok_or_else(|| anyhow::anyhow!("filesystem timestamp is out of range"))
    } else {
        UNIX_EPOCH
            .checked_sub(duration)
            .ok_or_else(|| anyhow::anyhow!("filesystem timestamp is out of range"))
    }
}

#[cfg(unix)]
fn set_symlink_modified_time(path: &Path, unix_ns: i128) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let seconds = unix_ns.div_euclid(1_000_000_000);
    let nanos = unix_ns.rem_euclid(1_000_000_000);
    let seconds: libc::time_t = seconds
        .try_into()
        .map_err(|_| anyhow::anyhow!("symlink timestamp is out of range"))?;
    let nanos: libc::c_long = nanos
        .try_into()
        .map_err(|_| anyhow::anyhow!("symlink timestamp is out of range"))?;
    let path = CString::new(path.as_os_str().as_bytes())?;
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: nanos,
        },
    ];
    let result = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            path.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(windows)]
fn set_symlink_modified_time(path: &Path, unix_ns: i128) -> anyhow::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    set_file_modified_time(&file, unix_ns)
}

#[cfg(not(any(unix, windows)))]
fn set_symlink_modified_time(_path: &Path, _unix_ns: i128) -> anyhow::Result<()> {
    anyhow::bail!("symlink timestamp restore is not supported on this platform")
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

    #[test]
    fn legacy_v1_tree_objects_remain_verifiable_and_restorable() {
        let temp = tempfile::tempdir().unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let content = b"legacy file object";
        let (chunk_id, _, _) = repository.put_raw(ObjectKind::Chunk, content).unwrap();
        let legacy_file = FileObjectV1 {
            schema: 1,
            file_type: RepositoryFileType::Regular,
            size: content.len() as u64,
            permissions: 0o100644,
            mtime_ns: 1_700_000_000_000_000_000,
            content_hash: *blake3::hash(content).as_bytes(),
            chunking_schema: 1,
            chunks: vec![ChunkReference {
                object_id: chunk_id,
                len: content.len() as u64,
            }],
        };
        let (file_id, _, _) = repository.put(ObjectKind::File, &legacy_file).unwrap();
        let legacy_tree = TreeObjectV1 {
            schema: 1,
            entries: vec![TreeEntry {
                name: "legacy.txt".to_string(),
                kind: TreeEntryKind::File,
                object_id: file_id,
            }],
        };
        let (tree_id, _, _) = repository.put(ObjectKind::Tree, &legacy_tree).unwrap();
        let legacy_commit = CommitObject {
            schema: 1,
            root_tree: tree_id,
            parent: None,
            created_unix_ns: 1_700_000_000_000_000_000,
            message: "legacy tree".to_string(),
            author: None,
            files: 1,
            input_bytes: content.len() as u64,
            change_index: None,
            semantic_index: None,
            compression_tree_index: None,
        };
        let (commit_id, _, _) = repository.put(ObjectKind::Commit, &legacy_commit).unwrap();
        repository.publish_head(commit_id).unwrap();

        let decoded = read_tree_object(&repository, tree_id).unwrap();
        assert_eq!(decoded.metadata, None);
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(
            read_file_object(&repository, file_id).unwrap().hardlink_id,
            None
        );
        verify_repository(temp.path()).unwrap();

        let output = temp.path().parent().unwrap().join(format!(
            "restored-v1-tree-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(fs::read(output.join("legacy.txt")).unwrap(), content);
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn directory_modified_times_are_restored_exactly() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("src/generated");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("artifact.bin"), b"directory metadata").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let root_time = 1_700_000_000_123_456_700;
        let nested_time = 1_700_000_100_765_432_100;
        set_directory_modified_time(&nested, nested_time).unwrap();
        set_directory_modified_time(temp.path(), root_time).unwrap();
        let expected_root =
            capture_directory_metadata(temp.path(), &fs::metadata(temp.path()).unwrap()).unwrap();
        let expected_nested =
            capture_directory_metadata(&nested, &fs::metadata(&nested).unwrap()).unwrap();

        snapshot_repository(temp.path(), "directory metadata".to_string(), None).unwrap();
        fs::remove_dir_all(temp.path().join("src")).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-directory-time-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();

        let restored_root =
            capture_directory_metadata(&output, &fs::metadata(&output).unwrap()).unwrap();
        let restored_path = output.join("src/generated");
        let restored_nested =
            capture_directory_metadata(&restored_path, &fs::metadata(&restored_path).unwrap())
                .unwrap();
        assert_eq!(restored_root.mtime_ns, expected_root.mtime_ns);
        assert_eq!(restored_nested.mtime_ns, expected_nested.mtime_ns);
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_permissions_are_restored_exactly() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join("private/nested");
        fs::create_dir_all(&private).unwrap();
        fs::write(private.join("secret.txt"), b"secret").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        fs::set_permissions(
            temp.path().join("private"),
            fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o710)).unwrap();

        snapshot_repository(temp.path(), "directory modes".to_string(), None).unwrap();
        fs::remove_dir_all(temp.path().join("private")).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-directory-mode-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();

        assert_eq!(
            fs::metadata(output.join("private"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        assert_eq!(
            fs::metadata(output.join("private/nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o710
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn regular_file_modified_time_is_restored_exactly() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("time.txt");
        fs::write(&source, b"timestamp").unwrap();
        let expected_time = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700);
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(expected_time))
            .unwrap();
        let expected = file_fingerprint(&fs::symlink_metadata(&source).unwrap())
            .unwrap()
            .modified_ns;
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "timestamp".to_string(), None).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-time-{}",
            hex::encode(crate::random_bytes::<4>())
        ));

        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        let restored = file_fingerprint(&fs::symlink_metadata(output.join("time.txt")).unwrap())
            .unwrap()
            .modified_ns;
        assert_eq!(restored, expected);
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_preserved_by_restore() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("target.txt"), b"target").unwrap();
        symlink("target.txt", temp.path().join("link.txt")).unwrap();
        set_symlink_modified_time(&temp.path().join("link.txt"), 1_700_000_000_123_456_700)
            .unwrap();
        let expected_mtime =
            file_fingerprint(&fs::symlink_metadata(temp.path().join("link.txt")).unwrap())
                .unwrap()
                .modified_ns;
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
        let restored_mtime =
            file_fingerprint(&fs::symlink_metadata(output.join("link.txt")).unwrap())
                .unwrap()
                .modified_ns;
        assert_eq!(restored_mtime, expected_mtime);
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_ownership_survives_source_deletion_for_files_directories_and_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("owned");
        let file = directory.join("payload.bin");
        let link = directory.join("payload.link");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&file, b"owned payload").unwrap();
        symlink("payload.bin", &link).unwrap();
        let expected_root = metadata_ownership(&fs::symlink_metadata(temp.path()).unwrap());
        let expected_directory = metadata_ownership(&fs::symlink_metadata(&directory).unwrap());
        let expected_file = metadata_ownership(&fs::symlink_metadata(&file).unwrap());
        let expected_link = metadata_ownership(&fs::symlink_metadata(&link).unwrap());

        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot =
            snapshot_repository(temp.path(), "Unix ownership".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        assert_eq!(files["owned/payload.bin"].object.schema, 7);
        assert_eq!(files["owned/payload.bin"].object.ownership, expected_file);
        assert_eq!(files["owned/payload.link"].object.ownership, expected_link);

        fs::remove_dir_all(&directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-ownership-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            metadata_ownership(&fs::symlink_metadata(&output).unwrap()),
            expected_root
        );
        assert_eq!(
            metadata_ownership(&fs::symlink_metadata(output.join("owned")).unwrap()),
            expected_directory
        );
        assert_eq!(
            metadata_ownership(&fs::symlink_metadata(output.join("owned/payload.bin")).unwrap()),
            expected_file
        );
        assert_eq!(
            metadata_ownership(&fs::symlink_metadata(output.join("owned/payload.link")).unwrap()),
            expected_link
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn hardlink_identity_survives_source_deletion_and_restore() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.bin");
        let alias = temp.path().join("nested/alias.bin");
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        fs::write(&original, b"shared inode content").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "hardlink".to_string(), None).unwrap();

        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(repository.read_head().unwrap().unwrap(), ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        assert_eq!(files["original.bin"].object.schema, 7);
        assert!(files["original.bin"].object.hardlink_id.is_some());
        assert_eq!(
            files["original.bin"].object.hardlink_id,
            files["nested/alias.bin"].object.hardlink_id
        );
        assert_eq!(
            files["original.bin"].object_id,
            files["nested/alias.bin"].object_id
        );

        fs::remove_file(&original).unwrap();
        fs::remove_dir_all(temp.path().join("nested")).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-hardlink-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        let restored_original = output.join("original.bin");
        let restored_alias = output.join("nested/alias.bin");
        verify_hardlink_pair(&restored_original, &restored_alias).unwrap();
        fs::write(&restored_original, b"changed through one name").unwrap();
        assert_eq!(
            fs::read(&restored_alias).unwrap(),
            b"changed through one name"
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn replacing_one_hardlink_with_an_independent_copy_is_a_metadata_change() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.bin");
        let alias = temp.path().join("alias.bin");
        fs::write(&original, b"same bytes").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "linked".to_string(), None).unwrap();
        let original_metadata = fs::metadata(&original).unwrap();

        fs::remove_file(&alias).unwrap();
        fs::copy(&original, &alias).unwrap();
        set_file_modified_time(
            &File::options().write(true).open(&alias).unwrap(),
            metadata_modified_ns(&original_metadata).unwrap(),
        )
        .unwrap();
        set_permissions(&alias, metadata_permissions(&original_metadata)).unwrap();
        let second = snapshot_repository(temp.path(), "unlinked".to_string(), None).unwrap();
        let diff = repository_diff(
            temp.path(),
            Some(&first.commit_id.to_hex()),
            Some(&second.commit_id.to_hex()),
        )
        .unwrap();
        assert_eq!(diff.metadata, 2);
        assert_eq!(
            diff.changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>(),
            ["alias.bin", "original.bin"]
        );
        assert!(
            diff.changes
                .iter()
                .all(|change| change.kind == RepositoryChangeKind::Metadata)
        );
    }

    #[test]
    fn inconsistent_hardlink_group_fails_verify_and_atomic_restore() {
        let temp = tempfile::tempdir().unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let hardlink_id = [0x5a; 32];
        let mut entries = Vec::new();
        for (name, content) in [("a.bin", b"a".as_slice()), ("b.bin", b"b".as_slice())] {
            let (chunk_id, _, _) = repository.put_raw(ObjectKind::Chunk, content).unwrap();
            let file = FileObjectV2 {
                schema: 2,
                file_type: RepositoryFileType::Regular,
                size: 1,
                permissions: 0o100644,
                mtime_ns: 1_700_000_000_000_000_000,
                content_hash: *blake3::hash(content).as_bytes(),
                chunking_schema: 2,
                hardlink_id: Some(hardlink_id),
                chunks: vec![ChunkReference {
                    object_id: chunk_id,
                    len: 1,
                }],
            };
            let (file_id, _, _) = repository.put(ObjectKind::File, &file).unwrap();
            entries.push(TreeEntry {
                name: name.to_string(),
                kind: TreeEntryKind::File,
                object_id: file_id,
            });
        }
        let root_metadata =
            capture_directory_metadata(temp.path(), &fs::metadata(temp.path()).unwrap()).unwrap();
        let tree = TreeObjectV2 {
            schema: 2,
            permissions: root_metadata.permissions,
            mtime_ns: root_metadata.mtime_ns,
            entries,
        };
        let (tree_id, _, _) = repository.put(ObjectKind::Tree, &tree).unwrap();
        let commit = CommitObject {
            schema: 1,
            root_tree: tree_id,
            parent: None,
            created_unix_ns: now_unix_ns(),
            message: "invalid hardlink group".to_string(),
            author: None,
            files: 2,
            input_bytes: 2,
            change_index: None,
            semantic_index: None,
            compression_tree_index: None,
        };
        let (commit_id, _, _) = repository.put(ObjectKind::Commit, &commit).unwrap();
        repository.publish_head(commit_id).unwrap();

        let verify_error = verify_repository(temp.path()).unwrap_err().to_string();
        assert!(verify_error.contains("hardlink group contains inconsistent"));
        let output = temp.path().parent().unwrap().join(format!(
            "rejected-hardlink-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        let restore_error = restore_repository(temp.path(), "HEAD", &output, None, false)
            .unwrap_err()
            .to_string();
        assert!(restore_error.contains("hardlink group contains inconsistent"));
        assert!(!output.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn sparse_extents_survive_source_deletion_and_exact_restore() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("disk-image.bin");
        let logical_size = 32 * 1024 * 1024 + 123;
        let first_offset = 1024 * 1024 + 17;
        let second_offset = 20 * 1024 * 1024 + 31;
        let first_payload = vec![0xa5; 8192];
        let second_payload = vec![0x3c; 16384];
        let mut file = File::options()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&source)
            .unwrap();
        file.set_len(logical_size).unwrap();
        file.seek(SeekFrom::Start(first_offset)).unwrap();
        file.write_all(&first_payload).unwrap();
        file.seek(SeekFrom::Start(second_offset)).unwrap();
        file.write_all(&second_payload).unwrap();
        file.sync_all().unwrap();
        let source_extents = allocated_file_extents(&file, logical_size)
            .unwrap()
            .expect("test filesystem must expose sparse extents");
        assert!(is_sparse_layout(logical_size, &source_extents));
        drop(file);

        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot = snapshot_repository(temp.path(), "sparse file".to_string(), None).unwrap();
        assert_eq!(snapshot.input_bytes, logical_size);
        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        let state = &files["disk-image.bin"];
        assert_eq!(state.object.schema, 7);
        assert_eq!(
            state.object.allocated_extents.as_deref(),
            Some(source_extents.as_slice())
        );
        let stored_data_bytes = state
            .object
            .chunks
            .iter()
            .map(|chunk| chunk.len)
            .sum::<u64>();
        let allocated_data_bytes = validate_file_extents(logical_size, &source_extents).unwrap();
        assert_eq!(stored_data_bytes, allocated_data_bytes);
        assert!(stored_data_bytes < logical_size);
        let across_hole = reconstruct_file_range(
            &repository,
            state,
            first_offset + first_payload.len() as u64,
            4096,
        )
        .unwrap();
        assert_eq!(across_hole, vec![0; 4096]);

        fs::remove_file(&source).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-sparse-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        let restored = output.join("disk-image.bin");
        let mut restored_file = File::open(&restored).unwrap();
        assert_eq!(restored_file.metadata().unwrap().len(), logical_size);
        let restored_extents = allocated_file_extents(&restored_file, logical_size)
            .unwrap()
            .expect("restore filesystem must expose sparse extents");
        assert!(is_sparse_layout(logical_size, &restored_extents));
        let mut payload = vec![0; first_payload.len()];
        restored_file.seek(SeekFrom::Start(first_offset)).unwrap();
        restored_file.read_exact(&mut payload).unwrap();
        assert_eq!(payload, first_payload);
        let mut payload = vec![0; second_payload.len()];
        restored_file.seek(SeekFrom::Start(second_offset)).unwrap();
        restored_file.read_exact(&mut payload).unwrap();
        assert_eq!(payload, second_payload);
        let restored_bytes = fs::read(&restored).unwrap();
        assert_eq!(
            blake3::hash(&restored_bytes).as_bytes(),
            &state.object.content_hash
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn fully_sparse_file_restores_without_payload_objects() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("empty-volume.bin");
        let logical_size = 8 * 1024 * 1024;
        File::create(&source)
            .unwrap()
            .set_len(logical_size)
            .unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot = snapshot_repository(temp.path(), "fully sparse".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        let state = &files["empty-volume.bin"];
        assert_eq!(state.object.schema, 7);
        assert_eq!(state.object.allocated_extents, Some(Vec::new()));
        assert!(state.object.chunks.is_empty());

        fs::remove_file(&source).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-fully-sparse-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        let restored = File::open(output.join("empty-volume.bin")).unwrap();
        assert_eq!(restored.metadata().unwrap().len(), logical_size);
        assert_eq!(
            allocated_file_extents(&restored, logical_size).unwrap(),
            Some(Vec::new())
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn malformed_sparse_extents_are_rejected_before_object_traversal() {
        let malformed = FileObjectV3 {
            schema: 3,
            file_type: RepositoryFileType::Regular,
            size: 4096,
            permissions: 0,
            mtime_ns: 0,
            content_hash: [0; 32],
            chunking_schema: 2,
            hardlink_id: None,
            allocated_extents: vec![
                FileExtent {
                    offset: 0,
                    len: 2048,
                },
                FileExtent {
                    offset: 1024,
                    len: 2048,
                },
            ],
            chunks: Vec::new(),
        };
        let error = deserialize_file_object(&serialize_canonical(&malformed).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("sparse extents overlap"));
    }

    #[cfg(unix)]
    #[test]
    fn extended_attributes_survive_source_deletion_on_files_directories_and_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("metadata");
        let file = directory.join("payload.bin");
        let link = directory.join("payload.link");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&file, b"xattr payload").unwrap();
        symlink("payload.bin", &link).unwrap();
        xattr::set(temp.path(), "user.hig.root", b"root\0value").unwrap();
        xattr::set(&directory, "user.hig.directory", &[0, 1, 2, 0xff]).unwrap();
        xattr::set(&file, "user.hig.file", &[0xff, 0, 0x7f, 0x80]).unwrap();
        xattr::set(&link, "user.hig.symlink", b"link attribute").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot = snapshot_repository(temp.path(), "xattrs".to_string(), None).unwrap();

        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        assert_eq!(files["metadata/payload.bin"].object.schema, 7);
        assert_eq!(
            files["metadata/payload.bin"].object.extended_attributes,
            vec![ExtendedAttribute {
                name: b"user.hig.file".to_vec(),
                value: vec![0xff, 0, 0x7f, 0x80],
            }]
        );
        assert!(
            files["metadata/payload.link"]
                .object
                .extended_attributes
                .iter()
                .any(|attribute| attribute.name == b"user.hig.symlink")
        );
        assert!(files.values().all(|state| {
            state
                .object
                .extended_attributes
                .iter()
                .all(|attribute| attribute.name != b"com.apple.provenance")
        }));

        fs::remove_dir_all(&directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-xattrs-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            xattr::get(&output, "user.hig.root").unwrap().unwrap(),
            b"root\0value"
        );
        assert_eq!(
            xattr::get(output.join("metadata"), "user.hig.directory")
                .unwrap()
                .unwrap(),
            [0, 1, 2, 0xff]
        );
        assert_eq!(
            xattr::get(output.join("metadata/payload.bin"), "user.hig.file")
                .unwrap()
                .unwrap(),
            [0xff, 0, 0x7f, 0x80]
        );
        assert_eq!(
            xattr::get(output.join("metadata/payload.link"), "user.hig.symlink")
                .unwrap()
                .unwrap(),
            b"link attribute"
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn extended_attribute_change_is_versioned_as_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.bin");
        fs::write(&file, b"unchanged bytes").unwrap();
        xattr::set(&file, "user.hig.version", b"one").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first xattr".to_string(), None).unwrap();
        xattr::set(&file, "user.hig.version", b"two").unwrap();
        let second = snapshot_repository(temp.path(), "second xattr".to_string(), None).unwrap();

        let diff = repository_diff(
            temp.path(),
            Some(&first.commit_id.to_hex()),
            Some(&second.commit_id.to_hex()),
        )
        .unwrap();
        assert_eq!(diff.metadata, 1);
        assert_eq!(diff.changes[0].path, "file.bin");
        assert_eq!(diff.changes[0].kind, RepositoryChangeKind::Metadata);
        assert_eq!(
            diff.changes[0].old_content_hash,
            diff.changes[0].new_content_hash
        );
    }

    #[test]
    fn noncanonical_extended_attributes_are_rejected() {
        let malformed = FileObjectV4 {
            schema: 4,
            file_type: RepositoryFileType::Regular,
            size: 0,
            permissions: 0,
            mtime_ns: 0,
            content_hash: *blake3::hash(&[]).as_bytes(),
            chunking_schema: 2,
            hardlink_id: None,
            allocated_extents: None,
            extended_attributes: vec![
                ExtendedAttribute {
                    name: b"user.z".to_vec(),
                    value: vec![1],
                },
                ExtendedAttribute {
                    name: b"user.a".to_vec(),
                    value: vec![2],
                },
            ],
            chunks: Vec::new(),
        };
        let error = deserialize_file_object(&serialize_canonical(&malformed).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("extended attributes are not canonical"));
    }

    #[test]
    fn previous_filesystem_metadata_schemas_decode_missing_fields_explicitly() {
        let file = FileObjectV5 {
            schema: 5,
            file_type: RepositoryFileType::Regular,
            size: 0,
            permissions: 0o640,
            mtime_ns: 123,
            content_hash: *blake3::hash(&[]).as_bytes(),
            chunking_schema: 2,
            hardlink_id: None,
            allocated_extents: None,
            extended_attributes: Vec::new(),
            access_control: None,
            chunks: Vec::new(),
        };
        let decoded_file = deserialize_file_object(&serialize_canonical(&file).unwrap()).unwrap();
        assert_eq!(decoded_file.schema, 5);
        assert_eq!(decoded_file.ownership, None);

        let tree = TreeObjectV4 {
            schema: 4,
            permissions: 0o750,
            mtime_ns: 456,
            extended_attributes: Vec::new(),
            access_control: None,
            entries: Vec::new(),
        };
        let decoded_tree = deserialize_tree_object(&serialize_canonical(&tree).unwrap()).unwrap();
        assert_eq!(decoded_tree.metadata.unwrap().ownership, None);

        let file = FileObjectV6 {
            schema: 6,
            file_type: RepositoryFileType::Regular,
            size: 0,
            permissions: 0o640,
            mtime_ns: 789,
            content_hash: *blake3::hash(&[]).as_bytes(),
            chunking_schema: 2,
            hardlink_id: None,
            allocated_extents: None,
            extended_attributes: Vec::new(),
            access_control: None,
            ownership: None,
            chunks: Vec::new(),
        };
        let decoded_file = deserialize_file_object(&serialize_canonical(&file).unwrap()).unwrap();
        assert_eq!(decoded_file.schema, 6);
        assert!(decoded_file.alternate_data_streams.is_empty());

        let tree = TreeObjectV5 {
            schema: 5,
            permissions: 0o750,
            mtime_ns: 987,
            extended_attributes: Vec::new(),
            access_control: None,
            ownership: None,
            entries: Vec::new(),
        };
        let decoded_tree = deserialize_tree_object(&serialize_canonical(&tree).unwrap()).unwrap();
        assert!(
            decoded_tree
                .metadata
                .unwrap()
                .alternate_data_streams
                .is_empty()
        );
    }

    #[test]
    fn unsafe_or_noncanonical_alternate_data_streams_are_rejected() {
        let valid_name = ":valid:$DATA".encode_utf16().collect::<Vec<_>>();
        let unsafe_name = ":../escape:$DATA".encode_utf16().collect::<Vec<_>>();
        let empty_hash = *blake3::hash(&[]).as_bytes();
        let unsafe_stream = AlternateDataStream {
            name: unsafe_name,
            size: 0,
            content_hash: empty_hash,
            chunks: Vec::new(),
        };
        assert!(
            validate_alternate_data_streams(&[unsafe_stream])
                .unwrap_err()
                .to_string()
                .contains("unsafe character")
        );
        let duplicate = AlternateDataStream {
            name: valid_name,
            size: 0,
            content_hash: empty_hash,
            chunks: Vec::new(),
        };
        assert!(
            validate_alternate_data_streams(&[duplicate.clone(), duplicate])
                .unwrap_err()
                .to_string()
                .contains("not canonical")
        );
    }

    #[test]
    fn alternate_data_stream_checksum_mismatch_fails_verify_and_atomic_restore() {
        let temp = tempfile::tempdir().unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let stream_bytes = b"named stream";
        let (chunk_id, _, _) = repository.put_raw(ObjectKind::Chunk, stream_bytes).unwrap();
        let file = FileObjectV7 {
            schema: 7,
            file_type: RepositoryFileType::Regular,
            size: 0,
            permissions: 0o100644,
            mtime_ns: 0,
            content_hash: *blake3::hash(&[]).as_bytes(),
            chunking_schema: 2,
            hardlink_id: None,
            allocated_extents: None,
            extended_attributes: Vec::new(),
            access_control: None,
            ownership: None,
            alternate_data_streams: vec![AlternateDataStream {
                name: ":metadata:$DATA".encode_utf16().collect(),
                size: stream_bytes.len() as u64,
                content_hash: [0x5a; 32],
                chunks: vec![ChunkReference {
                    object_id: chunk_id,
                    len: stream_bytes.len() as u64,
                }],
            }],
            chunks: Vec::new(),
        };
        let (file_id, _, _) = repository.put(ObjectKind::File, &file).unwrap();
        let tree = TreeObjectV1 {
            schema: 1,
            entries: vec![TreeEntry {
                name: "payload.bin".to_string(),
                kind: TreeEntryKind::File,
                object_id: file_id,
            }],
        };
        let (tree_id, _, _) = repository.put(ObjectKind::Tree, &tree).unwrap();
        let commit = CommitObject {
            schema: 1,
            root_tree: tree_id,
            parent: None,
            created_unix_ns: now_unix_ns(),
            message: "invalid ADS checksum".to_string(),
            author: None,
            files: 1,
            input_bytes: stream_bytes.len() as u64,
            change_index: None,
            semantic_index: None,
            compression_tree_index: None,
        };
        let (commit_id, _, _) = repository.put(ObjectKind::Commit, &commit).unwrap();
        repository.publish_head(commit_id).unwrap();

        let verify_error = verify_repository(temp.path()).unwrap_err().to_string();
        assert!(verify_error.contains("stream checksum mismatch"));
        let output = temp.path().parent().unwrap().join(format!(
            "rejected-stream-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        let restore_error = restore_repository(temp.path(), "HEAD", &output, None, false)
            .unwrap_err()
            .to_string();
        assert!(restore_error.contains("stream"));
        assert!(!output.exists());
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn apple_acl_survives_source_deletion_for_file_and_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("controlled");
        let file = directory.join("document.txt");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&file, b"access controlled").unwrap();
        let user = std::env::var("USER").unwrap();
        let rule = format!("user:{user} allow read,write");
        for path in [&directory, &file] {
            let status = std::process::Command::new("chmod")
                .arg("+a")
                .arg(&rule)
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let expected_file = read_access_control(&file, AccessControlNodeKind::Regular)
            .unwrap()
            .expect("file ACL must be captured");
        let expected_directory = read_access_control(&directory, AccessControlNodeKind::Directory)
            .unwrap()
            .expect("directory ACL must be captured");
        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot = snapshot_repository(temp.path(), "Apple ACL".to_string(), None)
            .unwrap_or_else(|error| panic!("Apple ACL snapshot failed: {error:#}"));

        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        assert_eq!(
            files["controlled/document.txt"].object.access_control,
            Some(expected_file.clone())
        );

        fs::remove_dir_all(&directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-apple-acl-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            read_access_control(
                &output.join("controlled/document.txt"),
                AccessControlNodeKind::Regular,
            )
            .unwrap(),
            Some(expected_file)
        );
        assert_eq!(
            read_access_control(&output.join("controlled"), AccessControlNodeKind::Directory,)
                .unwrap(),
            Some(expected_directory)
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn apple_acl_change_is_versioned_as_metadata_without_content_change() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("policy.txt");
        fs::write(&file, b"stable content").unwrap();
        let user = std::env::var("USER").unwrap();
        let first_rule = format!("user:{user} allow read");
        assert!(
            std::process::Command::new("chmod")
                .arg("+a")
                .arg(&first_rule)
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "read ACL".to_string(), None).unwrap();
        assert!(
            std::process::Command::new("chmod")
                .args(["-a#", "0"])
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        let second_rule = format!("user:{user} allow read,write");
        assert!(
            std::process::Command::new("chmod")
                .arg("+a")
                .arg(&second_rule)
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        let second = snapshot_repository(temp.path(), "read-write ACL".to_string(), None).unwrap();
        let diff = repository_diff(
            temp.path(),
            Some(&first.commit_id.to_hex()),
            Some(&second.commit_id.to_hex()),
        )
        .unwrap();
        assert_eq!(diff.metadata, 1);
        assert_eq!(diff.changes[0].path, "policy.txt");
        assert_eq!(
            diff.changes[0].old_content_hash,
            diff.changes[0].new_content_hash
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn linux_posix_acl_survives_source_deletion_for_file_and_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("controlled");
        let file = directory.join("document.txt");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&file, b"access controlled").unwrap();
        let user = std::env::var("USER").unwrap();
        let access_rule = format!("u:{user}:rw");
        let default_rule = format!("d:u:{user}:rwx");
        assert!(
            std::process::Command::new("setfacl")
                .args(["-m", access_rule.as_str()])
                .arg(&file)
                .status()
                .expect("setfacl is required for native ACL qualification")
                .success()
        );
        assert!(
            std::process::Command::new("setfacl")
                .args(["-m", default_rule.as_str()])
                .arg(&directory)
                .status()
                .expect("setfacl is required for native ACL qualification")
                .success()
        );
        let expected_file = read_access_control(&file, AccessControlNodeKind::Regular)
            .unwrap()
            .expect("file ACL must be captured");
        let expected_directory = read_access_control(&directory, AccessControlNodeKind::Directory)
            .unwrap()
            .expect("directory ACL must be captured");
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "Linux ACL".to_string(), None).unwrap();
        fs::remove_dir_all(&directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-linux-acl-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            read_access_control(
                &output.join("controlled/document.txt"),
                AccessControlNodeKind::Regular,
            )
            .unwrap(),
            Some(expected_file)
        );
        assert_eq!(
            read_access_control(&output.join("controlled"), AccessControlNodeKind::Directory,)
                .unwrap(),
            Some(expected_directory)
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_owner_group_and_dacl_survive_source_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("controlled");
        let file = directory.join("document.txt");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&file, b"access controlled").unwrap();
        let expected_file = read_access_control(&file, AccessControlNodeKind::Regular)
            .unwrap()
            .expect("file security descriptor must be captured");
        let expected_directory = read_access_control(&directory, AccessControlNodeKind::Directory)
            .unwrap()
            .expect("directory security descriptor must be captured");
        init_repository(temp.path(), Vec::new()).unwrap();
        snapshot_repository(temp.path(), "Windows DACL".to_string(), None).unwrap();
        fs::remove_dir_all(&directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-windows-acl-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            read_access_control(
                &output.join("controlled/document.txt"),
                AccessControlNodeKind::Regular,
            )
            .unwrap(),
            Some(expected_file)
        );
        assert_eq!(
            read_access_control(&output.join("controlled"), AccessControlNodeKind::Directory,)
                .unwrap(),
            Some(expected_directory)
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_alternate_data_streams_survive_source_deletion_for_file_and_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("streams");
        let file = directory.join("payload.bin");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&file, b"default stream").unwrap();
        let file_stream_name = ":hig-large:$DATA".encode_utf16().collect::<Vec<_>>();
        let directory_stream_name = ":hig-directory:$DATA".encode_utf16().collect::<Vec<_>>();
        let mut file_stream = (0..(2 * 1024 * 1024 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let directory_stream = b"directory stream metadata".to_vec();
        fs::write(
            windows_alternate_data_streams::stream_path(&file, &file_stream_name),
            &file_stream,
        )
        .unwrap();
        fs::write(
            windows_alternate_data_streams::stream_path(&directory, &directory_stream_name),
            &directory_stream,
        )
        .unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "Windows ADS".to_string(), None).unwrap();
        file_stream[1024 * 1024 + 9] ^= 0xff;
        fs::write(
            windows_alternate_data_streams::stream_path(&file, &file_stream_name),
            &file_stream,
        )
        .unwrap();
        let snapshot =
            snapshot_repository(temp.path(), "Windows ADS update".to_string(), None).unwrap();
        let diff = repository_diff(
            temp.path(),
            Some(&first.commit_id.to_hex()),
            Some(&snapshot.commit_id.to_hex()),
        )
        .unwrap();
        assert_eq!(diff.metadata, 1);
        assert_eq!(diff.modified, 0);
        assert_eq!(diff.changes[0].path, "streams/payload.bin");
        let expected_file_streams = read_alternate_data_streams(&file).unwrap();
        let expected_directory_streams = read_alternate_data_streams(&directory).unwrap();

        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        assert_eq!(files["streams/payload.bin"].object.schema, 7);
        assert_eq!(
            files["streams/payload.bin"].object.alternate_data_streams[0].size,
            file_stream.len() as u64
        );
        let directories = tree_directories(&repository, commit.root_tree).unwrap();
        let stored_directory = directories
            .iter()
            .find(|entry| entry.path == "streams")
            .unwrap();
        assert_eq!(
            stored_directory
                .metadata
                .as_ref()
                .unwrap()
                .alternate_data_streams[0]
                .size,
            directory_stream.len() as u64
        );
        verify_repository(temp.path()).unwrap();

        fs::remove_dir_all(&directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-windows-streams-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert_eq!(
            read_alternate_data_streams(&output.join("streams/payload.bin")).unwrap(),
            expected_file_streams
        );
        assert_eq!(
            read_alternate_data_streams(&output.join("streams")).unwrap(),
            expected_directory_streams
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_and_directory_symlink_types_survive_source_deletion() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        let temp = tempfile::tempdir().unwrap();
        let target_file = temp.path().join("target.txt");
        let target_directory = temp.path().join("target-directory");
        let file_link = temp.path().join("file-link");
        let directory_link = temp.path().join("directory-link");
        fs::write(&target_file, b"target").unwrap();
        fs::create_dir_all(&target_directory).unwrap();
        symlink_file("target.txt", &file_link)
            .expect("native Windows qualification requires symbolic-link support");
        symlink_dir("target-directory", &directory_link)
            .expect("native Windows qualification requires directory symbolic-link support");
        init_repository(temp.path(), Vec::new()).unwrap();
        let snapshot =
            snapshot_repository(temp.path(), "Windows symlink types".to_string(), None).unwrap();
        let repository = Repository::discover(temp.path()).unwrap();
        let commit: CommitObject = repository
            .read(snapshot.commit_id, ObjectKind::Commit)
            .unwrap();
        let files = flatten_tree(&repository, commit.root_tree).unwrap();
        assert_eq!(
            files["file-link"].object.file_type,
            RepositoryFileType::Symlink
        );
        assert_eq!(
            files["directory-link"].object.file_type,
            RepositoryFileType::SymlinkDirectory
        );

        fs::remove_file(&file_link).unwrap();
        fs::remove_dir(&directory_link).unwrap();
        fs::remove_file(&target_file).unwrap();
        fs::remove_dir_all(&target_directory).unwrap();
        let output = temp.path().parent().unwrap().join(format!(
            "restored-windows-links-{}",
            hex::encode(crate::random_bytes::<4>())
        ));
        restore_repository(temp.path(), "HEAD", &output, None, false).unwrap();
        assert!(
            fs::symlink_metadata(output.join("file-link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(output.join("directory-link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::metadata(output.join("directory-link"))
                .unwrap()
                .is_dir()
        );
        assert_eq!(
            fs::read_link(output.join("file-link")).unwrap(),
            Path::new("target.txt")
        );
        assert_eq!(
            fs::read_link(output.join("directory-link")).unwrap(),
            Path::new("target-directory")
        );
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
    fn watcher_reconciles_changes_without_a_native_event() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("watched.txt"), b"first").unwrap();
        init_repository(temp.path(), Vec::new()).unwrap();
        let first = snapshot_repository(temp.path(), "first".to_string(), None).unwrap();

        fs::write(temp.path().join("watched.txt"), b"changed before watch").unwrap();
        let mut watcher = RepositoryWatcher::start_with_reconciliation_interval(
            temp.path(),
            Duration::from_millis(50),
            Duration::from_millis(25),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(30));

        let second = watcher
            .poll("reconciliation", Some("watcher"))
            .unwrap()
            .expect("reconciliation did not run");
        assert!(second.created);
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
