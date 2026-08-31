mod adaptive_io;
mod archive;
mod cache;
mod codec;
mod crypto;
mod daemon;
mod desktop;
mod operation;
mod pipeline;
mod project;
mod recovery;
mod repository;
mod scan;
mod session;
mod task;
mod writer;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub use archive::{
    ArchiveMigrationReport, inspect_archive, migrate_archive, pack, pack_with_control, unpack,
    unpack_with_control,
};
pub use cache::{CacheMaintenanceReport, CacheStats, PathChunkRecord};
pub use crypto::{derive_key, random_bytes};
pub use daemon::{
    DaemonErrorCode, DaemonRequest, DaemonResponse, DaemonStatus, JobKeyMaterial, PackAuthMode,
    PackJobRequest, ProjectRegistration, SerializablePackOptions, cache_writer_available,
    daemon_socket_path, daemon_status, request_daemon, run_daemon_server, stop_daemon,
};
pub use desktop::{DesktopPackRequest, DesktopUnpackRequest};
pub use operation::{OperationControl, OperationKind, OperationPhase, OperationProgress};
pub use pipeline::{BufferPool, PipelineScheduler};
pub use project::{
    DEFAULT_PROJECT_EXCLUDES, ProjectConfig, ProjectFileRecord, ProjectJournalEntry,
    ProjectSnapshot, ProjectStatusReport, SnapshotResourcePolicy, SnapshotValidity,
    WorkspaceSnapshotPolicy, append_project_journal, discover_project, init_project,
    load_project_config, load_snapshot, rebuild_snapshot, resolve_project_cache_dir, save_snapshot,
    stable_read_record, update_snapshot_policy, verify_snapshot_metadata,
};
pub use recovery::{
    RecoveryAtRestPolicy, RecoveryCaptureReport, RecoveryDurability, RecoveryGcCandidate,
    RecoveryPinReport, RecoveryPoint, RecoveryPointState, RecoveryRegistration,
    RecoveryRegistrationReport, RecoveryRepairReport, RecoveryReplicaStatus, RecoveryRestoreReport,
    RecoveryRetentionPolicy, RecoveryScrubLocationReport, RecoveryScrubReport, RecoveryTombstone,
    RecoveryTombstoneKind, RecoveryTombstoneReport, RecoveryVaultConfig, RecoveryVaultGcReport,
    RecoveryVaultInitReport, RecoveryVaultListReport, RecoveryVerifyReport, capture_recovery_point,
    default_recovery_vault_root, gc_recovery_vault, init_recovery_vault, list_recovery_vault,
    record_recovery_tombstone, recovery_vault_config, register_recovery_repository,
    repair_recovery_point, restore_recovery_point, scrub_recovery_vault, set_recovery_point_pin,
    update_recovery_retention, verify_recovery_point,
};
pub use repository::{
    DEFAULT_REPOSITORY_EXCLUDES, RepositoryBranchReport, RepositoryByteRange,
    RepositoryCacheProvenance, RepositoryChange, RepositoryChangeKind, RepositoryCommitSummary,
    RepositoryConfig, RepositoryDiffReport, RepositoryGcReport, RepositoryInitReport,
    RepositoryMigrationReport, RepositoryObjectId, RepositoryPathHistoryEntry,
    RepositoryPathHistoryReport, RepositoryRangeRestoreReport, RepositoryRef,
    RepositoryRefDeleteReport, RepositoryRefKind, RepositoryRefsReport, RepositoryRestoreReport,
    RepositorySemanticChangeKind, RepositorySnapshotReport, RepositoryStoragePath,
    RepositoryStorageTreeReport, RepositorySymbol, RepositorySymbolHistoryEntry,
    RepositorySymbolHistoryReport, RepositorySymbolIndexReport, RepositorySymbolRestoreReport,
    RepositoryTagReport, RepositoryVerifyReport, RepositoryWatcher, create_repository_branch,
    create_repository_tag, delete_repository_branch, delete_repository_tag, gc_repository,
    init_repository, migrate_repository, repository_branch_names, repository_diff, repository_log,
    repository_path_history, repository_refs, repository_storage_tree, repository_symbol_history,
    repository_symbols, repository_tag_names, restore_repository, restore_repository_range,
    restore_repository_symbol, snapshot_repository, switch_repository_branch, verify_repository,
};
pub use scan::{ScanStats, ScannedFile};
pub use session::{SessionBinding, default_session_ttl, derive_session_binding};
pub use task::{
    DaemonTaskError, SerializableUnpackOptions, TaskManager, TaskRequest, TaskResult, TaskState,
    TaskStatusReport, TaskSubmitRequest, UnpackJobRequest,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveInspection {
    pub format: ArchiveFormat,
    pub encrypted: bool,
    pub files: Vec<ArchiveFileInfo>,
    pub input_bytes: u64,
    pub archive_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveFileInfo {
    pub relative_path: String,
    pub size: u64,
    pub modified_unix_ns: i128,
    pub permissions: u32,
    pub content_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    Zstd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadMemoryMode {
    #[default]
    Adaptive,
    Low,
}

impl std::str::FromStr for PayloadMemoryMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "adaptive" => Ok(Self::Adaptive),
            "low" => Ok(Self::Low),
            other => anyhow::bail!("unsupported payload memory mode: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoOptions {
    pub writer_buffer_bytes: usize,
    pub transfer_chunk_bytes: usize,
    pub prefetch_depth: usize,
    pub pipeline_memory_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineOptions {
    pub daemon_mode: DaemonMode,
    pub project_mode: ProjectMode,
    pub cpu_queue_small_first: bool,
    pub memory_budget_bytes: usize,
    pub io_prefetch_bytes: usize,
    pub cache_pack_enabled: bool,
    #[serde(default)]
    pub payload_memory_mode: PayloadMemoryMode,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            daemon_mode: DaemonMode::Auto,
            project_mode: ProjectMode::Auto,
            cpu_queue_small_first: true,
            memory_budget_bytes: 128 * 1024 * 1024,
            io_prefetch_bytes: 4 * 1024 * 1024,
            cache_pack_enabled: true,
            payload_memory_mode: PayloadMemoryMode::Adaptive,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectMode {
    #[default]
    Auto,
    Off,
    Required,
}

impl std::str::FromStr for ProjectMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "required" => Ok(Self::Required),
            other => anyhow::bail!("unsupported project mode: {other}"),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub timings_us: PackTimingsUs,
    #[serde(default)]
    pub write_profile: WriteProfileReport,
    #[serde(default)]
    pub project: ProjectPackReport,
    #[serde(default)]
    pub adaptive_io: AdaptiveIoReport,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdaptiveIoReport {
    pub enabled: bool,
    pub initial_concurrency: usize,
    pub min_concurrency: usize,
    pub max_concurrency: usize,
    pub final_concurrency: usize,
    pub min_observed_concurrency: usize,
    pub max_observed_concurrency: usize,
    pub transitions: u64,
    pub constrained_entries: u64,
    pub recovery_steps: u64,
    pub normal_us: u64,
    pub constrained_us: u64,
    pub total_us: u64,
    pub final_constraint_stage: Option<String>,
    pub final_constraint_direction: Option<String>,
    pub stages: BTreeMap<String, AdaptiveIoStageReport>,
    #[serde(default)]
    pub transition_events: Vec<AdaptiveIoTransitionReport>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdaptiveIoStageReport {
    pub samples: u64,
    pub bytes: u64,
    pub io_us: u64,
    pub wait_us: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdaptiveIoTransitionReport {
    pub at_us: u64,
    pub stage: String,
    pub direction: String,
    pub reason: String,
    pub from_concurrency: usize,
    pub to_concurrency: usize,
    pub throughput_mib_s: f64,
    pub small_io_p95_us: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WriteProfileReport {
    pub temp_create_us: u64,
    pub preallocate_us: u64,
    pub header_write_us: u64,
    pub manifest_write_us: u64,
    pub payload_read_us: u64,
    pub payload_write_us: u64,
    pub payload_memory_write_us: u64,
    pub payload_cached_write_us: u64,
    pub direct_write_us: u64,
    pub buffered_write_us: u64,
    pub writer_wait_us: u64,
    pub flush_us: u64,
    pub fsync_us: u64,
    pub rename_us: u64,
    pub memory_payload_count: usize,
    pub memory_payload_bytes: u64,
    pub cached_file_payload_count: usize,
    pub cached_file_payload_bytes: u64,
    pub cached_range_payload_count: usize,
    pub cached_range_payload_bytes: u64,
    pub direct_write_count: usize,
    pub buffered_write_count: usize,
    #[serde(default)]
    pub coalesced_write_count: usize,
    #[serde(default)]
    pub coalesced_payload_count: usize,
    #[serde(default)]
    pub coalesced_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectPackReport {
    pub project_mode_used: bool,
    pub project_generation: u64,
    pub project_snapshot_valid: bool,
    pub project_metadata_verified_files: u64,
    pub project_hash_reuses: u64,
    pub project_prepared_object_hits: u64,
    pub project_prepared_object_misses: u64,
    pub project_dirty_files: u64,
    pub project_dirty_groups: u64,
    pub project_verify_us: u64,
    pub project_freeze_us: u64,
    pub project_retry_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackTimingsUs {
    pub total_us: u64,
    pub daemon_connect_us: u64,
    pub socket_request_us: u64,
    pub socket_connect_us: u64,
    pub socket_pack_roundtrip_us: u64,
    pub daemon_auth_us: u64,
    pub daemon_job_execute_us: u64,
    pub daemon_response_bytes: u64,
    pub client_decode_us: u64,
    pub queue_wait_us: u64,
    pub cache_hot_lookup_us: u64,
    pub walk_us: u64,
    pub metadata_us: u64,
    pub hash_us: u64,
    pub plan_us: u64,
    pub read_us: u64,
    pub compression_us: u64,
    pub crypto_us: u64,
    pub cache_commit_wait_us: u64,
    pub cache_commit_us: u64,
    pub manifest_serialize_us: u64,
    pub manifest_compress_us: u64,
    pub manifest_encrypt_us: u64,
    pub output_create_us: u64,
    pub output_preallocate_us: u64,
    pub output_header_write_us: u64,
    pub output_manifest_write_us: u64,
    pub output_payload_read_us: u64,
    pub output_payload_write_us: u64,
    pub output_write_us: u64,
    pub output_flush_us: u64,
    pub output_fsync_us: u64,
    pub output_rename_us: u64,
    pub response_serialize_us: u64,
    pub unattributed_us: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackResponseMode {
    #[default]
    Summary,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackSummary {
    pub input_files: usize,
    pub input_bytes: u64,
    pub archive_bytes: u64,
    pub duration_us: u64,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub cache_hit_rate: f64,
    pub scan_cache_hit_rate: f64,
    pub encryption_mode: EncryptionMode,
    pub speed: SpeedMode,
    pub kdf_profile: KdfProfile,
    pub session_used: bool,
    pub cache_index_commit_ms: u128,
    pub cache_commit_mode: String,
    pub cache_shards_written: usize,
    pub solid_groups: usize,
    pub solid_files: usize,
    pub cache_policy_misses: usize,
    pub timings_us: PackTimingsUs,
    #[serde(default)]
    pub project: ProjectPackReport,
    #[serde(default)]
    pub adaptive_io: AdaptiveIoReport,
}

impl From<&PackReport> for PackSummary {
    fn from(report: &PackReport) -> Self {
        Self {
            input_files: report.input_files,
            input_bytes: report.input_bytes,
            archive_bytes: report.archive_bytes,
            duration_us: report.duration.as_micros() as u64,
            cache_hits: report.cache.hits,
            cache_misses: report.cache.misses,
            cache_hit_rate: report.cache.hit_rate() * 100.0,
            scan_cache_hit_rate: report.scan.scan_cache_hit_rate() * 100.0,
            encryption_mode: report.encryption_mode,
            speed: report.speed,
            kdf_profile: report.kdf_profile,
            session_used: report.session.session_used,
            cache_index_commit_ms: report.l2.cache_index_commit_ms,
            cache_commit_mode: report.l2.cache_commit_mode.clone(),
            cache_shards_written: report.l2.cache_shards_written,
            solid_groups: report.blocks.solid_groups,
            solid_files: report.blocks.solid_files,
            cache_policy_misses: report.blocks.cache_policy_misses,
            timings_us: report.timings_us.clone(),
            project: report.project.clone(),
            adaptive_io: report.adaptive_io.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionReport {
    pub session_used: bool,
    pub session_lookup_ms: u128,
    pub session_key_age_secs: u64,
    pub kdf_skipped_by_session: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct L1CacheReport {
    pub l1_index_hits: usize,
    pub l1_metadata_hits: usize,
    pub l1_scratch_reuses: usize,
    pub rayon_pool_reused: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct L2CacheReport {
    pub cache_index_format: String,
    pub cache_index_open_ms: u128,
    pub cache_index_commit_ms: u128,
    pub cache_shards_read: usize,
    pub cache_shards_written: usize,
    pub cache_shard_dirty_count: usize,
    pub cache_commit_mode: String,
    pub journal_upsert_records: u64,
    pub journal_upsert_paths: u64,
    pub journal_upsert_objects: u64,
    pub journal_upsert_sealed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArchiveSizeBreakdown {
    pub header_bytes: u64,
    pub manifest_plain_bytes: u64,
    pub manifest_compressed_bytes: u64,
    pub manifest_protected_bytes: u64,
    pub payload_bytes: u64,
    pub total_archive_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    pub output_preallocate_ms: u128,
    pub output_header_write_ms: u128,
    pub output_manifest_write_ms: u128,
    pub output_flush_ms: u128,
    pub output_fsync_ms: u128,
    pub output_rename_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    pub payload_source_spool_payloads: usize,
    pub payload_source_spool_bytes: u64,
    #[serde(default)]
    pub source_read_bytes: u64,
    #[serde(default)]
    pub source_hot_raw_bytes: u64,
    #[serde(default)]
    pub payload_memory_mode: PayloadMemoryMode,
    #[serde(default)]
    pub payload_memory_budget_bytes: u64,
    #[serde(default)]
    pub payload_memory_available_bytes: u64,
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
