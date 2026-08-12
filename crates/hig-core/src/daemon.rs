use crate::archive::{PackKeyOverride, pack_with_engine_cache};
use crate::cache::{CacheMaintenanceReport, CacheStore};
use crate::crypto::{KEY_LEN, SALT_LEN};
use crate::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, EncryptionMode, KdfProfile,
    ManifestFormat, PackOptions, PackReport, PackResponseMode, PackSummary, PipelineOptions,
    SessionBinding, SolidMode, SpeedMode,
};
use crate::{BufferPool, PipelineScheduler};
use crate::{
    DaemonTaskError, TaskManager, TaskRequest, TaskResult, TaskState, TaskStatusReport,
    TaskSubmitRequest,
};
use crate::{ProjectConfig, ProjectStatusReport};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zeroize::Zeroize;

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

const PROTOCOL_VERSION: u16 = 4;
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub active: bool,
    pub age_secs: u64,
    pub uptime_secs: u64,
    pub ttl_secs: u64,
    pub jobs_completed: u64,
    pub active_jobs: u64,
    pub queued_jobs: u64,
    pub cache_open_count: u64,
    pub cache_dir: String,
    pub journal_bytes: u64,
    pub session_active: bool,
    pub session_age_secs: u64,
    pub session_binding_fingerprint: Option<[u8; 32]>,
    #[serde(default)]
    pub watched_projects: u64,
    #[serde(default)]
    pub project_ready_count: u64,
    #[serde(default)]
    pub project_pending_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobKeyMaterial {
    pub key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
}

impl Drop for JobKeyMaterial {
    fn drop(&mut self) {
        self.key.zeroize();
        self.salt.zeroize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializablePackOptions {
    pub input_dir: PathBuf,
    pub output_file: PathBuf,
    pub encryption: EncryptionMode,
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
    pub solid: SolidMode,
    pub pipeline: PipelineOptions,
}

impl SerializablePackOptions {
    pub fn from_pack(options: &PackOptions) -> Self {
        Self {
            input_dir: options.input_dir.clone(),
            output_file: options.output_file.clone(),
            encryption: options.encryption,
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: options.use_cache,
            trust_metadata: options.trust_metadata,
            format: options.format,
            batch: options.batch,
            chunk: options.chunk,
            speed: options.speed,
            kdf_profile: options.kdf_profile,
            sealed_cache: options.sealed_cache,
            manifest_format: options.manifest_format,
            use_session: options.use_session,
            session_required: options.session_required,
            solid: options.solid,
            pipeline: options.pipeline,
        }
    }

    fn into_pack(self, cache_dir: PathBuf) -> PackOptions {
        PackOptions {
            input_dir: self.input_dir,
            output_file: self.output_file,
            password: None,
            encryption: self.encryption,
            cache_dir: Some(cache_dir),
            threads: self.threads,
            compression: self.compression,
            level: self.level,
            use_cache: self.use_cache,
            trust_metadata: self.trust_metadata,
            format: self.format,
            batch: self.batch,
            chunk: self.chunk,
            speed: self.speed,
            kdf_profile: self.kdf_profile,
            sealed_cache: self.sealed_cache,
            manifest_format: self.manifest_format,
            use_session: false,
            session_required: false,
            session_ttl_secs: None,
            solid: self.solid,
            pipeline: PipelineOptions {
                daemon_mode: crate::DaemonMode::Off,
                ..self.pipeline
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackJobRequest {
    pub options: SerializablePackOptions,
    pub binding_fingerprint: Option<[u8; 32]>,
    pub ephemeral_key: Option<JobKeyMaterial>,
    pub auth_mode: PackAuthMode,
    #[serde(default)]
    pub response_mode: PackResponseMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackAuthMode {
    UseSession,
    PreferSessionOrJobKey,
    JobKeyOnly,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonErrorCode {
    NoSession,
    BindingMismatch,
    ProtocolMismatch,
    CacheLocked,
    UnsupportedFormat,
    AuthRequired,
    DaemonUnavailable,
    StaleSocketRecovered,
    CorruptedCacheJournal,
    OutputPathBusy,
    ProjectNotInitialized,
    ProjectSnapshotBuilding,
    ProjectSnapshotInvalid,
    ProjectChangedDuringPack,
    WatcherUnavailable,
    WatcherOverflow,
    ProjectAlreadyWatched,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRegistration {
    pub root: PathBuf,
    pub config: ProjectConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonRequest {
    Status,
    Stop,
    UnlockChallenge {
        binding: SessionBinding,
    },
    InstallSessionKey {
        binding: SessionBinding,
        key: [u8; KEY_LEN],
        salt: [u8; SALT_LEN],
        ttl_secs: u64,
    },
    ClearSession,
    ProjectRegister(ProjectRegistration),
    ProjectStatus {
        project_id: [u8; 16],
    },
    ProjectRebuild {
        project_id: [u8; 16],
    },
    ProjectWatchForeground(ProjectRegistration),
    SubmitTask(TaskSubmitRequest),
    TaskStatus {
        task_id: [u8; 16],
    },
    TaskCancel {
        task_id: [u8; 16],
    },
    TaskResult {
        task_id: [u8; 16],
    },
    TaskList {
        include_completed: bool,
    },
    Pack(PackJobRequest),
    CacheStatus,
    CacheGc {
        dry_run: bool,
    },
    CacheCompact {
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    Status(DaemonStatus),
    UnlockChallenge {
        salt: [u8; SALT_LEN],
    },
    SessionInstalled,
    SessionCleared,
    PackComplete(Box<PackReport>),
    PackSummary(Box<PackSummary>),
    ProjectStatus(ProjectStatusReport),
    ProjectRegistered(ProjectStatusReport),
    TaskAccepted(TaskStatusReport),
    TaskStatus(TaskStatusReport),
    TaskResult(TaskResult),
    TaskList(Vec<TaskStatusReport>),
    CacheMaintenance(CacheMaintenanceReport),
    Stopped,
    Error {
        code: DaemonErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProtocolEnvelope<T> {
    protocol_version: u16,
    request_id: [u8; 16],
    payload: T,
}

struct SessionState {
    binding: SessionBinding,
    key: [u8; KEY_LEN],
    salt: [u8; SALT_LEN],
    created: Instant,
    ttl_secs: u64,
}

impl SessionState {
    fn active(&self) -> bool {
        self.created.elapsed().as_secs() < self.ttl_secs
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        self.key.zeroize();
        self.salt.zeroize();
    }
}

struct DaemonRuntime {
    cache_dir: PathBuf,
    engine: PackEngine,
    session: Option<SessionState>,
    started: Instant,
    ttl_secs: u64,
    jobs_completed: u64,
    active_outputs: BTreeSet<PathBuf>,
    projects: BTreeMap<[u8; 16], crate::project::ProjectWatcher>,
    tasks: TaskManager,
    stop: bool,
}

struct PackEngine {
    cache: CacheStore,
    rayon_pool: rayon::ThreadPool,
    buffers: BufferPool,
    scheduler: PipelineScheduler,
    cache_open_count: u64,
}

impl PackEngine {
    fn open(cache_dir: &Path) -> anyhow::Result<Self> {
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        Ok(Self {
            cache: CacheStore::open(cache_dir)?,
            rayon_pool: rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()?,
            buffers: BufferPool::new(256 * 1024 * 1024),
            scheduler: PipelineScheduler::new(true),
            cache_open_count: 1,
        })
    }

    fn execute(
        &mut self,
        cache_dir: &Path,
        options: SerializablePackOptions,
        key_override: Option<PackKeyOverride>,
        project_snapshot: Option<&crate::ProjectSnapshot>,
    ) -> anyhow::Result<PackReport> {
        let _scheduler = &self.scheduler;
        let mut report = self.rayon_pool.install(|| {
            pack_with_engine_cache(
                options.into_pack(cache_dir.to_path_buf()),
                &mut self.cache,
                key_override,
                project_snapshot,
            )
        })?;
        report.pipeline.buffer_pool_hits = self.buffers.hits();
        report.pipeline.buffer_pool_misses = self.buffers.misses();
        report.pipeline.pipeline_peak_memory_bytes = self.buffers.peak_bytes();
        report.pipeline.hot_index_reuses = 1;
        Ok(report)
    }
}

impl DaemonRuntime {
    fn status(&self) -> DaemonStatus {
        DaemonStatus {
            active: !self.stop,
            age_secs: self.started.elapsed().as_secs(),
            uptime_secs: self.started.elapsed().as_secs(),
            ttl_secs: self.ttl_secs,
            jobs_completed: self.jobs_completed,
            active_jobs: self.active_outputs.len() as u64,
            queued_jobs: self
                .tasks
                .list(false)
                .into_iter()
                .filter(|task| task.state == TaskState::Queued)
                .count() as u64,
            cache_open_count: self.engine.cache_open_count,
            cache_dir: self.cache_dir.display().to_string(),
            journal_bytes: self
                .engine
                .cache
                .maintenance_status()
                .map(|report| report.journal_bytes)
                .unwrap_or_default(),
            session_active: self.session.as_ref().is_some_and(SessionState::active),
            session_age_secs: self
                .session
                .as_ref()
                .map(|session| session.created.elapsed().as_secs())
                .unwrap_or(0),
            session_binding_fingerprint: self
                .session
                .as_ref()
                .filter(|session| session.active())
                .map(|session| session.binding.fingerprint),
            watched_projects: self.projects.len() as u64,
            project_ready_count: self
                .projects
                .values()
                .filter(|project| project.snapshot().validity == crate::SnapshotValidity::Ready)
                .count() as u64,
            project_pending_events: self
                .projects
                .values()
                .map(|project| project.status().pending_events)
                .sum(),
        }
    }

    fn poll_projects(&mut self) {
        for project in self.projects.values_mut() {
            if project
                .poll(&mut self.engine.cache, PipelineOptions::default())
                .is_err()
            {
                // Project status remains non-ready and the pack path will safely fall back.
            }
        }
    }

    fn handle(&mut self, request: DaemonRequest) -> anyhow::Result<DaemonResponse> {
        match request {
            DaemonRequest::Status => Ok(DaemonResponse::Status(self.status())),
            DaemonRequest::Stop => {
                self.stop = true;
                self.session = None;
                Ok(DaemonResponse::Stopped)
            }
            DaemonRequest::UnlockChallenge { .. } => Ok(DaemonResponse::UnlockChallenge {
                salt: crate::crypto::random_bytes(),
            }),
            DaemonRequest::InstallSessionKey {
                binding,
                key,
                salt,
                ttl_secs,
            } => {
                anyhow::ensure!(
                    binding.cache_dir == canonical_string(&self.cache_dir),
                    "session cache binding mismatch"
                );
                self.session = Some(SessionState {
                    binding,
                    key,
                    salt,
                    created: Instant::now(),
                    ttl_secs: crate::default_session_ttl(Some(ttl_secs)),
                });
                Ok(DaemonResponse::SessionInstalled)
            }
            DaemonRequest::ClearSession => {
                self.session = None;
                Ok(DaemonResponse::SessionCleared)
            }
            DaemonRequest::ProjectRegister(registration)
            | DaemonRequest::ProjectWatchForeground(registration) => {
                if let Some(project) = self.projects.get(&registration.config.project_id) {
                    return Ok(DaemonResponse::ProjectRegistered(project.status()));
                }
                let expected_cache =
                    crate::resolve_project_cache_dir(&registration.root, &registration.config);
                let expected_cache = expected_cache.canonicalize().unwrap_or(expected_cache);
                anyhow::ensure!(
                    expected_cache == self.cache_dir,
                    "project cache binding does not match daemon cache directory"
                );
                let watcher = crate::project::ProjectWatcher::start(
                    &registration.root,
                    registration.config,
                    &mut self.engine.cache,
                    PipelineOptions::default(),
                )?;
                let status = watcher.status();
                self.projects.insert(watcher.project_id(), watcher);
                Ok(DaemonResponse::ProjectRegistered(status))
            }
            DaemonRequest::ProjectStatus { project_id } => {
                let project = self
                    .projects
                    .get_mut(&project_id)
                    .ok_or_else(|| anyhow::anyhow!("project is not registered with this daemon"))?;
                project.poll(&mut self.engine.cache, PipelineOptions::default())?;
                Ok(DaemonResponse::ProjectStatus(project.status()))
            }
            DaemonRequest::ProjectRebuild { project_id } => {
                let project = self
                    .projects
                    .get_mut(&project_id)
                    .ok_or_else(|| anyhow::anyhow!("project is not registered with this daemon"))?;
                project.rebuild(&mut self.engine.cache, PipelineOptions::default())?;
                Ok(DaemonResponse::ProjectStatus(project.status()))
            }
            DaemonRequest::SubmitTask(submission) => {
                let (task_id, cancellation) = self.tasks.submit(&submission.request)?;
                self.tasks.mark_running(task_id);
                let control = crate::OperationControl::new(
                    task_id,
                    submission.request.kind(),
                    cancellation,
                    Arc::new(|_| {}),
                );
                let result = self.execute_task(submission.request, &control);
                self.tasks.complete(task_id, result);
                Ok(DaemonResponse::TaskAccepted(self.tasks.status(task_id)?))
            }
            DaemonRequest::TaskStatus { task_id } => {
                Ok(DaemonResponse::TaskStatus(self.tasks.status(task_id)?))
            }
            DaemonRequest::TaskCancel { task_id } => {
                Ok(DaemonResponse::TaskStatus(self.tasks.cancel(task_id)?))
            }
            DaemonRequest::TaskResult { task_id } => {
                Ok(DaemonResponse::TaskResult(self.tasks.result(task_id)?))
            }
            DaemonRequest::TaskList { include_completed } => {
                Ok(DaemonResponse::TaskList(self.tasks.list(include_completed)))
            }
            DaemonRequest::CacheStatus => Ok(DaemonResponse::CacheMaintenance(
                self.engine.cache.maintenance_status()?,
            )),
            DaemonRequest::CacheGc { dry_run } => Ok(DaemonResponse::CacheMaintenance(
                self.engine.cache.gc(dry_run)?,
            )),
            DaemonRequest::CacheCompact { dry_run } => Ok(DaemonResponse::CacheMaintenance(
                self.engine.cache.compact_sealed(dry_run)?,
            )),
            DaemonRequest::Pack(mut request) => {
                let auth_started = Instant::now();
                let output_path = request
                    .options
                    .output_file
                    .canonicalize()
                    .unwrap_or_else(|_| request.options.output_file.clone());
                if self.active_outputs.contains(&output_path) {
                    return Ok(DaemonResponse::Error {
                        code: DaemonErrorCode::OutputPathBusy,
                        message: "another daemon pack job is already writing this output path"
                            .to_string(),
                    });
                }
                self.active_outputs.insert(output_path.clone());
                if request.options.format != ArchiveFormat::HigV2 {
                    self.active_outputs.remove(&output_path);
                    return Ok(DaemonResponse::Error {
                        code: DaemonErrorCode::UnsupportedFormat,
                        message: "daemon supports HIGV2 only".to_string(),
                    });
                }
                let key_override = match request.options.encryption {
                    EncryptionMode::None => None,
                    EncryptionMode::Password => match request.auth_mode {
                        PackAuthMode::None => {
                            self.active_outputs.remove(&output_path);
                            return Ok(DaemonResponse::Error {
                                code: DaemonErrorCode::AuthRequired,
                                message: "password encryption requires a session or job key"
                                    .to_string(),
                            });
                        }
                        PackAuthMode::JobKeyOnly => {
                            if let Some(key) = request.ephemeral_key.take() {
                                Some(PackKeyOverride {
                                    key: key.key,
                                    salt: key.salt,
                                    age_secs: 0,
                                    session: false,
                                })
                            } else {
                                self.active_outputs.remove(&output_path);
                                return Ok(DaemonResponse::Error {
                                    code: DaemonErrorCode::AuthRequired,
                                    message: "job key was required but not provided".to_string(),
                                });
                            }
                        }
                        PackAuthMode::UseSession | PackAuthMode::PreferSessionOrJobKey => {
                            let session = self.session.as_ref().filter(|session| {
                                session.active()
                                    && request.binding_fingerprint
                                        == Some(session.binding.fingerprint)
                            });
                            if let Some(session) = session {
                                Some(PackKeyOverride {
                                    key: session.key,
                                    salt: session.salt,
                                    age_secs: session.created.elapsed().as_secs(),
                                    session: true,
                                })
                            } else if request.auth_mode == PackAuthMode::PreferSessionOrJobKey
                                && let Some(key) = request.ephemeral_key.take()
                            {
                                Some(PackKeyOverride {
                                    key: key.key,
                                    salt: key.salt,
                                    age_secs: 0,
                                    session: false,
                                })
                            } else if self.session.as_ref().is_some_and(SessionState::active) {
                                self.active_outputs.remove(&output_path);
                                return Ok(DaemonResponse::Error {
                                    code: DaemonErrorCode::BindingMismatch,
                                    message:
                                        "daemon session binding does not match this pack request"
                                            .to_string(),
                                });
                            } else {
                                self.active_outputs.remove(&output_path);
                                return Ok(DaemonResponse::Error {
                                    code: DaemonErrorCode::NoSession,
                                    message: "no matching daemon session; run `hig session unlock`"
                                        .to_string(),
                                });
                            }
                        }
                    },
                };
                let daemon_auth_us = auth_started.elapsed().as_micros() as u64;
                let input_root = request
                    .options
                    .input_dir
                    .canonicalize()
                    .unwrap_or_else(|_| request.options.input_dir.clone());
                let mut project_id = self
                    .projects
                    .iter()
                    .find(|(_, project)| project.root() == input_root)
                    .map(|(id, _)| *id);
                if project_id.is_none()
                    && request.options.pipeline.project_mode != crate::ProjectMode::Off
                    && let Some((root, config)) = crate::discover_project(&input_root)?
                {
                    let expected_cache = crate::resolve_project_cache_dir(&root, &config)
                        .canonicalize()
                        .unwrap_or_else(|_| crate::resolve_project_cache_dir(&root, &config));
                    if expected_cache == self.cache_dir {
                        let watcher = crate::project::ProjectWatcher::start(
                            &root,
                            config,
                            &mut self.engine.cache,
                            request.options.pipeline,
                        )?;
                        project_id = Some(watcher.project_id());
                        self.projects.insert(watcher.project_id(), watcher);
                    }
                }
                let project_required =
                    request.options.pipeline.project_mode == crate::ProjectMode::Required;
                if project_required && project_id.is_none() {
                    self.active_outputs.remove(&output_path);
                    return Ok(DaemonResponse::Error {
                        code: DaemonErrorCode::ProjectNotInitialized,
                        message: "project mode is required but this directory is not registered"
                            .to_string(),
                    });
                }
                let mut retry_count = 0_u32;
                let job_started = Instant::now();
                let mut report = loop {
                    let verify_started = Instant::now();
                    let freeze_started = Instant::now();
                    let project_snapshot = if request.options.pipeline.project_mode
                        == crate::ProjectMode::Off
                    {
                        None
                    } else if let Some(id) = project_id {
                        let project = self.projects.get(&id).expect("project id exists");
                        if project.snapshot().validity == crate::SnapshotValidity::Ready
                            && crate::verify_snapshot_metadata(project.root(), project.snapshot())?
                        {
                            Some(project.snapshot().clone())
                        } else if project_required {
                            self.active_outputs.remove(&output_path);
                            return Ok(DaemonResponse::Error {
                                code: DaemonErrorCode::ProjectSnapshotInvalid,
                                message:
                                    "project snapshot is not ready or metadata verification failed"
                                        .to_string(),
                            });
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let project_freeze_us = freeze_started.elapsed().as_micros() as u64;
                    let project_verify_us = verify_started.elapsed().as_micros() as u64;
                    let generation = project_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.generation);
                    let result = self.engine.execute(
                        &self.cache_dir,
                        request.options.clone(),
                        key_override.clone(),
                        project_snapshot.as_ref(),
                    );
                    let mut candidate = match result {
                        Ok(report) => report,
                        Err(error) => {
                            self.active_outputs.remove(&output_path);
                            return Err(error);
                        }
                    };
                    self.poll_projects();
                    let generation_stable = match (project_id, generation) {
                        (Some(id), Some(expected)) => {
                            self.projects.get(&id).is_some_and(|project| {
                                project.snapshot().validity == crate::SnapshotValidity::Ready
                                    && project.snapshot().generation == expected
                            })
                        }
                        _ => true,
                    };
                    candidate.project.project_verify_us = project_verify_us;
                    candidate.project.project_freeze_us = project_freeze_us;
                    candidate.project.project_retry_count = retry_count;
                    if let Some(id) = project_id
                        && let Some(project) = self.projects.get(&id)
                    {
                        let status = project.status();
                        candidate.project.project_dirty_files = status.dirty_files;
                        candidate.project.project_dirty_groups = status.dirty_groups;
                    }
                    if generation_stable {
                        break candidate;
                    }
                    let _ = fs::remove_file(&output_path);
                    if retry_count >= 2 {
                        self.active_outputs.remove(&output_path);
                        return Ok(DaemonResponse::Error {
                            code: DaemonErrorCode::ProjectChangedDuringPack,
                            message: "project changed during pack after two retries".to_string(),
                        });
                    }
                    retry_count += 1;
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    self.poll_projects();
                };
                report.timings_us.daemon_auth_us = daemon_auth_us;
                report.timings_us.daemon_job_execute_us = job_started.elapsed().as_micros() as u64;
                self.active_outputs.remove(&output_path);
                report.pipeline.daemon_used = true;
                self.jobs_completed += 1;
                if self.jobs_completed.is_multiple_of(64) {
                    let _ = self.engine.cache.gc(false);
                }
                match request.response_mode {
                    PackResponseMode::Full => Ok(DaemonResponse::PackComplete(Box::new(report))),
                    PackResponseMode::Summary => Ok(DaemonResponse::PackSummary(Box::new(
                        PackSummary::from(&report),
                    ))),
                }
            }
        }
    }

    fn execute_task(
        &mut self,
        request: TaskRequest,
        control: &crate::OperationControl,
    ) -> TaskResult {
        if control.is_cancelled() {
            return TaskResult::Cancelled;
        }
        match request {
            TaskRequest::Pack(mut request) => {
                request.response_mode = PackResponseMode::Full;
                match self.handle(DaemonRequest::Pack(request)) {
                    Ok(DaemonResponse::PackComplete(report)) => TaskResult::Pack { report },
                    Ok(DaemonResponse::PackSummary(summary)) => {
                        TaskResult::Failed(DaemonTaskError::new(
                            "unexpected_pack_summary",
                            format!(
                                "daemon returned summary for task: {} bytes",
                                summary.archive_bytes
                            ),
                            true,
                        ))
                    }
                    Ok(DaemonResponse::Error { code, message }) => {
                        TaskResult::Failed(DaemonTaskError::new(format!("{code:?}"), message, true))
                    }
                    Ok(_) => TaskResult::Failed(DaemonTaskError::new(
                        "unexpected_response",
                        "daemon returned an unexpected pack task response",
                        true,
                    )),
                    Err(error) => TaskResult::Failed(DaemonTaskError::new(
                        "pack_failed",
                        error.to_string(),
                        true,
                    )),
                }
            }
            TaskRequest::Unpack(request) => {
                let output_dir = request.options.output_dir.clone();
                match crate::unpack_with_control(
                    request.options.into_unpack(request.password),
                    control,
                ) {
                    Ok(()) => TaskResult::Unpack { output_dir },
                    Err(error) => TaskResult::Failed(DaemonTaskError::new(
                        "unpack_failed",
                        error.to_string(),
                        true,
                    )),
                }
            }
            TaskRequest::ProjectRebuild { project_id } => {
                let result = self
                    .projects
                    .get_mut(&project_id)
                    .ok_or_else(|| anyhow::anyhow!("project is not registered with this daemon"))
                    .and_then(|project| {
                        project.rebuild(&mut self.engine.cache, PipelineOptions::default())?;
                        Ok(project.status())
                    });
                match result {
                    Ok(status) => TaskResult::ProjectRebuild(status),
                    Err(error) => TaskResult::Failed(DaemonTaskError::new(
                        "project_rebuild_failed",
                        error.to_string(),
                        true,
                    )),
                }
            }
            TaskRequest::CacheGc { dry_run } => match self.engine.cache.gc(dry_run) {
                Ok(report) => TaskResult::CacheMaintenance(report),
                Err(error) => TaskResult::Failed(DaemonTaskError::new(
                    "cache_gc_failed",
                    error.to_string(),
                    true,
                )),
            },
            TaskRequest::CacheCompact { dry_run } => {
                match self.engine.cache.compact_sealed(dry_run) {
                    Ok(report) => TaskResult::CacheMaintenance(report),
                    Err(error) => TaskResult::Failed(DaemonTaskError::new(
                        "cache_compact_failed",
                        error.to_string(),
                        true,
                    )),
                }
            }
        }
    }
}

fn handle_shared_request(
    runtime: Arc<Mutex<DaemonRuntime>>,
    request: DaemonRequest,
) -> anyhow::Result<DaemonResponse> {
    let tasks = runtime
        .lock()
        .expect("daemon runtime mutex poisoned")
        .tasks
        .clone();
    match request {
        DaemonRequest::SubmitTask(submission) => {
            let task_kind = submission.request.kind();
            let (task_id, cancellation) = tasks.submit(&submission.request)?;
            let accepted = tasks.status(task_id)?;
            let runtime_for_task = runtime.clone();
            let tasks_for_worker = tasks.clone();
            std::thread::Builder::new()
                .name(format!("hig-task-{}", hex::encode(task_id)))
                .spawn(move || {
                    tasks_for_worker.mark_running(task_id);
                    let progress_tasks = tasks_for_worker.clone();
                    let control = crate::OperationControl::new(
                        task_id,
                        task_kind,
                        cancellation,
                        Arc::new(move |progress| {
                            progress_tasks.update_progress(progress);
                        }),
                    );
                    if control.is_cancelled() {
                        tasks_for_worker.complete(task_id, TaskResult::Cancelled);
                        return;
                    }
                    let result = runtime_for_task
                        .lock()
                        .map(|mut runtime| runtime.execute_task(submission.request, &control))
                        .unwrap_or_else(|_| {
                            TaskResult::Failed(DaemonTaskError::new(
                                "daemon_runtime_poisoned",
                                "daemon runtime lock was poisoned",
                                false,
                            ))
                        });
                    tasks_for_worker.complete(task_id, result);
                })?;
            Ok(DaemonResponse::TaskAccepted(accepted))
        }
        DaemonRequest::TaskStatus { task_id } => {
            Ok(DaemonResponse::TaskStatus(tasks.status(task_id)?))
        }
        DaemonRequest::TaskCancel { task_id } => {
            Ok(DaemonResponse::TaskStatus(tasks.cancel(task_id)?))
        }
        DaemonRequest::TaskResult { task_id } => {
            Ok(DaemonResponse::TaskResult(tasks.result(task_id)?))
        }
        DaemonRequest::TaskList { include_completed } => {
            Ok(DaemonResponse::TaskList(tasks.list(include_completed)))
        }
        other => runtime
            .lock()
            .expect("daemon runtime mutex poisoned")
            .handle(other),
    }
}

#[cfg(unix)]
pub fn run_daemon_server(cache_dir: &Path, ttl_secs: u64) -> anyhow::Result<()> {
    fs::create_dir_all(cache_dir)?;
    let cache_dir = cache_dir.canonicalize()?;
    let lock_path = cache_dir.join("daemon.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("a Hig daemon already owns this cache"))?;
    let socket = daemon_socket_path(&cache_dir);
    if socket.exists() {
        let _ = fs::remove_file(&socket);
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let runtime = Arc::new(Mutex::new(DaemonRuntime {
        engine: PackEngine::open(&cache_dir)?,
        cache_dir,
        session: None,
        started: Instant::now(),
        ttl_secs,
        jobs_completed: 0,
        active_outputs: BTreeSet::new(),
        projects: BTreeMap::new(),
        tasks: TaskManager::default(),
        stop: false,
    }));
    loop {
        {
            let mut runtime = runtime.lock().expect("daemon runtime mutex poisoned");
            if runtime.stop || runtime.started.elapsed().as_secs() > ttl_secs {
                break;
            }
            runtime.poll_projects();
            if runtime
                .session
                .as_ref()
                .is_some_and(|session| !session.active())
            {
                runtime.session = None;
            }
            runtime
                .tasks
                .prune_older_than(std::time::Duration::from_secs(600));
        }
        let Some(mut stream) = accept_with_timeout(&listener, 5)? else {
            continue;
        };
        let response = match verify_peer(&stream).and_then(|_| read_request(&mut stream)) {
            Ok(envelope) if envelope.protocol_version == PROTOCOL_VERSION => {
                let request_id = envelope.request_id;
                let mut payload = handle_shared_request(runtime.clone(), envelope.payload)
                    .unwrap_or_else(|error| DaemonResponse::Error {
                        code: DaemonErrorCode::Internal,
                        message: error.to_string(),
                    });
                stamp_response_metrics(&mut payload);
                ProtocolEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    request_id,
                    payload,
                }
            }
            Ok(envelope) => ProtocolEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: envelope.request_id,
                payload: DaemonResponse::Error {
                    code: DaemonErrorCode::ProtocolMismatch,
                    message: "daemon protocol version mismatch".to_string(),
                },
            },
            Err(error) => ProtocolEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: [0; 16],
                payload: DaemonResponse::Error {
                    code: DaemonErrorCode::Internal,
                    message: error.to_string(),
                },
            },
        };
        let _ = write_frame(&mut stream, &response);
    }
    runtime
        .lock()
        .expect("daemon runtime mutex poisoned")
        .session = None;
    let _ = fs::remove_file(socket);
    drop(lock);
    Ok(())
}

fn stamp_response_metrics(response: &mut DaemonResponse) {
    if !matches!(
        response,
        DaemonResponse::PackComplete(_) | DaemonResponse::PackSummary(_)
    ) {
        return;
    }
    let response_started = Instant::now();
    let response_bytes = bincode::serialize(response)
        .map(|bytes| bytes.len() as u64)
        .unwrap_or_default();
    let response_serialize_us = response_started.elapsed().as_micros() as u64;
    match response {
        DaemonResponse::PackComplete(report) => {
            report.timings_us.response_serialize_us = response_serialize_us;
            report.timings_us.daemon_response_bytes = response_bytes;
        }
        DaemonResponse::PackSummary(summary) => {
            summary.timings_us.response_serialize_us = response_serialize_us;
            summary.timings_us.daemon_response_bytes = response_bytes;
        }
        _ => {}
    }
}

#[cfg(unix)]
fn accept_with_timeout(
    listener: &UnixListener,
    timeout_ms: i32,
) -> anyhow::Result<Option<UnixStream>> {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor points to one valid pollfd for the duration of this call.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if result == 0 {
        return Ok(None);
    }
    anyhow::ensure!(result > 0, "daemon socket poll failed");
    let (stream, _) = listener.accept()?;
    Ok(Some(stream))
}

#[cfg(not(unix))]
pub fn run_daemon_server(_cache_dir: &Path, _ttl_secs: u64) -> anyhow::Result<()> {
    anyhow::bail!("daemon server is only supported on Unix platforms in v1.9.7")
}

pub fn daemon_socket_path(cache_dir: &Path) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"hig daemon socket v4");
    hasher.update(canonical_string(cache_dir).as_bytes());
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    hasher.update(user.as_bytes());
    std::env::temp_dir().join(format!(
        "hig-daemon-{}.sock",
        hex::encode(&hasher.finalize().as_bytes()[..12])
    ))
}

pub fn daemon_status(cache_dir: &Path) -> anyhow::Result<DaemonStatus> {
    match request_daemon(cache_dir, DaemonRequest::Status)? {
        Some(DaemonResponse::Status(status)) => Ok(status),
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => Ok(DaemonStatus::default()),
    }
}

pub fn stop_daemon(cache_dir: &Path) -> anyhow::Result<bool> {
    match request_daemon(cache_dir, DaemonRequest::Stop)? {
        Some(DaemonResponse::Stopped) => Ok(true),
        None => Ok(false),
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => Ok(false),
    }
}

pub fn cache_writer_available(cache_dir: &Path) -> anyhow::Result<bool> {
    fs::create_dir_all(cache_dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache_dir.join("daemon.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            fs2::FileExt::unlock(&lock)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn request_daemon(
    cache_dir: &Path,
    request: DaemonRequest,
) -> anyhow::Result<Option<DaemonResponse>> {
    #[cfg(unix)]
    {
        let socket = daemon_socket_path(cache_dir);
        let connect_started = Instant::now();
        let mut stream = match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let socket_connect_us = connect_started.elapsed().as_micros() as u64;
        let request_id = crate::crypto::random_bytes();
        write_frame(
            &mut stream,
            &ProtocolEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id,
                payload: request,
            },
        )?;
        let (mut response, client_decode_us, response_bytes) = read_response_frame(&mut stream)?;
        anyhow::ensure!(
            response.request_id == request_id,
            "daemon response id mismatch"
        );
        stamp_client_response_metrics(
            &mut response.payload,
            socket_connect_us,
            client_decode_us,
            response_bytes,
        );
        Ok(Some(response.payload))
    }
    #[cfg(not(unix))]
    {
        let _ = (cache_dir, request);
        Ok(None)
    }
}

#[cfg(unix)]
fn read_request(stream: &mut UnixStream) -> anyhow::Result<ProtocolEnvelope<DaemonRequest>> {
    read_frame(stream)
}

#[cfg(unix)]
fn read_response_frame(
    reader: &mut impl Read,
) -> anyhow::Result<(ProtocolEnvelope<DaemonResponse>, u64, u64)> {
    let mut len = [0_u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME_BYTES, "daemon frame exceeds 1MiB limit");
    let mut bytes = zeroize::Zeroizing::new(vec![0_u8; len]);
    reader.read_exact(&mut bytes)?;
    let decode_started = Instant::now();
    let response = bincode::deserialize(&bytes)?;
    Ok((
        response,
        decode_started.elapsed().as_micros() as u64,
        len as u64,
    ))
}

fn stamp_client_response_metrics(
    response: &mut DaemonResponse,
    socket_connect_us: u64,
    client_decode_us: u64,
    response_bytes: u64,
) {
    match response {
        DaemonResponse::PackComplete(report) => {
            report.timings_us.socket_connect_us = socket_connect_us;
            report.timings_us.client_decode_us = client_decode_us;
            report.timings_us.daemon_response_bytes = response_bytes;
        }
        DaemonResponse::PackSummary(summary) => {
            summary.timings_us.socket_connect_us = socket_connect_us;
            summary.timings_us.client_decode_us = client_decode_us;
            summary.timings_us.daemon_response_bytes = response_bytes;
        }
        _ => {}
    }
}

fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> anyhow::Result<T> {
    let mut len = [0_u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME_BYTES, "daemon frame exceeds 1MiB limit");
    let mut bytes = zeroize::Zeroizing::new(vec![0_u8; len]);
    reader.read_exact(&mut bytes)?;
    Ok(bincode::deserialize(&bytes)?)
}

fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> anyhow::Result<()> {
    let bytes = zeroize::Zeroizing::new(bincode::serialize(value)?);
    anyhow::ensure!(
        bytes.len() <= MAX_FRAME_BYTES,
        "daemon frame exceeds 1MiB limit"
    );
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

#[cfg(unix)]
fn verify_peer(stream: &UnixStream) -> anyhow::Result<()> {
    // SAFETY: geteuid has no arguments and no memory safety preconditions.
    let expected = unsafe { libc::geteuid() };
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
    {
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: uid/gid are valid writable values and the stream owns a valid descriptor.
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        anyhow::ensure!(result == 0, "failed to read daemon peer credentials");
        anyhow::ensure!(uid == expected, "daemon peer uid mismatch");
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: ucred is a plain C data structure and zero is a valid initialization.
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: all pointers reference valid writable storage for the declared lengths.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        anyhow::ensure!(result == 0, "failed to read daemon peer credentials");
        anyhow::ensure!(credentials.uid == expected, "daemon peer uid mismatch");
    }
    Ok(())
}

fn canonical_string(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_rejects_oversized_frame() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_le_bytes());
        assert!(read_frame::<ProtocolEnvelope<DaemonRequest>>(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn serializable_options_do_not_contain_password() {
        let options = SerializablePackOptions {
            input_dir: PathBuf::from("input"),
            output_file: PathBuf::from("out.hig"),
            encryption: EncryptionMode::Password,
            threads: None,
            compression: Compression::Zstd,
            level: None,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: ManifestFormat::Compact,
            use_session: false,
            session_required: false,
            solid: SolidMode::Auto,
            pipeline: PipelineOptions::default(),
        };
        let bytes = bincode::serialize(&options).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("password"));
    }

    #[test]
    fn zero_ttl_session_is_immediately_inactive() {
        let temp = tempfile::tempdir().unwrap();
        let kdf = KdfProfile::Secure.params();
        let session = SessionState {
            binding: crate::derive_session_binding(
                temp.path(),
                KdfProfile::Secure,
                &kdf,
                EncryptionMode::Password,
            ),
            key: [1; KEY_LEN],
            salt: [2; SALT_LEN],
            created: Instant::now(),
            ttl_secs: 0,
        };
        assert!(!session.active());
    }

    #[test]
    fn use_session_without_session_returns_structured_error() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = DaemonRuntime {
            cache_dir: temp.path().to_path_buf(),
            engine: PackEngine::open(temp.path()).unwrap(),
            session: None,
            started: Instant::now(),
            ttl_secs: 60,
            jobs_completed: 0,
            active_outputs: BTreeSet::new(),
            projects: BTreeMap::new(),
            tasks: TaskManager::default(),
            stop: false,
        };
        let response = runtime
            .handle(DaemonRequest::Pack(PackJobRequest {
                options: SerializablePackOptions {
                    input_dir: temp.path().join("missing"),
                    output_file: temp.path().join("out.hig"),
                    encryption: EncryptionMode::Password,
                    threads: None,
                    compression: Compression::Zstd,
                    level: None,
                    use_cache: true,
                    trust_metadata: false,
                    format: ArchiveFormat::HigV2,
                    batch: BatchOptions::default(),
                    chunk: ChunkOptions::default(),
                    speed: SpeedMode::Balanced,
                    kdf_profile: KdfProfile::Secure,
                    sealed_cache: false,
                    manifest_format: ManifestFormat::Compact,
                    use_session: true,
                    session_required: true,
                    solid: SolidMode::Auto,
                    pipeline: PipelineOptions::default(),
                },
                binding_fingerprint: Some([1; 32]),
                ephemeral_key: None,
                auth_mode: PackAuthMode::UseSession,
                response_mode: PackResponseMode::Summary,
            }))
            .unwrap();
        match response {
            DaemonResponse::Error { code, .. } => assert_eq!(code, DaemonErrorCode::NoSession),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn pack_response_mode_roundtrips() {
        let request = PackJobRequest {
            options: SerializablePackOptions {
                input_dir: PathBuf::from("input"),
                output_file: PathBuf::from("out.hig"),
                encryption: EncryptionMode::None,
                threads: None,
                compression: Compression::Zstd,
                level: None,
                use_cache: true,
                trust_metadata: false,
                format: ArchiveFormat::HigV2,
                batch: BatchOptions::default(),
                chunk: ChunkOptions::default(),
                speed: SpeedMode::Balanced,
                kdf_profile: KdfProfile::Secure,
                sealed_cache: false,
                manifest_format: ManifestFormat::Compact,
                use_session: false,
                session_required: false,
                solid: SolidMode::Auto,
                pipeline: PipelineOptions::default(),
            },
            binding_fingerprint: None,
            ephemeral_key: None,
            auth_mode: PackAuthMode::None,
            response_mode: PackResponseMode::Full,
        };
        let decoded: PackJobRequest =
            bincode::deserialize(&bincode::serialize(&request).unwrap()).unwrap();
        assert_eq!(decoded.response_mode, PackResponseMode::Full);
    }

    #[test]
    fn daemon_can_return_summary_or_full_pack_response() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("a.txt"), b"hello").unwrap();
        let mut runtime = DaemonRuntime {
            cache_dir: temp.path().join("cache"),
            engine: PackEngine::open(&temp.path().join("cache")).unwrap(),
            session: None,
            started: Instant::now(),
            ttl_secs: 60,
            jobs_completed: 0,
            active_outputs: BTreeSet::new(),
            projects: BTreeMap::new(),
            tasks: TaskManager::default(),
            stop: false,
        };
        let options = SerializablePackOptions {
            input_dir: input,
            output_file: temp.path().join("summary.hig"),
            encryption: EncryptionMode::None,
            threads: None,
            compression: Compression::Zstd,
            level: None,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: ManifestFormat::Compact,
            use_session: false,
            session_required: false,
            solid: SolidMode::Auto,
            pipeline: PipelineOptions::default(),
        };
        let summary = runtime
            .handle(DaemonRequest::Pack(PackJobRequest {
                options: options.clone(),
                binding_fingerprint: None,
                ephemeral_key: None,
                auth_mode: PackAuthMode::None,
                response_mode: PackResponseMode::Summary,
            }))
            .unwrap();
        assert!(matches!(summary, DaemonResponse::PackSummary(_)));
        let summary_bytes = bincode::serialize(&summary).unwrap().len();

        let mut full_options = options;
        full_options.output_file = temp.path().join("full.hig");
        let full = runtime
            .handle(DaemonRequest::Pack(PackJobRequest {
                options: full_options,
                binding_fingerprint: None,
                ephemeral_key: None,
                auth_mode: PackAuthMode::None,
                response_mode: PackResponseMode::Full,
            }))
            .unwrap();
        assert!(matches!(full, DaemonResponse::PackComplete(_)));
        let full_bytes = bincode::serialize(&full).unwrap().len();
        assert!(
            summary_bytes < full_bytes,
            "summary response should be smaller: summary={summary_bytes}, full={full_bytes}"
        );
    }
}
