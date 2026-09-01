use hig_core::{
    ArchiveInspection, DaemonRequest, DaemonResponse, DesktopPackRequest, DesktopUnpackRequest,
    EncryptionMode, KdfProfile, OperationKind, OperationPhase, PackAuthMode, PackJobRequest,
    ProjectStatusReport, SerializableUnpackOptions, TaskRequest, TaskResult, TaskState,
    TaskStatusReport, TaskSubmitRequest, UnpackJobRequest, daemon_status, default_session_ttl,
    derive_key, derive_session_binding, discover_project, init_project,
    inspect_archive as inspect_hig_archive, request_daemon, resolve_project_cache_dir, stop_daemon,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppError {
    code: String,
    message: String,
    recoverable: bool,
}

impl AppError {
    fn from_error(code: &str, error: impl std::fmt::Display, recoverable: bool) -> Self {
        Self {
            code: code.to_string(),
            message: error.to_string(),
            recoverable,
        }
    }
}

type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    default_output_dir: Option<String>,
    default_speed: String,
    default_encryption: String,
    session_ttl_secs: u64,
    recent_projects: Vec<String>,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default)]
    known_cache_dirs: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_output_dir: None,
            default_speed: "balanced".to_string(),
            default_encryption: "password".to_string(),
            session_ttl_secs: 1_800,
            recent_projects: Vec::new(),
            language: default_language(),
            known_cache_dirs: Vec::new(),
        }
    }
}

fn default_language() -> String {
    "system".to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    version: &'static str,
    platform: &'static str,
    settings: AppSettings,
    daemon_active: bool,
    session_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskStatus {
    id: String,
    kind: OperationKind,
    phase: OperationPhase,
    files_done: u64,
    files_total: Option<u64>,
    bytes_done: u64,
    bytes_total: Option<u64>,
    elapsed_us: u64,
    message: Option<String>,
    output_path: Option<String>,
    archive_bytes: Option<u64>,
    input_bytes: Option<u64>,
    error: Option<AppError>,
    cancellable: bool,
    cache_dir: String,
    disconnected: bool,
    result_expired: bool,
}

struct TaskRecord {
    status: TaskStatus,
    cache_dir: PathBuf,
}

#[derive(Default)]
struct DesktopState {
    tasks: Mutex<BTreeMap<String, TaskRecord>>,
    benchmarks: Mutex<BTreeMap<String, BenchmarkRecord>>,
    hidden_tasks: Mutex<BTreeSet<String>>,
    settings: Mutex<AppSettings>,
}

struct BenchmarkRecord {
    status: TaskStatus,
    child: Option<Child>,
    result_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkRequest {
    input_dir: String,
    suite: String,
    cache_dir: Option<String>,
    bench_dir: Option<String>,
    workers: Option<usize>,
    compare: bool,
    password: String,
}

fn remember_cache_dir(
    app: &AppHandle,
    state: &State<'_, Arc<DesktopState>>,
    cache_dir: &Path,
) -> AppResult<()> {
    let value = cache_dir.display().to_string();
    let mut settings = state.settings.lock().expect("settings mutex poisoned");
    settings.known_cache_dirs.retain(|path| path != &value);
    settings.known_cache_dirs.insert(0, value);
    settings.known_cache_dirs.truncate(16);
    save_settings(app, &settings)
}

fn settings_path(app: &AppHandle) -> AppResult<PathBuf> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::from_error("settings_path", error, false))?;
    fs::create_dir_all(&root)
        .map_err(|error| AppError::from_error("settings_create", error, false))?;
    Ok(root.join("settings.json"))
}

fn save_settings(app: &AppHandle, settings: &AppSettings) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| AppError::from_error("settings_encode", error, false))?;
    fs::write(settings_path(app)?, bytes)
        .map_err(|error| AppError::from_error("settings_write", error, true))
}

fn load_settings(app: &AppHandle) -> AppSettings {
    settings_path(app)
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn default_cache_dir() -> PathBuf {
    std::env::temp_dir().join("hig-desktop-cache")
}

fn sidecar_path() -> AppResult<PathBuf> {
    let executable = std::env::current_exe()
        .map_err(|error| AppError::from_error("sidecar_path", error, false))?;
    let sibling = executable.with_file_name("hig");
    if sibling.is_file() {
        return Ok(sibling);
    }
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/hig");
    if development.is_file() {
        return Ok(development);
    }
    Err(AppError::from_error(
        "sidecar_missing",
        "The bundled Hig command-line engine is missing",
        false,
    ))
}

fn ensure_daemon(cache_dir: &Path, ttl_secs: u64) -> AppResult<()> {
    if daemon_status(cache_dir).is_ok_and(|status| status.active) {
        return Ok(());
    }
    fs::create_dir_all(cache_dir)
        .map_err(|error| AppError::from_error("cache_create", error, true))?;
    Command::new(sidecar_path()?)
        .args([
            "daemon",
            "serve",
            "--cache-dir",
            &cache_dir.to_string_lossy(),
            "--ttl-secs",
            &ttl_secs.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| AppError::from_error("daemon_start", error, true))?;
    for _ in 0..100 {
        if daemon_status(cache_dir).is_ok_and(|status| status.active) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(AppError::from_error(
        "daemon_unavailable",
        "The Hig daemon did not become ready",
        true,
    ))
}

fn task_id_hex(id: &[u8; 16]) -> String {
    hex::encode(id)
}

fn task_key(cache_dir: &Path, id: &[u8; 16]) -> String {
    format!("{}::{}", cache_dir.display(), task_id_hex(id))
}

fn decode_task_id(value: &str) -> AppResult<[u8; 16]> {
    let bytes =
        hex::decode(value).map_err(|error| AppError::from_error("task_id", error, false))?;
    bytes
        .try_into()
        .map_err(|_| AppError::from_error("task_id", "Task id must be 16 bytes", false))
}

#[cfg(test)]
fn temp_output_path(output: &Path, task_id: &str) -> PathBuf {
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("archive.hig");
    output.with_file_name(format!(".{name}.hig-ui-task-{task_id}"))
}

fn task_status_from_report(report: TaskStatusReport, cache_dir: &Path) -> TaskStatus {
    TaskStatus {
        id: task_id_hex(&report.task_id),
        kind: report.kind,
        phase: report.progress.phase,
        files_done: report.progress.files_done,
        files_total: report.progress.files_total,
        bytes_done: report.progress.bytes_done,
        bytes_total: report.progress.bytes_total,
        elapsed_us: report.progress.elapsed_us,
        message: report.progress.message,
        output_path: report
            .output_path
            .as_ref()
            .map(|path| path.display().to_string()),
        archive_bytes: None,
        input_bytes: None,
        error: report.error.map(|error| AppError {
            code: error.code,
            message: error.message,
            recoverable: error.recoverable,
        }),
        cancellable: report.cancellable
            && matches!(report.state, TaskState::Queued | TaskState::Running),
        cache_dir: cache_dir.display().to_string(),
        disconnected: false,
        result_expired: false,
    }
}

fn task_status_from_result(mut status: TaskStatus, result: Option<TaskResult>) -> TaskStatus {
    match result {
        Some(TaskResult::Pack { report }) => {
            status.phase = OperationPhase::Completed;
            status.archive_bytes = Some(report.archive_bytes);
            status.input_bytes = Some(report.input_bytes);
            status.message = Some("Archive created".to_string());
            status.cancellable = false;
        }
        Some(TaskResult::Unpack { .. }) => {
            status.phase = OperationPhase::Completed;
            status.message = Some("Archive extracted".to_string());
            status.cancellable = false;
        }
        Some(TaskResult::CacheMaintenance(_)) | Some(TaskResult::ProjectRebuild(_)) => {
            status.phase = OperationPhase::Completed;
            status.message = Some("Task completed".to_string());
            status.cancellable = false;
        }
        Some(TaskResult::Cancelled) => {
            status.phase = OperationPhase::Cancelled;
            status.message = Some("Cancelled safely".to_string());
            status.cancellable = false;
        }
        Some(TaskResult::Failed(error)) => {
            status.phase = OperationPhase::Failed;
            status.error = Some(AppError {
                code: error.code,
                message: error.message,
                recoverable: error.recoverable,
            });
            status.cancellable = false;
        }
        None => {}
    }
    status
}

fn refresh_tasks_for_cache(state: &Arc<DesktopState>, cache_dir: &Path) {
    let Ok(Some(DaemonResponse::TaskList(reports))) = request_daemon(
        cache_dir,
        DaemonRequest::TaskList {
            include_completed: true,
        },
    ) else {
        let mut tasks = state.tasks.lock().expect("task mutex poisoned");
        for task in tasks
            .values_mut()
            .filter(|task| task.cache_dir == cache_dir)
        {
            task.status.disconnected = true;
        }
        return;
    };
    let mut tasks = state.tasks.lock().expect("task mutex poisoned");
    let hidden = state
        .hidden_tasks
        .lock()
        .expect("hidden task mutex poisoned")
        .clone();
    for report in reports {
        let key = task_key(cache_dir, &report.task_id);
        if hidden.contains(&key) {
            continue;
        }
        let mut status = task_status_from_report(report.clone(), cache_dir);
        if matches!(
            report.state,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled
        ) {
            match request_daemon(
                cache_dir,
                DaemonRequest::TaskResult {
                    task_id: report.task_id,
                },
            ) {
                Ok(Some(DaemonResponse::TaskResult(result))) => {
                    status = task_status_from_result(status, Some(result));
                }
                _ => status.result_expired = true,
            }
        }
        tasks.insert(
            key,
            TaskRecord {
                status,
                cache_dir: cache_dir.to_path_buf(),
            },
        );
    }
}

#[tauri::command]
fn bootstrap_app(app: AppHandle, state: State<'_, Arc<DesktopState>>) -> AppResult<AppSnapshot> {
    let settings = load_settings(&app);
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    let mut caches = settings
        .known_cache_dirs
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    caches.push(default_cache_dir());
    caches.sort();
    caches.dedup();
    for cache in caches {
        refresh_tasks_for_cache(state.inner(), &cache);
    }
    let daemon = daemon_status(&default_cache_dir()).unwrap_or_default();
    Ok(AppSnapshot {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        settings,
        daemon_active: daemon.active,
        session_active: daemon.session_active,
    })
}

#[tauri::command]
fn get_settings(state: State<'_, Arc<DesktopState>>) -> AppSettings {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone()
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    settings: AppSettings,
) -> AppResult<AppSettings> {
    save_settings(&app, &settings)?;
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    Ok(settings)
}

#[tauri::command]
fn initialize_project(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    directory: String,
    cache_dir: Option<String>,
    excludes: Vec<String>,
) -> AppResult<ProjectStatusReport> {
    let root = PathBuf::from(&directory);
    let config = init_project(&root, cache_dir.map(PathBuf::from), excludes)
        .map_err(|error| AppError::from_error("project_init", error, true))?;
    let root = root
        .canonicalize()
        .map_err(|error| AppError::from_error("project_path", error, true))?;
    let cache = resolve_project_cache_dir(&root, &config);
    remember_cache_dir(&app, &state, &cache)?;
    let status = match request_daemon(
        &cache,
        DaemonRequest::ProjectRegister(hig_core::ProjectRegistration {
            root: root.clone(),
            config,
        }),
    ) {
        Ok(Some(DaemonResponse::ProjectRegistered(status))) => status,
        _ => ProjectStatusReport {
            initialized: true,
            root: root.display().to_string(),
            cache_dir: cache.display().to_string(),
            ..ProjectStatusReport::default()
        },
    };
    if let Ok(mut settings) = state.settings.lock() {
        settings.recent_projects.retain(|path| path != &directory);
        settings.recent_projects.insert(0, directory);
        settings.recent_projects.truncate(12);
        save_settings(&app, &settings)?;
    }
    Ok(status)
}

#[tauri::command]
fn get_project_status(directory: String) -> AppResult<ProjectStatusReport> {
    let (root, config) = discover_project(Path::new(&directory))
        .map_err(|error| AppError::from_error("project_discovery", error, true))?
        .ok_or_else(|| {
            AppError::from_error(
                "project_not_initialized",
                "Project is not initialized",
                true,
            )
        })?;
    let cache = resolve_project_cache_dir(&root, &config);
    match request_daemon(
        &cache,
        DaemonRequest::ProjectStatus {
            project_id: config.project_id,
        },
    ) {
        Ok(Some(DaemonResponse::ProjectStatus(status))) => Ok(status),
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("project_status", message, true))
        }
        Ok(_) => Err(AppError::from_error(
            "daemon_unavailable",
            "Project watcher is offline",
            true,
        )),
        Err(error) => Err(AppError::from_error("daemon_unavailable", error, true)),
    }
}

#[tauri::command]
fn submit_project_rebuild(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    directory: String,
) -> AppResult<TaskStatus> {
    let (root, config) = discover_project(Path::new(&directory))
        .map_err(|error| AppError::from_error("project_discovery", error, true))?
        .ok_or_else(|| {
            AppError::from_error(
                "project_not_initialized",
                "Project is not initialized",
                true,
            )
        })?;
    let cache = resolve_project_cache_dir(&root, &config);
    remember_cache_dir(&app, &state, &cache)?;
    match request_daemon(
        &cache,
        DaemonRequest::SubmitTask(TaskSubmitRequest {
            request: TaskRequest::ProjectRebuild {
                project_id: config.project_id,
            },
        }),
    ) {
        Ok(Some(DaemonResponse::TaskAccepted(report))) => {
            let status = task_status_from_report(report.clone(), &cache);
            state.tasks.lock().expect("task mutex poisoned").insert(
                task_key(&cache, &report.task_id),
                TaskRecord {
                    status: status.clone(),
                    cache_dir: cache,
                },
            );
            Ok(status)
        }
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("project_rebuild", message, true))
        }
        Ok(_) => Err(AppError::from_error(
            "daemon_unavailable",
            "Project watcher is offline",
            true,
        )),
        Err(error) => Err(AppError::from_error("daemon_unavailable", error, true)),
    }
}

#[tauri::command]
fn start_pack(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    mut request: DesktopPackRequest,
) -> AppResult<TaskStatus> {
    request
        .validate()
        .map_err(|error| AppError::from_error("invalid_pack_option", error, true))?;
    let options = request
        .serializable_options()
        .map_err(|error| AppError::from_error("invalid_pack_option", error, true))?;
    let encryption = request.encryption;
    let cache_dir = request
        .cache_dir
        .clone()
        .unwrap_or_else(|| request.input_dir.join(".hig-cache"));
    ensure_daemon(&cache_dir, default_session_ttl(None))?;
    remember_cache_dir(&app, &state, &cache_dir)?;
    let mut password = request.password.take().map(Zeroizing::new);
    let kdf_profile = request.resolved_kdf_profile();
    let kdf = kdf_profile.params();
    let binding = derive_session_binding(&cache_dir, kdf_profile, &kdf, encryption);
    let mut ephemeral_key = None;
    let auth_mode = if encryption == EncryptionMode::None {
        PackAuthMode::None
    } else if request.use_session {
        PackAuthMode::UseSession
    } else if let Some(password) = password.as_ref() {
        let salt = hig_core::random_bytes::<16>();
        let key = derive_key(password, &salt, &kdf)
            .map_err(|error| AppError::from_error("pack_kdf", error, true))?;
        ephemeral_key = Some(hig_core::JobKeyMaterial { key, salt });
        PackAuthMode::PreferSessionOrJobKey
    } else {
        return Err(AppError::from_error(
            "session_required",
            "Unlock a secure session",
            true,
        ));
    };
    if let Some(password) = password.as_mut() {
        password.zeroize();
    }
    let response = request_daemon(
        &cache_dir,
        DaemonRequest::SubmitTask(TaskSubmitRequest {
            request: TaskRequest::Pack(PackJobRequest {
                options,
                binding_fingerprint: Some(binding.fingerprint),
                ephemeral_key,
                auth_mode,
                response_mode: hig_core::PackResponseMode::Full,
            }),
        }),
    );
    let report = match response {
        Ok(Some(DaemonResponse::TaskAccepted(report))) => report,
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            return Err(AppError::from_error("daemon_task", message, true));
        }
        Ok(_) => {
            return Err(AppError::from_error(
                "daemon_task",
                "Daemon returned an unexpected task response",
                true,
            ));
        }
        Err(error) => return Err(AppError::from_error("daemon_task", error, true)),
    };
    let mut status = task_status_from_report(report.clone(), &cache_dir);
    if let Ok(Some(DaemonResponse::TaskResult(result))) = request_daemon(
        &cache_dir,
        DaemonRequest::TaskResult {
            task_id: report.task_id,
        },
    ) {
        status = task_status_from_result(status, Some(result));
    }
    let id = task_key(&cache_dir, &report.task_id);
    state.tasks.lock().expect("task mutex poisoned").insert(
        id.clone(),
        TaskRecord {
            status: status.clone(),
            cache_dir,
        },
    );
    let _ = app.emit("hig://task-progress", status.clone());
    Ok(status)
}

#[tauri::command]
fn start_unpack(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    request: DesktopUnpackRequest,
) -> AppResult<TaskStatus> {
    let cache_dir = default_cache_dir();
    ensure_daemon(&cache_dir, default_session_ttl(None))?;
    let response = request_daemon(
        &cache_dir,
        DaemonRequest::SubmitTask(TaskSubmitRequest {
            request: TaskRequest::Unpack(UnpackJobRequest {
                options: SerializableUnpackOptions {
                    archive_file: request.archive_file,
                    output_dir: request.output_dir,
                    overwrite: request.overwrite,
                },
                password: request.password,
            }),
        }),
    );
    let report = match response {
        Ok(Some(DaemonResponse::TaskAccepted(report))) => report,
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            return Err(AppError::from_error("daemon_task", message, true));
        }
        Ok(_) => {
            return Err(AppError::from_error(
                "daemon_task",
                "Daemon returned an unexpected task response",
                true,
            ));
        }
        Err(error) => return Err(AppError::from_error("daemon_task", error, true)),
    };
    remember_cache_dir(&app, &state, &cache_dir)?;
    let mut status = task_status_from_report(report.clone(), &cache_dir);
    if let Ok(Some(DaemonResponse::TaskResult(result))) = request_daemon(
        &cache_dir,
        DaemonRequest::TaskResult {
            task_id: report.task_id,
        },
    ) {
        status = task_status_from_result(status, Some(result));
    }
    let id = task_key(&cache_dir, &report.task_id);
    state.tasks.lock().expect("task mutex poisoned").insert(
        id,
        TaskRecord {
            status: status.clone(),
            cache_dir,
        },
    );
    let _ = app.emit("hig://task-progress", status.clone());
    Ok(status)
}

#[tauri::command]
fn inspect_archive(path: String, password: Option<String>) -> AppResult<ArchiveInspection> {
    let password = password.map(Zeroizing::new);
    inspect_hig_archive(Path::new(&path), password.as_deref().map(String::as_str))
        .map_err(|error| AppError::from_error("inspect_failed", error, true))
}

fn find_task_key(
    tasks: &BTreeMap<String, TaskRecord>,
    task_id: &str,
    cache_dir: Option<&str>,
) -> Option<String> {
    tasks.iter().find_map(|(key, task)| {
        let cache_matches = cache_dir.is_none_or(|cache| task.cache_dir == Path::new(cache));
        (task.status.id == task_id && cache_matches).then(|| key.clone())
    })
}

#[tauri::command]
fn get_task_status(
    state: State<'_, Arc<DesktopState>>,
    task_id: String,
    cache_dir: Option<String>,
) -> AppResult<TaskStatus> {
    let id = decode_task_id(&task_id)?;
    let mut tasks = state.tasks.lock().expect("task mutex poisoned");
    let key = find_task_key(&tasks, &task_id, cache_dir.as_deref())
        .ok_or_else(|| AppError::from_error("task_not_found", "Task not found", false))?;
    let task = tasks
        .get_mut(&key)
        .ok_or_else(|| AppError::from_error("task_not_found", "Task not found", false))?;
    match request_daemon(&task.cache_dir, DaemonRequest::TaskStatus { task_id: id }) {
        Ok(Some(DaemonResponse::TaskStatus(report))) => {
            task.status = task_status_from_report(report, &task.cache_dir);
            if let Ok(Some(DaemonResponse::TaskResult(result))) =
                request_daemon(&task.cache_dir, DaemonRequest::TaskResult { task_id: id })
            {
                task.status = task_status_from_result(task.status.clone(), Some(result));
            }
            Ok(task.status.clone())
        }
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("task_status", message, true))
        }
        Ok(_) => Ok(task.status.clone()),
        Err(error) => Err(AppError::from_error("task_status", error, true)),
    }
}

#[tauri::command]
fn list_daemon_tasks(state: State<'_, Arc<DesktopState>>) -> Vec<TaskStatus> {
    let caches = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .known_cache_dirs
        .iter()
        .map(PathBuf::from)
        .chain(std::iter::once(default_cache_dir()))
        .collect::<Vec<_>>();
    for cache in caches {
        refresh_tasks_for_cache(state.inner(), &cache);
    }
    let mut statuses = state
        .tasks
        .lock()
        .expect("task mutex poisoned")
        .values()
        .map(|task| task.status.clone())
        .collect::<Vec<_>>();
    let mut benchmarks = state.benchmarks.lock().expect("benchmark mutex poisoned");
    for record in benchmarks.values_mut() {
        refresh_benchmark(record);
        statuses.push(record.status.clone());
    }
    statuses
}

#[tauri::command]
fn cancel_task(
    state: State<'_, Arc<DesktopState>>,
    task_id: String,
    cache_dir: Option<String>,
) -> AppResult<TaskStatus> {
    let mut tasks = state.tasks.lock().expect("task mutex poisoned");
    let key = find_task_key(&tasks, &task_id, cache_dir.as_deref())
        .ok_or_else(|| AppError::from_error("task_not_found", "Task not found", false))?;
    let task = tasks
        .get_mut(&key)
        .ok_or_else(|| AppError::from_error("task_not_found", "Task not found", false))?;
    let id = decode_task_id(&task_id)?;
    match request_daemon(&task.cache_dir, DaemonRequest::TaskCancel { task_id: id }) {
        Ok(Some(DaemonResponse::TaskStatus(report))) => {
            task.status = task_status_from_report(report, &task.cache_dir);
            task.status.message = Some("Cancellation requested".to_string());
            Ok(task.status.clone())
        }
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("task_cancel", message, true))
        }
        Ok(_) => Err(AppError::from_error(
            "task_cancel",
            "Daemon returned an unexpected cancel response",
            true,
        )),
        Err(error) => Err(AppError::from_error("task_cancel", error, true)),
    }
}

#[tauri::command]
fn get_task_result(task_id: String, cache_dir: String) -> AppResult<TaskResult> {
    let id = decode_task_id(&task_id)?;
    match request_daemon(
        Path::new(&cache_dir),
        DaemonRequest::TaskResult { task_id: id },
    ) {
        Ok(Some(DaemonResponse::TaskResult(result))) => Ok(result),
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("task_result_expired", message, false))
        }
        Ok(_) => Err(AppError::from_error(
            "task_result_expired",
            "Task result is unavailable",
            false,
        )),
        Err(error) => Err(AppError::from_error("daemon_unavailable", error, true)),
    }
}

#[tauri::command]
fn clear_local_task_history(state: State<'_, Arc<DesktopState>>) -> bool {
    let mut tasks = state.tasks.lock().expect("task mutex poisoned");
    let completed = tasks
        .iter()
        .filter(|(_, task)| !task.status.cancellable)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    state
        .hidden_tasks
        .lock()
        .expect("hidden task mutex poisoned")
        .extend(completed);
    tasks.retain(|_, task| task.status.cancellable);
    true
}

#[tauri::command]
fn get_cache_status(cache_dir: Option<String>) -> AppResult<hig_core::CacheMaintenanceReport> {
    cache_request(cache_dir, DaemonRequest::CacheStatus)
}

#[tauri::command]
fn preview_cache_gc(cache_dir: Option<String>) -> AppResult<hig_core::CacheMaintenanceReport> {
    cache_request(cache_dir, DaemonRequest::CacheGc { dry_run: true })
}

#[tauri::command]
fn preview_cache_compact(cache_dir: Option<String>) -> AppResult<hig_core::CacheMaintenanceReport> {
    cache_request(cache_dir, DaemonRequest::CacheCompact { dry_run: true })
}

fn submit_cache_task(
    app: &AppHandle,
    state: &State<'_, Arc<DesktopState>>,
    cache_dir: Option<String>,
    request: TaskRequest,
) -> AppResult<TaskStatus> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    ensure_daemon(&cache, default_session_ttl(None))?;
    remember_cache_dir(app, state, &cache)?;
    match request_daemon(
        &cache,
        DaemonRequest::SubmitTask(TaskSubmitRequest { request }),
    ) {
        Ok(Some(DaemonResponse::TaskAccepted(report))) => {
            let status = task_status_from_report(report.clone(), &cache);
            state.tasks.lock().expect("task mutex poisoned").insert(
                task_key(&cache, &report.task_id),
                TaskRecord {
                    status: status.clone(),
                    cache_dir: cache,
                },
            );
            Ok(status)
        }
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("cache_maintenance", message, true))
        }
        Ok(_) => Err(AppError::from_error(
            "daemon_unavailable",
            "Cache daemon is offline",
            true,
        )),
        Err(error) => Err(AppError::from_error("daemon_unavailable", error, true)),
    }
}

#[tauri::command]
fn submit_cache_gc(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    cache_dir: Option<String>,
) -> AppResult<TaskStatus> {
    submit_cache_task(
        &app,
        &state,
        cache_dir,
        TaskRequest::CacheGc { dry_run: false },
    )
}

#[tauri::command]
fn submit_cache_compact(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    cache_dir: Option<String>,
) -> AppResult<TaskStatus> {
    submit_cache_task(
        &app,
        &state,
        cache_dir,
        TaskRequest::CacheCompact { dry_run: false },
    )
}

fn cache_request(
    cache_dir: Option<String>,
    request: DaemonRequest,
) -> AppResult<hig_core::CacheMaintenanceReport> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    match request_daemon(&cache, request) {
        Ok(Some(DaemonResponse::CacheMaintenance(report))) => Ok(report),
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("cache_maintenance", message, true))
        }
        Ok(_) => Err(AppError::from_error(
            "daemon_unavailable",
            "Cache daemon is offline",
            true,
        )),
        Err(error) => Err(AppError::from_error("daemon_unavailable", error, true)),
    }
}

#[tauri::command]
fn get_session_status(cache_dir: Option<String>) -> AppResult<hig_core::DaemonStatus> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    daemon_status(&cache).map_err(|error| AppError::from_error("session_status", error, true))
}

#[tauri::command]
fn get_daemon_status(cache_dir: Option<String>) -> AppResult<hig_core::DaemonStatus> {
    get_session_status(cache_dir)
}

#[tauri::command]
fn start_daemon(
    cache_dir: Option<String>,
    ttl_secs: Option<u64>,
) -> AppResult<hig_core::DaemonStatus> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    ensure_daemon(&cache, default_session_ttl(ttl_secs))?;
    daemon_status(&cache).map_err(|error| AppError::from_error("daemon_unavailable", error, true))
}

#[tauri::command]
fn stop_desktop_daemon(cache_dir: Option<String>, force: bool) -> AppResult<bool> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    let status = daemon_status(&cache).unwrap_or_default();
    if status.active_jobs > 0 && !force {
        return Err(AppError::from_error(
            "daemon_busy",
            "The daemon has active tasks; confirm a forced stop",
            true,
        ));
    }
    stop_daemon(&cache).map_err(|error| AppError::from_error("daemon_stop", error, true))?;
    Ok(true)
}

#[tauri::command]
fn restart_daemon(
    cache_dir: Option<String>,
    ttl_secs: Option<u64>,
    force: bool,
) -> AppResult<hig_core::DaemonStatus> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    let status = daemon_status(&cache).unwrap_or_default();
    if status.active_jobs > 0 && !force {
        return Err(AppError::from_error(
            "daemon_busy",
            "The daemon has active tasks",
            true,
        ));
    }
    let _ = stop_daemon(&cache);
    ensure_daemon(&cache, default_session_ttl(ttl_secs))?;
    daemon_status(&cache).map_err(|error| AppError::from_error("daemon_unavailable", error, true))
}

#[tauri::command]
fn unlock_session(
    cache_dir: Option<String>,
    password: String,
    ttl_secs: Option<u64>,
) -> AppResult<hig_core::DaemonStatus> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    ensure_daemon(&cache, default_session_ttl(ttl_secs))?;
    let mut password = Zeroizing::new(password);
    let kdf_profile = KdfProfile::Secure;
    let kdf = kdf_profile.params();
    let binding = derive_session_binding(&cache, kdf_profile, &kdf, EncryptionMode::Password);
    let salt = match request_daemon(
        &cache,
        DaemonRequest::UnlockChallenge {
            binding: binding.clone(),
        },
    ) {
        Ok(Some(DaemonResponse::UnlockChallenge { salt })) => salt,
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            return Err(AppError::from_error("session_unlock", message, true));
        }
        Ok(_) => {
            return Err(AppError::from_error(
                "daemon_unavailable",
                "Daemon is offline",
                true,
            ));
        }
        Err(error) => return Err(AppError::from_error("daemon_unavailable", error, true)),
    };
    let mut key = derive_key(&password, &salt, &kdf)
        .map_err(|error| AppError::from_error("session_kdf", error, true))?;
    password.zeroize();
    let response = request_daemon(
        &cache,
        DaemonRequest::InstallSessionKey {
            binding,
            key,
            salt,
            ttl_secs: default_session_ttl(ttl_secs),
        },
    );
    key.zeroize();
    match response {
        Ok(Some(DaemonResponse::SessionInstalled)) => {
            get_session_status(Some(cache.display().to_string()))
        }
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("session_unlock", message, true))
        }
        Ok(_) => Err(AppError::from_error(
            "session_unlock",
            "Session was not installed",
            true,
        )),
        Err(error) => Err(AppError::from_error("session_unlock", error, true)),
    }
}

#[tauri::command]
fn clear_session(cache_dir: Option<String>) -> AppResult<bool> {
    let cache = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    match request_daemon(&cache, DaemonRequest::ClearSession) {
        Ok(Some(DaemonResponse::SessionCleared)) => Ok(true),
        Ok(None) => Ok(false),
        Ok(Some(DaemonResponse::Error { message, .. })) => {
            Err(AppError::from_error("session_clear", message, true))
        }
        Ok(_) => Ok(false),
        Err(error) => Err(AppError::from_error("session_clear", error, true)),
    }
}

fn refresh_benchmark(record: &mut BenchmarkRecord) {
    let Some(child) = record.child.as_mut() else {
        return;
    };
    match child.try_wait() {
        Ok(Some(exit)) => {
            record.status.cancellable = false;
            record.status.phase = if exit.success() {
                OperationPhase::Completed
            } else {
                OperationPhase::Failed
            };
            record.status.message = Some(if exit.success() {
                "Benchmark completed".to_string()
            } else {
                "Benchmark process failed".to_string()
            });
            if !exit.success() {
                record.status.error = Some(AppError::from_error(
                    "benchmark_failed",
                    format!("Benchmark exited with {exit}"),
                    true,
                ));
            }
            record.child = None;
        }
        Ok(None) => {}
        Err(error) => {
            record.status.phase = OperationPhase::Failed;
            record.status.cancellable = false;
            record.status.error = Some(AppError::from_error("benchmark_failed", error, true));
            record.child = None;
        }
    }
}

#[tauri::command]
fn start_benchmark(
    state: State<'_, Arc<DesktopState>>,
    request: BenchmarkRequest,
) -> AppResult<TaskStatus> {
    let password = Zeroizing::new(request.password);
    let allowed_suites = [
        "source",
        "lobehub",
        "lobehub-watch",
        "small500",
        "textmix",
        "repeat4m",
        "random8m",
        "binarymix",
        "all",
    ];
    if !allowed_suites.contains(&request.suite.as_str()) {
        return Err(AppError::from_error(
            "unsupported_option",
            "Unsupported benchmark suite",
            true,
        ));
    }
    if let Some(workers) = request.workers
        && !(1..=1024).contains(&workers)
    {
        return Err(AppError::from_error(
            "invalid_pack_option",
            "Workers must be between 1 and 1024",
            true,
        ));
    }
    let id: [u8; 16] = hig_core::random_bytes();
    let id_hex = task_id_hex(&id);
    let bench_dir = request
        .bench_dir
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&bench_dir)
        .map_err(|error| AppError::from_error("benchmark_dir", error, true))?;
    let result_file = bench_dir.join(format!("hig-desktop-{id_hex}.json"));
    let archive_file = bench_dir.join(format!("hig-desktop-{id_hex}.hig"));
    let stdout = fs::File::create(&result_file)
        .map_err(|error| AppError::from_error("benchmark_output", error, true))?;
    let mut command = Command::new(sidecar_path()?);
    command
        .arg("bench")
        .arg(&request.input_dir)
        .arg("--output")
        .arg(&archive_file)
        .arg("--bench-suite")
        .arg(&request.suite)
        .arg("--bench-dir")
        .arg(&bench_dir)
        .arg("--json")
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null());
    if request.compare {
        command.arg("--compare");
    }
    if let Some(cache_dir) = request.cache_dir.as_ref() {
        command.arg("--cache-dir").arg(cache_dir);
    }
    if let Some(workers) = request.workers {
        command.arg("--threads").arg(workers.to_string());
    }
    let mut child = command
        .spawn()
        .map_err(|error| AppError::from_error("benchmark_tool_missing", error, true))?;
    let mut child_stdin = child.stdin.take().ok_or_else(|| {
        AppError::from_error(
            "benchmark_password_pipe",
            "Benchmark password pipe is unavailable",
            true,
        )
    })?;
    if let Err(error) = child_stdin
        .write_all(password.as_bytes())
        .and_then(|_| child_stdin.write_all(b"\n"))
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AppError::from_error("benchmark_password_pipe", error, true));
    }
    drop(child_stdin);
    let status = TaskStatus {
        id: id_hex.clone(),
        kind: OperationKind::Benchmark,
        phase: OperationPhase::Benchmarking,
        files_done: 0,
        files_total: None,
        bytes_done: 0,
        bytes_total: None,
        elapsed_us: 0,
        message: Some("Benchmark running".to_string()),
        output_path: Some(result_file.display().to_string()),
        archive_bytes: None,
        input_bytes: None,
        error: None,
        cancellable: true,
        cache_dir: request.cache_dir.unwrap_or_default(),
        disconnected: false,
        result_expired: false,
    };
    state
        .benchmarks
        .lock()
        .expect("benchmark mutex poisoned")
        .insert(
            id_hex,
            BenchmarkRecord {
                status: status.clone(),
                child: Some(child),
                result_file,
            },
        );
    Ok(status)
}

#[tauri::command]
fn get_benchmark_status(
    state: State<'_, Arc<DesktopState>>,
    task_id: String,
) -> AppResult<TaskStatus> {
    let mut records = state.benchmarks.lock().expect("benchmark mutex poisoned");
    let record = records
        .get_mut(&task_id)
        .ok_or_else(|| AppError::from_error("task_not_found", "Benchmark task not found", false))?;
    refresh_benchmark(record);
    Ok(record.status.clone())
}

#[tauri::command]
fn cancel_benchmark(state: State<'_, Arc<DesktopState>>, task_id: String) -> AppResult<TaskStatus> {
    let mut records = state.benchmarks.lock().expect("benchmark mutex poisoned");
    let record = records
        .get_mut(&task_id)
        .ok_or_else(|| AppError::from_error("task_not_found", "Benchmark task not found", false))?;
    if let Some(child) = record.child.as_mut() {
        child
            .kill()
            .map_err(|error| AppError::from_error("benchmark_cancelled", error, true))?;
        let _ = child.wait();
    }
    record.child = None;
    record.status.phase = OperationPhase::Cancelled;
    record.status.cancellable = false;
    record.status.message = Some("Benchmark cancelled".to_string());
    let _ = fs::remove_file(&record.result_file);
    Ok(record.status.clone())
}

#[tauri::command]
fn current_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(DesktopState::default());
    tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            bootstrap_app,
            get_settings,
            update_settings,
            initialize_project,
            get_project_status,
            submit_project_rebuild,
            start_pack,
            start_unpack,
            inspect_archive,
            get_task_status,
            list_daemon_tasks,
            get_task_result,
            cancel_task,
            clear_local_task_history,
            get_cache_status,
            preview_cache_gc,
            submit_cache_gc,
            preview_cache_compact,
            submit_cache_compact,
            get_daemon_status,
            start_daemon,
            restart_daemon,
            stop_desktop_daemon,
            get_session_status,
            unlock_session,
            clear_session,
            start_benchmark,
            get_benchmark_status,
            cancel_benchmark,
            current_unix_ms,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Hig desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_temp_output_is_hidden_and_local_to_destination() {
        let output = Path::new("/tmp/releases/example.hig");
        let temporary = temp_output_path(output, "001122");
        assert_eq!(temporary.parent(), output.parent());
        assert_eq!(
            temporary.file_name().and_then(|name| name.to_str()),
            Some(".example.hig.hig-ui-task-001122")
        );
    }

    #[test]
    fn settings_serialization_contains_no_password_field() {
        let encoded = serde_json::to_string(&AppSettings::default()).unwrap();
        assert!(!encoded.to_ascii_lowercase().contains("benchmark-password"));
        assert!(!encoded.to_ascii_lowercase().contains("temporary-password"));
        assert!(!encoded.to_ascii_lowercase().contains("secret"));
    }

    #[test]
    fn legacy_settings_default_to_system_language() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "defaultOutputDir": null,
                "defaultSpeed": "balanced",
                "defaultEncryption": "password",
                "sessionTtlSecs": 1800,
                "recentProjects": []
            }"#,
        )
        .unwrap();
        assert_eq!(settings.language, "system");
        assert!(settings.known_cache_dirs.is_empty());
    }

    #[test]
    fn task_identity_includes_cache_binding() {
        let id = [7_u8; 16];
        assert_ne!(
            task_key(Path::new("/tmp/cache-a"), &id),
            task_key(Path::new("/tmp/cache-b"), &id)
        );
    }
}
