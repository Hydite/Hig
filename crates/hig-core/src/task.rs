use crate::{
    CacheMaintenanceReport, OperationKind, OperationPhase, OperationProgress, PackJobRequest,
    PackReport, ProjectStatusReport, UnpackOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableUnpackOptions {
    pub archive_file: PathBuf,
    pub output_dir: PathBuf,
    pub overwrite: bool,
}

impl SerializableUnpackOptions {
    pub fn from_unpack(options: &UnpackOptions) -> Self {
        Self {
            archive_file: options.archive_file.clone(),
            output_dir: options.output_dir.clone(),
            overwrite: options.overwrite,
        }
    }

    pub fn into_unpack(self, password: Option<String>) -> UnpackOptions {
        UnpackOptions {
            archive_file: self.archive_file,
            output_dir: self.output_dir,
            password,
            overwrite: self.overwrite,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpackJobRequest {
    pub options: SerializableUnpackOptions,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskRequest {
    Pack(PackJobRequest),
    Unpack(UnpackJobRequest),
    ProjectRebuild { project_id: [u8; 16] },
    CacheGc { dry_run: bool },
    CacheCompact { dry_run: bool },
}

impl TaskRequest {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::Pack(_) => OperationKind::Pack,
            Self::Unpack(_) => OperationKind::Unpack,
            Self::ProjectRebuild { .. } => OperationKind::ProjectRebuild,
            Self::CacheGc { .. } | Self::CacheCompact { .. } => OperationKind::CacheMaintenance,
        }
    }

    pub fn output_path(&self) -> Option<PathBuf> {
        match self {
            Self::Pack(request) => Some(request.options.output_file.clone()),
            Self::Unpack(request) => Some(request.options.output_dir.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSubmitRequest {
    pub request: TaskRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonTaskError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl DaemonTaskError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, recoverable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            recoverable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusReport {
    pub task_id: [u8; 16],
    pub kind: OperationKind,
    pub state: TaskState,
    pub progress: OperationProgress,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub output_path: Option<PathBuf>,
    pub error: Option<DaemonTaskError>,
    pub cancellable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskResult {
    Pack { report: Box<PackReport> },
    Unpack { output_dir: PathBuf },
    ProjectRebuild(ProjectStatusReport),
    CacheMaintenance(CacheMaintenanceReport),
    Cancelled,
    Failed(DaemonTaskError),
}

struct TaskEntry {
    status: TaskStatusReport,
    result: Option<TaskResult>,
    cancellation: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub struct TaskManager {
    inner: Arc<Mutex<TaskManagerInner>>,
}

#[derive(Default)]
struct TaskManagerInner {
    tasks: BTreeMap<[u8; 16], TaskEntry>,
    completed_order: VecDeque<[u8; 16]>,
}

impl TaskManager {
    pub fn submit(&self, request: &TaskRequest) -> anyhow::Result<([u8; 16], Arc<AtomicBool>)> {
        let task_id = crate::random_bytes();
        let now = unix_ms();
        let output_path = request
            .output_path()
            .map(|path| path.canonicalize().unwrap_or(path));
        let mut inner = self.inner.lock().expect("task manager mutex poisoned");
        if let Some(output_path) = &output_path {
            let busy = inner.tasks.values().any(|entry| {
                entry.status.output_path.as_ref() == Some(output_path)
                    && matches!(entry.status.state, TaskState::Queued | TaskState::Running)
            });
            anyhow::ensure!(!busy, "output path already has an active daemon task");
        }
        let kind = request.kind();
        let progress = OperationProgress {
            task_id,
            kind,
            phase: OperationPhase::Queued,
            files_done: 0,
            files_total: None,
            bytes_done: 0,
            bytes_total: None,
            elapsed_us: 0,
            message: Some("Queued".to_string()),
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        inner.tasks.insert(
            task_id,
            TaskEntry {
                status: TaskStatusReport {
                    task_id,
                    kind,
                    state: TaskState::Queued,
                    progress,
                    created_unix_ms: now,
                    updated_unix_ms: now,
                    output_path,
                    error: None,
                    cancellable: true,
                },
                result: None,
                cancellation: cancellation.clone(),
            },
        );
        Ok((task_id, cancellation))
    }

    pub fn mark_running(&self, task_id: [u8; 16]) {
        self.update(task_id, |entry| {
            entry.status.state = TaskState::Running;
            entry.status.progress.phase = match entry.status.kind {
                OperationKind::Pack => OperationPhase::Scanning,
                OperationKind::Unpack => OperationPhase::Extracting,
                OperationKind::ProjectRebuild => OperationPhase::Scanning,
                OperationKind::CacheMaintenance => OperationPhase::CommittingCache,
                OperationKind::Inspect => OperationPhase::VerifyingArchive,
                OperationKind::Benchmark => OperationPhase::Benchmarking,
            };
            entry.status.progress.message = Some("Running".to_string());
        });
    }

    pub fn update_progress(&self, progress: OperationProgress) {
        self.update(progress.task_id, |entry| {
            entry.status.progress = progress;
        });
    }

    pub fn complete(&self, task_id: [u8; 16], result: TaskResult) {
        let state = match result {
            TaskResult::Cancelled => TaskState::Cancelled,
            TaskResult::Failed(_) => TaskState::Failed,
            _ => TaskState::Completed,
        };
        self.update(task_id, |entry| {
            entry.status.state = state;
            entry.status.cancellable = false;
            entry.status.progress.phase = match state {
                TaskState::Completed => OperationPhase::Completed,
                TaskState::Cancelled => OperationPhase::Cancelled,
                TaskState::Failed => OperationPhase::Failed,
                TaskState::Queued | TaskState::Running => entry.status.progress.phase,
            };
            entry.status.error = match &result {
                TaskResult::Failed(error) => Some(error.clone()),
                _ => None,
            };
            entry.result = Some(result);
        });
        let mut inner = self.inner.lock().expect("task manager mutex poisoned");
        inner.completed_order.push_back(task_id);
        while inner.completed_order.len() > 128 {
            if let Some(oldest) = inner.completed_order.pop_front() {
                inner.tasks.remove(&oldest);
            }
        }
    }

    pub fn cancel(&self, task_id: [u8; 16]) -> anyhow::Result<TaskStatusReport> {
        let mut inner = self.inner.lock().expect("task manager mutex poisoned");
        let entry = inner
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;
        entry.cancellation.store(true, Ordering::Release);
        entry.status.updated_unix_ms = unix_ms();
        Ok(entry.status.clone())
    }

    pub fn status(&self, task_id: [u8; 16]) -> anyhow::Result<TaskStatusReport> {
        self.inner
            .lock()
            .expect("task manager mutex poisoned")
            .tasks
            .get(&task_id)
            .map(|entry| entry.status.clone())
            .ok_or_else(|| anyhow::anyhow!("task not found"))
    }

    pub fn result(&self, task_id: [u8; 16]) -> anyhow::Result<TaskResult> {
        self.inner
            .lock()
            .expect("task manager mutex poisoned")
            .tasks
            .get(&task_id)
            .and_then(|entry| entry.result.clone())
            .ok_or_else(|| anyhow::anyhow!("task result is not ready"))
    }

    pub fn list(&self, include_completed: bool) -> Vec<TaskStatusReport> {
        self.inner
            .lock()
            .expect("task manager mutex poisoned")
            .tasks
            .values()
            .filter(|entry| {
                include_completed
                    || matches!(entry.status.state, TaskState::Queued | TaskState::Running)
            })
            .map(|entry| entry.status.clone())
            .collect()
    }

    fn update(&self, task_id: [u8; 16], update: impl FnOnce(&mut TaskEntry)) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("task manager mutex poisoned")
            .tasks
            .get_mut(&task_id)
        {
            update(entry);
            entry.status.updated_unix_ms = unix_ms();
        }
    }

    pub fn prune_older_than(&self, retention: Duration) {
        let cutoff = unix_ms().saturating_sub(retention.as_millis() as u64);
        let mut inner = self.inner.lock().expect("task manager mutex poisoned");
        let expired: Vec<[u8; 16]> = inner
            .tasks
            .iter()
            .filter_map(|(id, entry)| {
                if matches!(
                    entry.status.state,
                    TaskState::Completed | TaskState::Cancelled | TaskState::Failed
                ) && entry.status.updated_unix_ms < cutoff
                {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        for id in expired {
            inner.tasks.remove(&id);
            inner.completed_order.retain(|value| *value != id);
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArchiveFormat, BatchOptions, ChunkOptions, Compression, EncryptionMode, KdfProfile,
        ManifestFormat, PackAuthMode, PackResponseMode, PipelineOptions, SerializablePackOptions,
        SolidMode, SpeedMode,
    };

    fn pack_request(output: &str) -> TaskRequest {
        TaskRequest::Pack(PackJobRequest {
            options: SerializablePackOptions {
                input_dir: PathBuf::from("input"),
                output_file: PathBuf::from(output),
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
        })
    }

    #[test]
    fn active_output_path_is_exclusive() {
        let manager = TaskManager::default();
        manager.submit(&pack_request("out.hig")).unwrap();
        assert!(manager.submit(&pack_request("out.hig")).is_err());
        assert!(manager.submit(&pack_request("other.hig")).is_ok());
    }

    #[test]
    fn cancellation_and_completion_update_status() {
        let manager = TaskManager::default();
        let (task_id, cancellation) = manager.submit(&pack_request("out.hig")).unwrap();
        manager.mark_running(task_id);
        let status = manager.cancel(task_id).unwrap();
        assert!(cancellation.load(Ordering::Acquire));
        assert_eq!(status.state, TaskState::Running);
        manager.complete(task_id, TaskResult::Cancelled);
        let status = manager.status(task_id).unwrap();
        assert_eq!(status.state, TaskState::Cancelled);
        assert_eq!(status.progress.phase, OperationPhase::Cancelled);
        assert!(matches!(manager.result(task_id), Ok(TaskResult::Cancelled)));
    }

    #[test]
    fn completed_tasks_are_hidden_from_active_list() {
        let manager = TaskManager::default();
        let (task_id, _) = manager.submit(&pack_request("out.hig")).unwrap();
        manager.complete(
            task_id,
            TaskResult::Failed(DaemonTaskError::new("test", "failed", true)),
        );
        assert!(manager.list(false).is_empty());
        assert_eq!(manager.list(true).len(), 1);
    }
}
