use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    Pack,
    Unpack,
    Inspect,
    ProjectRebuild,
    CacheMaintenance,
    Benchmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationPhase {
    Queued,
    Scanning,
    VerifyingProject,
    Hashing,
    Planning,
    ReadingCache,
    Compressing,
    Encrypting,
    BuildingManifest,
    WritingArchive,
    VerifyingArchive,
    Extracting,
    CommittingCache,
    Benchmarking,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationProgress {
    pub task_id: [u8; 16],
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub files_done: u64,
    pub files_total: Option<u64>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub elapsed_us: u64,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct OperationControl {
    task_id: [u8; 16],
    kind: OperationKind,
    started: Instant,
    cancellation: Arc<AtomicBool>,
    progress: Arc<dyn Fn(OperationProgress) + Send + Sync>,
}

impl OperationControl {
    pub fn new(
        task_id: [u8; 16],
        kind: OperationKind,
        cancellation: Arc<AtomicBool>,
        progress: Arc<dyn Fn(OperationProgress) + Send + Sync>,
    ) -> Self {
        Self {
            task_id,
            kind,
            started: Instant::now(),
            cancellation,
            progress,
        }
    }

    pub fn detached(kind: OperationKind) -> Self {
        Self::new(
            crate::random_bytes(),
            kind,
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
        )
    }

    pub fn task_id(&self) -> [u8; 16] {
        self.task_id
    }

    pub fn cancel(&self) {
        self.cancellation.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    pub fn check_cancelled(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            anyhow::bail!("operation cancelled");
        }
        Ok(())
    }

    pub fn report(
        &self,
        phase: OperationPhase,
        files_done: u64,
        files_total: Option<u64>,
        bytes_done: u64,
        bytes_total: Option<u64>,
        message: Option<String>,
    ) {
        (self.progress)(OperationProgress {
            task_id: self.task_id,
            kind: self.kind,
            phase,
            files_done,
            files_total,
            bytes_done,
            bytes_total,
            elapsed_us: self.started.elapsed().as_micros() as u64,
            message,
        });
    }
}

impl std::fmt::Debug for OperationControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationControl")
            .field("task_id", &self.task_id)
            .field("kind", &self.kind)
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}
