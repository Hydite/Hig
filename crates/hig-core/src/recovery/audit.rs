use super::{
    RECOVERY_REPORT_SCHEMA, checked_json_bytes, enforce_private_file, load_vault_config,
    now_unix_ns, read_checked_json, resolve_vault_root, secure_create_dir, sync_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const AUDIT_SCHEMA: u16 = 1;
const MAX_AUDIT_TEXT_BYTES: usize = 4 * 1024;
const MAX_AUDIT_EVENTS: usize = 1_000_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAuditOperation {
    VaultInitialize,
    RetentionUpdate,
    RepositoryRegister,
    Capture,
    Restore,
    PinUpdate,
    TombstoneRecord,
    GarbageCollection,
    MirrorSynchronize,
    Repair,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAuditOutcome {
    Prepared,
    Committed,
    Failed,
}

impl RecoveryAuditOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committed => "committed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuditActor {
    pub process_id: u32,
    pub executable: String,
    pub principal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuditEvent {
    pub schema: u16,
    pub operation_id: String,
    pub operation: RecoveryAuditOperation,
    pub outcome: RecoveryAuditOutcome,
    pub occurred_unix_ns: i128,
    pub started_unix_ns: i128,
    pub actor: RecoveryAuditActor,
    pub catalog_generation_before: Option<u64>,
    pub catalog_generation_after: Option<u64>,
    pub repository_id: Option<[u8; 16]>,
    pub recovery_point_id: Option<String>,
    pub details: BTreeMap<String, String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuditReport {
    pub schema: u16,
    pub vault_root: String,
    pub events: Vec<RecoveryAuditEvent>,
    pub incomplete_operation_ids: Vec<String>,
}

pub fn recovery_audit_log(requested_root: Option<&Path>) -> anyhow::Result<RecoveryAuditReport> {
    let root = resolve_vault_root(requested_root)?;
    load_vault_config(&root)?;
    let events_root = root.join("events");
    secure_create_dir(&events_root)?;
    let mut events = Vec::new();
    for entry in fs::read_dir(&events_root)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("recovery audit filename is not UTF-8"))?
            .to_string();
        if name.starts_with('.') && name.contains(".tmp.") {
            continue;
        }
        anyhow::ensure!(
            entry.file_type()?.is_file() && !entry.file_type()?.is_symlink(),
            "recovery audit entry is not a physical file: {name}"
        );
        anyhow::ensure!(
            events.len() < MAX_AUDIT_EVENTS,
            "recovery audit event limit exceeded"
        );
        enforce_private_file(&entry.path())?;
        let event: RecoveryAuditEvent = read_checked_json(&entry.path())?;
        validate_audit_event(&event)?;
        anyhow::ensure!(
            name == audit_event_filename(&event),
            "recovery audit filename does not match its event"
        );
        events.push(event);
    }
    events.sort_by(|left, right| {
        left.occurred_unix_ns
            .cmp(&right.occurred_unix_ns)
            .then_with(|| left.operation_id.cmp(&right.operation_id))
            .then_with(|| {
                audit_outcome_order(left.outcome).cmp(&audit_outcome_order(right.outcome))
            })
    });

    let mut operations = BTreeMap::<String, Vec<&RecoveryAuditEvent>>::new();
    for event in &events {
        operations
            .entry(event.operation_id.clone())
            .or_default()
            .push(event);
    }
    let mut incomplete_operation_ids = Vec::new();
    for (operation_id, operation_events) in operations {
        let prepared = operation_events
            .iter()
            .filter(|event| event.outcome == RecoveryAuditOutcome::Prepared)
            .copied()
            .collect::<Vec<_>>();
        let terminal = operation_events
            .iter()
            .filter(|event| event.outcome != RecoveryAuditOutcome::Prepared)
            .copied()
            .collect::<Vec<_>>();
        anyhow::ensure!(
            prepared.len() == 1,
            "recovery audit operation must contain exactly one prepared event: {operation_id}"
        );
        anyhow::ensure!(
            terminal.len() <= 1,
            "recovery audit operation contains multiple terminal events: {operation_id}"
        );
        if let Some(terminal) = terminal.first() {
            validate_terminal_audit_event(prepared[0], terminal)?;
        } else {
            incomplete_operation_ids.push(operation_id);
        }
    }

    Ok(RecoveryAuditReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        events,
        incomplete_operation_ids,
    })
}

pub(super) struct RecoveryAuditTransaction<'a> {
    root: &'a Path,
    pub(super) prepared: RecoveryAuditEvent,
}

impl RecoveryAuditTransaction<'_> {
    fn finish(
        &self,
        outcome: RecoveryAuditOutcome,
        catalog_generation_after: Option<u64>,
        error: Option<String>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            outcome != RecoveryAuditOutcome::Prepared,
            "audit transaction terminal outcome cannot be prepared"
        );
        let mut terminal = self.prepared.clone();
        terminal.outcome = outcome;
        terminal.occurred_unix_ns = now_unix_ns();
        terminal.catalog_generation_after = catalog_generation_after;
        terminal.error = error;
        validate_audit_event(&terminal)?;
        write_checked_json_new(
            &self
                .root
                .join("events")
                .join(audit_event_filename(&terminal)),
            &terminal,
        )
    }
}

pub(super) fn run_audited<T>(
    root: &Path,
    operation: RecoveryAuditOperation,
    catalog_generation_before: Option<u64>,
    repository_id: Option<[u8; 16]>,
    recovery_point_id: Option<String>,
    details: BTreeMap<String, String>,
    action: impl FnOnce() -> anyhow::Result<(T, Option<u64>)>,
) -> anyhow::Result<T> {
    let audit = begin_audit(
        root,
        operation,
        catalog_generation_before,
        repository_id,
        recovery_point_id,
        details,
    )?;
    match action() {
        Ok((value, generation_after)) => {
            audit.finish(RecoveryAuditOutcome::Committed, generation_after, None)?;
            Ok(value)
        }
        Err(error) => {
            let error_text = bounded_audit_text(&error.to_string());
            if let Err(audit_error) =
                audit.finish(RecoveryAuditOutcome::Failed, None, Some(error_text))
            {
                return Err(error.context(format!(
                    "recovery audit failed to record the operation failure: {audit_error}"
                )));
            }
            Err(error)
        }
    }
}

pub(super) fn begin_audit(
    root: &Path,
    operation: RecoveryAuditOperation,
    catalog_generation_before: Option<u64>,
    repository_id: Option<[u8; 16]>,
    recovery_point_id: Option<String>,
    details: BTreeMap<String, String>,
) -> anyhow::Result<RecoveryAuditTransaction<'_>> {
    secure_create_dir(&root.join("events"))?;
    let event_count = fs::read_dir(root.join("events"))?
        .take(MAX_AUDIT_EVENTS + 1)
        .count();
    anyhow::ensure!(
        event_count < MAX_AUDIT_EVENTS,
        "recovery audit event capacity is exhausted"
    );
    let started_unix_ns = now_unix_ns();
    let prepared = RecoveryAuditEvent {
        schema: AUDIT_SCHEMA,
        operation_id: hex::encode(crate::random_bytes::<16>()),
        operation,
        outcome: RecoveryAuditOutcome::Prepared,
        occurred_unix_ns: started_unix_ns,
        started_unix_ns,
        actor: current_audit_actor()?,
        catalog_generation_before,
        catalog_generation_after: None,
        repository_id,
        recovery_point_id,
        details,
        error: None,
    };
    validate_audit_event(&prepared)?;
    write_checked_json_new(
        &root.join("events").join(audit_event_filename(&prepared)),
        &prepared,
    )?;
    Ok(RecoveryAuditTransaction { root, prepared })
}

fn current_audit_actor() -> anyhow::Result<RecoveryAuditActor> {
    let executable = std::env::current_exe()?.display().to_string();
    validate_audit_text("audit executable", &executable)?;
    Ok(RecoveryAuditActor {
        process_id: std::process::id(),
        executable,
        principal: audit_principal(),
    })
}

#[cfg(unix)]
fn audit_principal() -> Option<String> {
    Some(format!("unix-euid:{}", unsafe { libc::geteuid() }))
}

#[cfg(windows)]
fn audit_principal() -> Option<String> {
    None
}

#[cfg(not(any(unix, windows)))]
fn audit_principal() -> Option<String> {
    None
}

fn audit_event_filename(event: &RecoveryAuditEvent) -> String {
    format!("{}.{}.json", event.operation_id, event.outcome.as_str())
}

fn audit_outcome_order(outcome: RecoveryAuditOutcome) -> u8 {
    match outcome {
        RecoveryAuditOutcome::Prepared => 0,
        RecoveryAuditOutcome::Committed => 1,
        RecoveryAuditOutcome::Failed => 2,
    }
}

fn validate_audit_event(event: &RecoveryAuditEvent) -> anyhow::Result<()> {
    anyhow::ensure!(
        event.schema == AUDIT_SCHEMA,
        "unsupported recovery audit schema"
    );
    let operation_id = hex::decode(&event.operation_id)?;
    anyhow::ensure!(
        operation_id.len() == 16 && hex::encode(&operation_id) == event.operation_id,
        "recovery audit operation ID is not canonical"
    );
    anyhow::ensure!(
        event.occurred_unix_ns >= event.started_unix_ns,
        "recovery audit terminal time precedes its start"
    );
    validate_audit_text("audit executable", &event.actor.executable)?;
    if let Some(principal) = event.actor.principal.as_deref() {
        validate_audit_text("audit principal", principal)?;
    }
    if let Some(point_id) = event.recovery_point_id.as_deref() {
        validate_audit_text("audit recovery point", point_id)?;
    }
    for (key, value) in &event.details {
        anyhow::ensure!(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit()),
            "recovery audit detail key is not canonical"
        );
        validate_audit_text("audit detail key", key)?;
        validate_audit_text("audit detail value", value)?;
    }
    if let Some(error) = event.error.as_deref() {
        validate_audit_text("audit error", error)?;
    }
    match event.outcome {
        RecoveryAuditOutcome::Prepared => {
            anyhow::ensure!(
                event.catalog_generation_after.is_none() && event.error.is_none(),
                "prepared recovery audit event contains terminal state"
            );
        }
        RecoveryAuditOutcome::Committed => {
            anyhow::ensure!(
                event.error.is_none(),
                "committed audit event contains an error"
            );
            if let (Some(before), Some(after)) = (
                event.catalog_generation_before,
                event.catalog_generation_after,
            ) {
                anyhow::ensure!(
                    after >= before,
                    "committed audit event regresses the catalog generation"
                );
            }
        }
        RecoveryAuditOutcome::Failed => {
            anyhow::ensure!(
                event.error.as_ref().is_some_and(|error| !error.is_empty()),
                "failed audit event must contain an error"
            );
            anyhow::ensure!(
                event.catalog_generation_after.is_none(),
                "failed audit event claims a committed catalog generation"
            );
        }
    }
    Ok(())
}

fn validate_terminal_audit_event(
    prepared: &RecoveryAuditEvent,
    terminal: &RecoveryAuditEvent,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        terminal.outcome != RecoveryAuditOutcome::Prepared
            && terminal.operation == prepared.operation
            && terminal.started_unix_ns == prepared.started_unix_ns
            && terminal.actor == prepared.actor
            && terminal.catalog_generation_before == prepared.catalog_generation_before
            && terminal.repository_id == prepared.repository_id
            && terminal.recovery_point_id == prepared.recovery_point_id
            && terminal.details == prepared.details,
        "recovery audit terminal event does not match its prepared event: {}",
        prepared.operation_id
    );
    Ok(())
}

fn validate_audit_text(label: &str, value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.len() <= MAX_AUDIT_TEXT_BYTES,
        "{label} exceeds the recovery audit limit"
    );
    anyhow::ensure!(!value.contains('\0'), "{label} contains a null character");
    Ok(())
}

fn bounded_audit_text(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| if character == '\0' { ' ' } else { character })
        .collect::<String>();
    if sanitized.len() <= MAX_AUDIT_TEXT_BYTES {
        return sanitized;
    }
    let mut end = MAX_AUDIT_TEXT_BYTES;
    while !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    sanitized[..end].to_string()
}

pub(super) fn atomic_write_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(!path.exists(), "recovery immutable document already exists");
    if let Some(parent) = path.parent() {
        secure_create_dir(parent)?;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("recovery");
    let temp = path.with_file_name(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        hex::encode(crate::random_bytes::<8>())
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_new(&temp, path)?;
        enforce_private_file(path)?;
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

fn write_checked_json_new<T: Serialize>(path: &Path, payload: &T) -> anyhow::Result<()> {
    let bytes = checked_json_bytes(payload)?;
    atomic_write_new(path, &bytes)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_new(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn rename_new(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(windows)]
fn rename_new(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
fn rename_new(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)?;
    Ok(())
}
