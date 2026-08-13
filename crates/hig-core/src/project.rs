use crate::PathChunkRecord;
use crate::cache::CacheStore;
use crate::{BatchOptions, ChunkOptions, PipelineOptions, SolidMode};
use crossbeam_channel::{Receiver, TryRecvError, bounded};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const PROJECT_SCHEMA: u16 = 1;
const SNAPSHOT_MAGIC: &[u8; 4] = b"HPS1";
const JOURNAL_MAGIC: &[u8; 4] = b"HPJ1";
const MAX_STABLE_READ_RETRIES: usize = 3;
const SNAPSHOT_POLICY_SCHEMA: u16 = 1;

pub const DEFAULT_PROJECT_EXCLUDES: &[&str] = &[
    ".git",
    ".hig",
    ".hig-cache",
    "node_modules",
    ".next",
    "dist",
    "build",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    pub schema: u16,
    pub project_id: [u8; 16],
    pub cache_dir: Option<PathBuf>,
    pub excludes: Vec<String>,
    pub compression_policy_version: u16,
    #[serde(default)]
    pub snapshot_policy: WorkspaceSnapshotPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotResourcePolicy {
    pub enabled: bool,
    pub min_available_memory_bytes: u64,
    pub resume_available_memory_bytes: u64,
    pub poll_interval_ms: u64,
}

impl Default for SnapshotResourcePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            min_available_memory_bytes: 64 * 1024 * 1024,
            resume_available_memory_bytes: 128 * 1024 * 1024,
            poll_interval_ms: 250,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSnapshotPolicy {
    pub schema: u16,
    pub enabled: bool,
    pub quiescence_ms: u64,
    pub periodic_interval_ms: u64,
    pub max_pending_events: u64,
    pub max_pending_files: u64,
    pub resource: SnapshotResourcePolicy,
}

impl Default for WorkspaceSnapshotPolicy {
    fn default() -> Self {
        Self {
            schema: SNAPSHOT_POLICY_SCHEMA,
            enabled: true,
            quiescence_ms: 15,
            periodic_interval_ms: 15 * 60 * 1000,
            max_pending_events: 8192,
            max_pending_files: 4096,
            resource: SnapshotResourcePolicy::default(),
        }
    }
}

impl WorkspaceSnapshotPolicy {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema == SNAPSHOT_POLICY_SCHEMA,
            "unsupported workspace snapshot policy schema {}",
            self.schema
        );
        anyhow::ensure!(
            self.quiescence_ms <= 60_000,
            "snapshot quiescence must not exceed 60000 ms"
        );
        anyhow::ensure!(
            self.periodic_interval_ms == 0 || self.periodic_interval_ms >= 1000,
            "snapshot periodic interval must be zero or at least 1000 ms"
        );
        anyhow::ensure!(
            self.max_pending_events > 0 && self.max_pending_files > 0,
            "snapshot pending budgets must be greater than zero"
        );
        anyhow::ensure!(
            self.resource.poll_interval_ms > 0,
            "snapshot resource poll interval must be greater than zero"
        );
        anyhow::ensure!(
            !self.resource.enabled
                || self.resource.resume_available_memory_bytes
                    >= self.resource.min_available_memory_bytes,
            "snapshot resource resume threshold must be at least the pressure threshold"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotValidity {
    Building,
    Ready,
    Dirty,
    #[default]
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectFileRecord {
    pub relative_path: String,
    pub device_id: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i128,
    pub ctime_ns: i128,
    pub permissions: u32,
    pub content_hash: [u8; 32],
    pub chunks: Vec<PathChunkRecord>,
    pub prepared_objects: Vec<[u8; 32]>,
    pub solid_group_id: Option<[u8; 16]>,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub schema: u16,
    pub project_id: [u8; 16],
    pub generation: u64,
    pub event_sequence: u64,
    pub validity: SnapshotValidity,
    pub files: BTreeMap<String, ProjectFileRecord>,
}

impl ProjectSnapshot {
    pub fn empty(project_id: [u8; 16]) -> Self {
        Self {
            schema: PROJECT_SCHEMA,
            project_id,
            generation: 0,
            event_sequence: 0,
            validity: SnapshotValidity::Building,
            files: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectStatusReport {
    pub initialized: bool,
    pub project_id: Option<[u8; 16]>,
    pub root: String,
    pub cache_dir: String,
    pub snapshot_validity: SnapshotValidity,
    pub generation: u64,
    pub event_sequence: u64,
    pub files: u64,
    pub pending_events: u64,
    pub dirty_files: u64,
    pub dirty_groups: u64,
    pub watcher_backend: String,
    pub watcher_overflow_count: u64,
    pub prepared_bytes: u64,
    pub last_event_age_ms: u64,
    pub last_full_rebuild_unix_ns: i128,
    #[serde(default)]
    pub policy_schema: u16,
    #[serde(default)]
    pub snapshot_paused: bool,
    #[serde(default)]
    pub pause_reason: Option<String>,
    #[serde(default)]
    pub resource_available_memory_bytes: Option<u64>,
    #[serde(default)]
    pub resource_pressure: bool,
}

pub struct ProjectWatcher {
    root: PathBuf,
    cache_root: PathBuf,
    config: ProjectConfig,
    snapshot: ProjectSnapshot,
    watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    overflowed: Arc<AtomicBool>,
    pending: BTreeSet<PathBuf>,
    last_event: Option<Instant>,
    last_full_rebuild_unix_ns: i128,
    watcher_overflow_count: u64,
    prepared_bytes: u64,
    dirty_files: u64,
    dirty_groups: u64,
    policy: WorkspaceSnapshotPolicy,
    last_snapshot: Instant,
    pending_events_count: u64,
    snapshot_paused: bool,
    pause_reason: Option<String>,
    resource_available_memory_bytes: Option<u64>,
    resource_pressure: bool,
    last_resource_check: Option<Instant>,
}

impl ProjectWatcher {
    pub fn start(
        root: &Path,
        config: ProjectConfig,
        cache: &mut CacheStore,
        pipeline: PipelineOptions,
    ) -> anyhow::Result<Self> {
        let root = root.canonicalize()?;
        config.snapshot_policy.validate()?;
        let cache_root = resolve_project_cache_dir(&root, &config);
        let (sender, receiver) = bounded(
            config
                .snapshot_policy
                .max_pending_events
                .min(usize::MAX as u64) as usize,
        );
        let overflowed = Arc::new(AtomicBool::new(false));
        let callback_overflow = overflowed.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            if sender.try_send(event).is_err() {
                callback_overflow.store(true, Ordering::Release);
            }
        })?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        let snapshot = rebuild_snapshot(&root, &cache_root, &config)?;
        let (_, prepared_bytes) = crate::archive::prewarm_project_snapshot(
            &root,
            &snapshot,
            cache,
            BatchOptions::default(),
            ChunkOptions::default(),
            SolidMode::Auto,
            pipeline,
        )?;
        let (_, warmed_bytes) = cache.warm_parameterized_payloads()?;
        cache.save_with_options(crate::cache::CacheSaveOptions { refresh_l1: false })?;
        let policy = config.snapshot_policy.clone();
        Ok(Self {
            root,
            cache_root,
            config,
            snapshot,
            watcher,
            receiver,
            overflowed,
            pending: BTreeSet::new(),
            last_event: None,
            last_full_rebuild_unix_ns: now_unix_ns(),
            watcher_overflow_count: 0,
            prepared_bytes: prepared_bytes.max(warmed_bytes),
            dirty_files: 0,
            dirty_groups: 0,
            policy,
            last_snapshot: Instant::now(),
            pending_events_count: 0,
            snapshot_paused: false,
            pause_reason: None,
            resource_available_memory_bytes: None,
            resource_pressure: false,
            last_resource_check: None,
        })
    }

    pub fn project_id(&self) -> [u8; 16] {
        self.config.project_id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn snapshot(&self) -> &ProjectSnapshot {
        &self.snapshot
    }

    pub fn update_policy(&mut self, policy: WorkspaceSnapshotPolicy) -> anyhow::Result<()> {
        policy.validate()?;
        update_snapshot_policy(&self.root, policy.clone())?;
        self.policy = policy.clone();
        self.config.snapshot_policy = policy;
        Ok(())
    }

    pub fn poll(
        &mut self,
        cache: &mut CacheStore,
        pipeline: PipelineOptions,
    ) -> anyhow::Result<bool> {
        self.refresh_resource_state();
        let mut received = false;
        loop {
            match self.receiver.try_recv() {
                Ok(Ok(event)) => {
                    received = true;
                    self.pending_events_count = self.pending_events_count.saturating_add(1);
                    if self.pending_events_count > self.policy.max_pending_events {
                        self.watcher_overflow_count = self.watcher_overflow_count.saturating_add(1);
                        self.invalidate();
                        break;
                    }
                    if event.need_rescan() {
                        self.watcher_overflow_count = self.watcher_overflow_count.saturating_add(1);
                        self.invalidate();
                        break;
                    }
                    for path in event.paths {
                        if let Ok(relative) = path.strip_prefix(&self.root)
                            && !is_project_excluded(relative, &self.config.excludes)
                        {
                            self.pending.insert(path);
                        }
                    }
                    if self.pending.len() as u64 > self.policy.max_pending_files {
                        self.watcher_overflow_count = self.watcher_overflow_count.saturating_add(1);
                        self.invalidate();
                        break;
                    }
                }
                Ok(Err(_)) => {
                    self.watcher_overflow_count = self.watcher_overflow_count.saturating_add(1);
                    self.invalidate();
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.watcher_overflow_count = self.watcher_overflow_count.saturating_add(1);
                    self.invalidate();
                    break;
                }
            }
        }
        if self.overflowed.swap(false, Ordering::AcqRel) {
            self.watcher_overflow_count += 1;
            self.invalidate();
        }
        if received {
            self.last_event = Some(Instant::now());
            self.snapshot.validity = SnapshotValidity::Dirty;
        }
        if self.snapshot_paused || !self.policy.enabled {
            return Ok(false);
        }
        if self.periodic_due() && self.pending.is_empty() {
            self.rebuild(cache, pipeline)?;
            return Ok(true);
        }
        if self.snapshot.validity == SnapshotValidity::Invalid {
            self.rebuild(cache, pipeline)?;
            return Ok(true);
        }
        if self.pending.is_empty()
            || self.last_event.is_some_and(|last| {
                last.elapsed() < Duration::from_millis(self.policy.quiescence_ms)
            })
        {
            return Ok(false);
        }

        let paths = std::mem::take(&mut self.pending);
        let generation = self.snapshot.generation.saturating_add(1);
        let mut dirty = 0_u64;
        for path in paths {
            let relative = match path.strip_prefix(&self.root) {
                Ok(relative) => relative
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                Err(_) => continue,
            };
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    let record = stable_read_record(&self.root, &path, generation)?;
                    cache.upsert_path_record(crate::cache::PathCacheRecord {
                        relative_path: record.relative_path.clone(),
                        size: record.size,
                        mtime_ns: record.mtime_ns,
                        permissions: record.permissions,
                        content_hash: record.content_hash,
                        last_seen_unix_ns: now_unix_ns(),
                        chunk_size: None,
                        chunks: record.chunks.clone(),
                    })?;
                    append_project_journal(
                        &self.cache_root,
                        &self.config.project_id,
                        &ProjectJournalEntry::Upsert(record.clone()),
                    )?;
                    self.snapshot.files.insert(relative, record);
                    dirty += 1;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if self.snapshot.files.remove(&relative).is_some() {
                        append_project_journal(
                            &self.cache_root,
                            &self.config.project_id,
                            &ProjectJournalEntry::Delete(relative),
                        )?;
                        dirty += 1;
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        self.snapshot.generation = generation;
        self.snapshot.event_sequence = self.snapshot.event_sequence.saturating_add(1);
        self.snapshot.validity = SnapshotValidity::Ready;
        self.dirty_files = dirty;
        self.dirty_groups = dirty;
        self.pending_events_count = 0;
        self.last_snapshot = Instant::now();
        append_project_journal(
            &self.cache_root,
            &self.config.project_id,
            &ProjectJournalEntry::State {
                generation,
                event_sequence: self.snapshot.event_sequence,
                validity: SnapshotValidity::Ready,
            },
        )?;
        let _ = pipeline;
        cache.save_with_options(crate::cache::CacheSaveOptions { refresh_l1: false })?;
        Ok(true)
    }

    pub fn rebuild(
        &mut self,
        cache: &mut CacheStore,
        pipeline: PipelineOptions,
    ) -> anyhow::Result<()> {
        let previous_generation = self.snapshot.generation;
        let previous_event_sequence = self.snapshot.event_sequence;
        self.snapshot.validity = SnapshotValidity::Building;
        let mut rebuilt = rebuild_snapshot(&self.root, &self.cache_root, &self.config)?;
        rebuilt.generation = previous_generation
            .saturating_add(1)
            .max(rebuilt.generation);
        rebuilt.event_sequence = previous_event_sequence.saturating_add(1);
        rebuilt.validity = SnapshotValidity::Ready;
        save_snapshot(&self.cache_root, &rebuilt)?;
        self.snapshot = rebuilt;
        let (_, prepared_bytes) = crate::archive::prewarm_project_snapshot(
            &self.root,
            &self.snapshot,
            cache,
            BatchOptions::default(),
            ChunkOptions::default(),
            SolidMode::Auto,
            pipeline,
        )?;
        self.prepared_bytes = prepared_bytes;
        self.last_full_rebuild_unix_ns = now_unix_ns();
        self.last_snapshot = Instant::now();
        self.pending_events_count = 0;
        self.last_event = None;
        self.pending.clear();
        cache.save_with_options(crate::cache::CacheSaveOptions { refresh_l1: false })?;
        Ok(())
    }

    pub fn status(&self) -> ProjectStatusReport {
        let _keep_watcher_alive = &self.watcher;
        ProjectStatusReport {
            initialized: true,
            project_id: Some(self.config.project_id),
            root: self.root.display().to_string(),
            cache_dir: self.cache_root.display().to_string(),
            snapshot_validity: self.snapshot.validity,
            generation: self.snapshot.generation,
            event_sequence: self.snapshot.event_sequence,
            files: self.snapshot.files.len() as u64,
            pending_events: self.pending.len() as u64,
            dirty_files: self.dirty_files,
            dirty_groups: self.dirty_groups,
            watcher_backend: watcher_backend().to_string(),
            watcher_overflow_count: self.watcher_overflow_count,
            prepared_bytes: self.prepared_bytes,
            last_event_age_ms: self
                .last_event
                .map(|event| event.elapsed().as_millis() as u64)
                .unwrap_or_default(),
            last_full_rebuild_unix_ns: self.last_full_rebuild_unix_ns,
            policy_schema: self.policy.schema,
            snapshot_paused: self.snapshot_paused,
            pause_reason: self.pause_reason.clone(),
            resource_available_memory_bytes: self.resource_available_memory_bytes,
            resource_pressure: self.resource_pressure,
        }
    }

    fn invalidate(&mut self) {
        self.snapshot.validity = SnapshotValidity::Invalid;
        self.pending.clear();
    }

    fn periodic_due(&self) -> bool {
        self.policy.enabled
            && self.policy.periodic_interval_ms > 0
            && self.last_snapshot.elapsed()
                >= Duration::from_millis(self.policy.periodic_interval_ms)
    }

    fn refresh_resource_state(&mut self) {
        if !self.policy.resource.enabled
            || self.last_resource_check.is_some_and(|last| {
                last.elapsed() < Duration::from_millis(self.policy.resource.poll_interval_ms)
            })
        {
            return;
        }
        self.last_resource_check = Some(Instant::now());
        self.resource_available_memory_bytes = available_memory_bytes();
        let Some(available) = self.resource_available_memory_bytes else {
            self.resource_pressure = false;
            if self.snapshot_paused {
                self.snapshot_paused = false;
                self.pause_reason = None;
            }
            return;
        };
        if self.snapshot_paused {
            if available >= self.policy.resource.resume_available_memory_bytes {
                self.snapshot_paused = false;
                self.resource_pressure = false;
                self.pause_reason = None;
            }
        } else if available < self.policy.resource.min_available_memory_bytes {
            self.snapshot_paused = true;
            self.resource_pressure = true;
            self.pause_reason = Some("low_available_memory".to_string());
        } else {
            self.resource_pressure = false;
        }
    }

    #[cfg(test)]
    fn force_overflow(&self) {
        self.overflowed.store(true, Ordering::Release);
    }
}

fn watcher_backend() -> &'static str {
    #[cfg(target_os = "macos")]
    return "fsevents";
    #[cfg(target_os = "linux")]
    return "inotify";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return "unsupported";
}

fn available_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let pages = unsafe { libc::sysconf(libc::_SC_AVPHYS_PAGES) };
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if pages > 0 && page_size > 0 {
            return (pages as u64).checked_mul(page_size as u64);
        }
    }
    #[cfg(target_os = "macos")]
    {
        fn sysctl_u64(name: &str) -> Option<u64> {
            let name = std::ffi::CString::new(name).ok()?;
            let mut value = 0_u64;
            let mut length = std::mem::size_of_val(&value);
            let result = unsafe {
                libc::sysctlbyname(
                    name.as_ptr(),
                    (&mut value as *mut u64).cast(),
                    &mut length,
                    std::ptr::null_mut(),
                    0,
                )
            };
            (result == 0 && length == std::mem::size_of_val(&value)).then_some(value)
        }
        let pages = sysctl_u64("vm.stats.vm.v_free_count")?
            .saturating_add(sysctl_u64("vm.stats.vm.v_inactive_count")?)
            .saturating_add(sysctl_u64("vm.stats.vm.v_speculative_count")?);
        let page_size = sysctl_u64("hw.pagesize")?;
        pages.checked_mul(page_size)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn now_unix_ns() -> i128 {
    let now = SystemTime::now();
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectJournalEntry {
    Upsert(ProjectFileRecord),
    Delete(String),
    State {
        generation: u64,
        event_sequence: u64,
        validity: SnapshotValidity,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFingerprint {
    pub device_id: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i128,
    pub ctime_ns: i128,
    pub permissions: u32,
}

pub fn init_project(
    root: &Path,
    cache_dir: Option<PathBuf>,
    extra_excludes: Vec<String>,
) -> anyhow::Result<ProjectConfig> {
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let config_path = project_config_path(&root);
    if config_path.exists() {
        return load_project_config(&root);
    }
    fs::create_dir_all(root.join(".hig"))?;
    let mut excludes = DEFAULT_PROJECT_EXCLUDES
        .iter()
        .map(|value| (*value).to_string())
        .collect::<BTreeSet<_>>();
    excludes.extend(extra_excludes);
    let config = ProjectConfig {
        schema: PROJECT_SCHEMA,
        project_id: crate::random_bytes(),
        cache_dir,
        excludes: excludes.into_iter().collect(),
        compression_policy_version: 2,
        snapshot_policy: WorkspaceSnapshotPolicy::default(),
    };
    atomic_write_json(&config_path, &config)?;
    let cache_root = resolve_project_cache_dir(&root, &config);
    fs::create_dir_all(project_state_dir(&cache_root, &config.project_id))?;
    save_snapshot(&cache_root, &ProjectSnapshot::empty(config.project_id))?;
    Ok(config)
}

pub fn discover_project(start: &Path) -> anyhow::Result<Option<(PathBuf, ProjectConfig)>> {
    let mut current = if start.is_dir() {
        start.canonicalize()?
    } else {
        start
            .parent()
            .ok_or_else(|| anyhow::anyhow!("project path has no parent"))?
            .canonicalize()?
    };
    loop {
        if project_config_path(&current).exists() {
            let config = load_project_config(&current)?;
            return Ok(Some((current, config)));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

pub fn load_project_config(root: &Path) -> anyhow::Result<ProjectConfig> {
    let config: ProjectConfig = serde_json::from_slice(&fs::read(project_config_path(root))?)?;
    anyhow::ensure!(
        config.schema == PROJECT_SCHEMA,
        "unsupported Hig project schema {}",
        config.schema
    );
    config.snapshot_policy.validate()?;
    Ok(config)
}

pub fn update_snapshot_policy(
    root: &Path,
    policy: WorkspaceSnapshotPolicy,
) -> anyhow::Result<ProjectConfig> {
    let root = root.canonicalize()?;
    let mut config = load_project_config(&root)?;
    policy.validate()?;
    config.snapshot_policy = policy;
    atomic_write_json(&project_config_path(&root), &config)?;
    Ok(config)
}

pub fn resolve_project_cache_dir(root: &Path, config: &ProjectConfig) -> PathBuf {
    match config.cache_dir.as_ref() {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => root.join(path),
        None => root.join(".hig-cache"),
    }
}

pub fn rebuild_snapshot(
    root: &Path,
    cache_root: &Path,
    config: &ProjectConfig,
) -> anyhow::Result<ProjectSnapshot> {
    config.snapshot_policy.validate()?;
    let root = root.canonicalize()?;
    let paths = project_file_paths(&root, &config.excludes);
    let records = paths
        .into_par_iter()
        .map(|path| stable_read_record(&root, &path, 1))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let files = records
        .into_iter()
        .map(|record| (record.relative_path.clone(), record))
        .collect();
    let snapshot = ProjectSnapshot {
        schema: PROJECT_SCHEMA,
        project_id: config.project_id,
        generation: 1,
        event_sequence: 0,
        validity: SnapshotValidity::Ready,
        files,
    };
    save_snapshot(cache_root, &snapshot)?;
    truncate_project_journal(cache_root, &config.project_id)?;
    Ok(snapshot)
}

pub fn load_snapshot(cache_root: &Path, project_id: &[u8; 16]) -> anyhow::Result<ProjectSnapshot> {
    let path = snapshot_path(cache_root, project_id);
    let bytes = fs::read(&path)?;
    let mut snapshot: ProjectSnapshot = decode_checked(&bytes, SNAPSHOT_MAGIC)?;
    anyhow::ensure!(
        snapshot.project_id == *project_id,
        "project snapshot id mismatch"
    );
    replay_project_journal(cache_root, &mut snapshot)?;
    // A persisted snapshot is an optimization hint only. A new daemon must rebuild it.
    snapshot.validity = SnapshotValidity::Invalid;
    Ok(snapshot)
}

pub fn save_snapshot(cache_root: &Path, snapshot: &ProjectSnapshot) -> anyhow::Result<()> {
    let path = snapshot_path(cache_root, &snapshot.project_id);
    let bytes = encode_checked(snapshot, SNAPSHOT_MAGIC)?;
    atomic_write(&path, &bytes)
}

pub fn append_project_journal(
    cache_root: &Path,
    project_id: &[u8; 16],
    entry: &ProjectJournalEntry,
) -> anyhow::Result<()> {
    let path = project_state_dir(cache_root, project_id).join("project-journal.bin");
    fs::create_dir_all(path.parent().unwrap_or(cache_root))?;
    let bytes = encode_checked(entry, JOURNAL_MAGIC)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&(bytes.len() as u32).to_le_bytes())?;
    file.write_all(&bytes)?;
    file.flush()?;
    Ok(())
}

pub fn replay_project_journal(
    cache_root: &Path,
    snapshot: &mut ProjectSnapshot,
) -> anyhow::Result<u64> {
    let path = project_state_dir(cache_root, &snapshot.project_id).join("project-journal.bin");
    let Ok(mut file) = fs::File::open(path) else {
        return Ok(0);
    };
    let mut replayed = 0;
    loop {
        let mut len = [0_u8; 4];
        match file.read_exact(&mut len) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        let len = u32::from_le_bytes(len) as usize;
        anyhow::ensure!(len <= 16 * 1024 * 1024, "project journal entry too large");
        let mut bytes = vec![0_u8; len];
        match file.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        match decode_checked::<ProjectJournalEntry>(&bytes, JOURNAL_MAGIC)? {
            ProjectJournalEntry::Upsert(record) => {
                snapshot.files.insert(record.relative_path.clone(), record);
            }
            ProjectJournalEntry::Delete(path) => {
                snapshot.files.remove(&path);
            }
            ProjectJournalEntry::State {
                generation,
                event_sequence,
                validity,
            } => {
                snapshot.generation = generation;
                snapshot.event_sequence = event_sequence;
                snapshot.validity = validity;
            }
        }
        replayed += 1;
    }
    Ok(replayed)
}

pub fn stable_read_record(
    root: &Path,
    path: &Path,
    generation: u64,
) -> anyhow::Result<ProjectFileRecord> {
    for _ in 0..MAX_STABLE_READ_RETRIES {
        let before = fingerprint(&fs::metadata(path)?)?;
        let content = fs::read(path)?;
        let after = fingerprint(&fs::metadata(path)?)?;
        if before == after && before.size == content.len() as u64 {
            let relative_path = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let chunks = content
                .chunks(1_048_576)
                .enumerate()
                .map(|(index, chunk)| PathChunkRecord {
                    chunk_hash: *blake3::hash(chunk).as_bytes(),
                    file_offset: index as u64 * 1_048_576,
                    len: chunk.len() as u64,
                })
                .collect();
            return Ok(ProjectFileRecord {
                relative_path,
                device_id: after.device_id,
                inode: after.inode,
                size: after.size,
                mtime_ns: after.mtime_ns,
                ctime_ns: after.ctime_ns,
                permissions: after.permissions,
                content_hash: *blake3::hash(&content).as_bytes(),
                chunks,
                prepared_objects: Vec::new(),
                solid_group_id: None,
                generation,
            });
        }
    }
    anyhow::bail!(
        "file changed while preparing project snapshot: {}",
        path.display()
    )
}

pub fn verify_snapshot_metadata(root: &Path, snapshot: &ProjectSnapshot) -> anyhow::Result<bool> {
    if snapshot.validity != SnapshotValidity::Ready {
        return Ok(false);
    }
    snapshot
        .files
        .par_iter()
        .try_fold(
            || true,
            |valid, (relative, record)| {
                if !valid {
                    return Ok(false);
                }
                let metadata = match fs::metadata(root.join(relative)) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Err(error.into()),
                };
                Ok(fingerprint(&metadata)?
                    == FileFingerprint {
                        device_id: record.device_id,
                        inode: record.inode,
                        size: record.size,
                        mtime_ns: record.mtime_ns,
                        ctime_ns: record.ctime_ns,
                        permissions: record.permissions,
                    })
            },
        )
        .try_reduce(|| true, |left, right| Ok(left && right))
}

pub fn is_project_excluded(relative: &Path, excludes: &[String]) -> bool {
    relative.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        excludes.iter().any(|excluded| excluded == value.as_ref())
    })
}

pub fn project_config_path(root: &Path) -> PathBuf {
    root.join(".hig").join("project.json")
}

fn project_file_paths(root: &Path, excludes: &[String]) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.path() == root
                || entry
                    .path()
                    .strip_prefix(root)
                    .map(|relative| !is_project_excluded(relative, excludes))
                    .unwrap_or(false)
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect()
}

fn fingerprint(metadata: &fs::Metadata) -> anyhow::Result<FileFingerprint> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FileFingerprint {
            device_id: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime_ns: metadata.mtime() as i128 * 1_000_000_000 + metadata.mtime_nsec() as i128,
            ctime_ns: metadata.ctime() as i128 * 1_000_000_000 + metadata.ctime_nsec() as i128,
            permissions: metadata.mode(),
        })
    }
    #[cfg(not(unix))]
    {
        let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
        let created = metadata.created().unwrap_or(modified);
        Ok(FileFingerprint {
            device_id: 0,
            inode: 0,
            size: metadata.len(),
            mtime_ns: unix_ns(modified),
            ctime_ns: unix_ns(created),
            permissions: 0,
        })
    }
}

#[cfg(not(unix))]
fn unix_ns(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}

fn snapshot_path(cache_root: &Path, project_id: &[u8; 16]) -> PathBuf {
    project_state_dir(cache_root, project_id).join("snapshot.bin")
}

fn project_state_dir(cache_root: &Path, project_id: &[u8; 16]) -> PathBuf {
    cache_root.join("projects").join(hex::encode(project_id))
}

fn truncate_project_journal(cache_root: &Path, project_id: &[u8; 16]) -> anyhow::Result<()> {
    let path = project_state_dir(cache_root, project_id).join("project-journal.bin");
    if path.exists() {
        OpenOptions::new().write(true).truncate(true).open(path)?;
    }
    Ok(())
}

fn encode_checked<T: Serialize>(value: &T, magic: &[u8; 4]) -> anyhow::Result<Vec<u8>> {
    let payload = bincode::serialize(value)?;
    let mut bytes = Vec::with_capacity(4 + 2 + 8 + payload.len() + 32);
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&PROJECT_SCHEMA.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(blake3::hash(&payload).as_bytes());
    Ok(bytes)
}

fn decode_checked<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    magic: &[u8; 4],
) -> anyhow::Result<T> {
    anyhow::ensure!(bytes.len() >= 46, "project state is truncated");
    anyhow::ensure!(&bytes[..4] == magic, "invalid project state magic");
    let schema = u16::from_le_bytes(bytes[4..6].try_into()?);
    anyhow::ensure!(schema == PROJECT_SCHEMA, "unsupported project state schema");
    let len = u64::from_le_bytes(bytes[6..14].try_into()?) as usize;
    anyhow::ensure!(14 + len + 32 == bytes.len(), "invalid project state length");
    let payload = &bytes[14..14 + len];
    anyhow::ensure!(
        blake3::hash(payload).as_bytes() == &bytes[14 + len..],
        "project state checksum mismatch"
    );
    Ok(bincode::deserialize(payload)?)
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("project state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        hex::encode(crate::random_bytes::<8>())
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_init_and_snapshot_roundtrip_are_safe() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"hello").unwrap();
        let config = init_project(temp.path(), None, vec!["vendor".to_string()]).unwrap();
        assert!(config.excludes.contains(&".hig".to_string()));
        assert!(config.excludes.contains(&"vendor".to_string()));
        let cache = resolve_project_cache_dir(temp.path(), &config);
        let snapshot = rebuild_snapshot(temp.path(), &cache, &config).unwrap();
        assert_eq!(snapshot.validity, SnapshotValidity::Ready);
        assert_eq!(snapshot.files.len(), 1);
        let reopened = load_snapshot(&cache, &config.project_id).unwrap();
        assert_eq!(reopened.validity, SnapshotValidity::Invalid);
        assert_eq!(reopened.files.len(), 1);
    }

    #[test]
    fn journal_replay_is_idempotent_and_truncated_tail_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let config = init_project(temp.path(), None, Vec::new()).unwrap();
        let cache = resolve_project_cache_dir(temp.path(), &config);
        fs::write(temp.path().join("a.txt"), b"hello").unwrap();
        let record = stable_read_record(temp.path(), &temp.path().join("a.txt"), 1).unwrap();
        append_project_journal(
            &cache,
            &config.project_id,
            &ProjectJournalEntry::Upsert(record.clone()),
        )
        .unwrap();
        let journal = project_state_dir(&cache, &config.project_id).join("project-journal.bin");
        OpenOptions::new()
            .append(true)
            .open(&journal)
            .unwrap()
            .write_all(&100_u32.to_le_bytes())
            .unwrap();
        let mut snapshot = ProjectSnapshot::empty(config.project_id);
        assert_eq!(replay_project_journal(&cache, &mut snapshot).unwrap(), 1);
        assert_eq!(snapshot.files.get("a.txt"), Some(&record));
        assert_eq!(replay_project_journal(&cache, &mut snapshot).unwrap(), 1);
        assert_eq!(snapshot.files.len(), 1);
    }

    #[test]
    fn metadata_verification_detects_content_change_even_if_size_is_equal() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), b"hello").unwrap();
        let config = init_project(temp.path(), None, Vec::new()).unwrap();
        let cache = resolve_project_cache_dir(temp.path(), &config);
        let snapshot = rebuild_snapshot(temp.path(), &cache, &config).unwrap();
        assert!(verify_snapshot_metadata(temp.path(), &snapshot).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(temp.path().join("a.txt"), b"world").unwrap();
        assert!(!verify_snapshot_metadata(temp.path(), &snapshot).unwrap());
    }

    #[test]
    fn watcher_updates_generation_and_recovers_from_overflow() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let root = temp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        let config = init_project(&root, Some(cache_dir.clone()), Vec::new()).unwrap();
        let mut cache = CacheStore::open(&cache_dir).unwrap();
        let mut watcher =
            ProjectWatcher::start(&root, config, &mut cache, PipelineOptions::default()).unwrap();
        let initial_generation = watcher.snapshot().generation;
        fs::write(root.join("a.txt"), b"two").unwrap();
        std::thread::sleep(Duration::from_millis(60));
        watcher
            .poll(&mut cache, PipelineOptions::default())
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        watcher
            .poll(&mut cache, PipelineOptions::default())
            .unwrap();
        assert!(watcher.snapshot().generation > initial_generation);
        assert_eq!(watcher.snapshot().validity, SnapshotValidity::Ready);
        watcher.force_overflow();
        watcher
            .poll(&mut cache, PipelineOptions::default())
            .unwrap();
        assert_eq!(watcher.snapshot().validity, SnapshotValidity::Ready);
        assert_eq!(watcher.status().watcher_overflow_count, 1);
    }

    #[test]
    fn snapshot_policy_validates_bounds_and_legacy_config_uses_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let config = init_project(temp.path(), None, Vec::new()).unwrap();
        assert_eq!(config.snapshot_policy, WorkspaceSnapshotPolicy::default());

        let mut invalid = WorkspaceSnapshotPolicy {
            quiescence_ms: 60_001,
            ..WorkspaceSnapshotPolicy::default()
        };
        assert!(invalid.validate().is_err());
        invalid = WorkspaceSnapshotPolicy::default();
        invalid.resource.resume_available_memory_bytes = 1;
        assert!(invalid.validate().is_err());
        invalid = WorkspaceSnapshotPolicy::default();
        invalid.max_pending_files = 0;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn disabled_snapshot_policy_keeps_changes_dirty_until_explicit_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let root = temp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        let mut config = init_project(&root, Some(cache_dir.clone()), Vec::new()).unwrap();
        config.snapshot_policy.enabled = false;
        atomic_write_json(&project_config_path(&root), &config).unwrap();
        let mut cache = CacheStore::open(&cache_dir).unwrap();
        let mut watcher =
            ProjectWatcher::start(&root, config, &mut cache, PipelineOptions::default()).unwrap();
        let initial_generation = watcher.snapshot().generation;
        fs::write(root.join("a.txt"), b"two").unwrap();
        std::thread::sleep(Duration::from_millis(60));
        watcher
            .poll(&mut cache, PipelineOptions::default())
            .unwrap();
        assert_eq!(watcher.snapshot().generation, initial_generation);
        assert_eq!(watcher.snapshot().validity, SnapshotValidity::Dirty);
        watcher
            .rebuild(&mut cache, PipelineOptions::default())
            .unwrap();
        assert!(watcher.snapshot().generation > initial_generation);
        assert_eq!(watcher.snapshot().validity, SnapshotValidity::Ready);
    }

    #[test]
    fn full_rebuild_keeps_generation_and_event_sequence_monotonic() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let root = temp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        let config = init_project(&root, Some(cache_dir.clone()), Vec::new()).unwrap();
        let mut cache = CacheStore::open(&cache_dir).unwrap();
        let mut watcher =
            ProjectWatcher::start(&root, config, &mut cache, PipelineOptions::default()).unwrap();
        let initial = watcher.status();
        watcher
            .rebuild(&mut cache, PipelineOptions::default())
            .unwrap();
        let rebuilt = watcher.status();
        assert!(rebuilt.generation > initial.generation);
        assert!(rebuilt.event_sequence > initial.event_sequence);
    }
}
