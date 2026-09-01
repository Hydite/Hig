use super::audit::atomic_write_new;
use super::auth;
use super::{MAX_RECOVERY_DOCUMENT_BYTES, atomic_write, enforce_private_file, secure_create_dir};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

const AUDIT_CHAIN_SCHEMA: u16 = 1;
const AUDIT_CHAIN_HASH_DOMAIN: &[u8] = b"hig-recovery-audit-chain-hash-v1\0";
const AUDIT_HEAD_MAC_DOMAIN: &[u8] = b"hig-recovery-audit-head-mac-v1\0";
const AUDIT_PENDING_MAC_DOMAIN: &[u8] = b"hig-recovery-audit-pending-mac-v1\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AuditChainEntry {
    schema: u16,
    vault_id: String,
    sequence: u64,
    event_filename: String,
    event_blake3: String,
    previous_chain_hash: Option<String>,
    chain_hash: String,
}

#[derive(Serialize)]
struct UnsignedAuditChainEntry<'a> {
    schema: u16,
    vault_id: &'a str,
    sequence: u64,
    event_filename: &'a str,
    event_blake3: &'a str,
    previous_chain_hash: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AuditChainHead {
    schema: u16,
    lineage_id: String,
    vault_id: String,
    key_id: String,
    sequence: u64,
    chain_hash: String,
    head_mac: String,
}

#[derive(Serialize)]
struct UnsignedAuditChainHead<'a> {
    schema: u16,
    lineage_id: &'a str,
    vault_id: &'a str,
    key_id: &'a str,
    sequence: u64,
    chain_hash: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AuditChainPending {
    schema: u16,
    lineage_id: String,
    vault_id: String,
    key_id: String,
    previous_sequence: u64,
    previous_chain_hash: Option<String>,
    entry: AuditChainEntry,
    target_head: AuditChainHead,
    pending_mac: String,
}

#[derive(Serialize)]
struct UnsignedAuditChainPending<'a> {
    schema: u16,
    lineage_id: &'a str,
    vault_id: &'a str,
    key_id: &'a str,
    previous_sequence: u64,
    previous_chain_hash: Option<&'a str>,
    entry: &'a AuditChainEntry,
    target_head: &'a AuditChainHead,
}

pub(super) fn append_event(
    root: &Path,
    event_filename: &str,
    event_bytes: &[u8],
) -> anyhow::Result<()> {
    append_event_internal(root, event_filename, event_bytes, false)
}

pub(super) fn recover(root: &Path) -> anyhow::Result<()> {
    recover_pending(root)
}

fn append_event_internal(
    root: &Path,
    event_filename: &str,
    event_bytes: &[u8],
    allow_existing_unbound_events: bool,
) -> anyhow::Result<()> {
    validate_event_filename(event_filename)?;
    recover_pending(root)?;
    let identity = auth::load_identity(root)?;
    let current = load_external_head(root, &identity.vault_id)?;
    if current.is_none() && !allow_existing_unbound_events {
        let existing_events = fs::read_dir(root.join("events"))?
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                !(name.starts_with('.') && name.contains(".tmp."))
            })
            .count();
        anyhow::ensure!(
            existing_events == 0,
            "Recovery audit chain is missing for existing events; run `hig recovery migrate-auth` before mutation"
        );
    }
    if let Some(current) = current.as_ref() {
        validate_head(root, current, &identity.lineage_id, &identity.vault_id)?;
    }
    let sequence = current.as_ref().map_or(Ok(1), |head| {
        head.sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("recovery audit sequence exhausted"))
    })?;
    let mut entry = AuditChainEntry {
        schema: AUDIT_CHAIN_SCHEMA,
        vault_id: identity.vault_id.clone(),
        sequence,
        event_filename: event_filename.to_string(),
        event_blake3: blake3::hash(event_bytes).to_hex().to_string(),
        previous_chain_hash: current.as_ref().map(|head| head.chain_hash.clone()),
        chain_hash: String::new(),
    };
    entry.chain_hash = entry_hash(&entry)?;
    let mut head = AuditChainHead {
        schema: AUDIT_CHAIN_SCHEMA,
        lineage_id: identity.lineage_id.clone(),
        vault_id: identity.vault_id.clone(),
        key_id: identity.key_id.clone(),
        sequence,
        chain_hash: entry.chain_hash.clone(),
        head_mac: String::new(),
    };
    head.head_mac = sign_head(root, &head)?;
    let mut pending = AuditChainPending {
        schema: AUDIT_CHAIN_SCHEMA,
        lineage_id: identity.lineage_id,
        vault_id: identity.vault_id,
        key_id: identity.key_id,
        previous_sequence: current.as_ref().map_or(0, |head| head.sequence),
        previous_chain_hash: current.as_ref().map(|head| head.chain_hash.clone()),
        entry,
        target_head: head,
        pending_mac: String::new(),
    };
    pending.pending_mac = sign_pending(root, &pending)?;
    prepare_event_staging(root, event_filename, event_bytes)?;
    atomic_write(
        &auth::external_audit_pending_path(root, &pending.vault_id)?,
        &serde_json::to_vec_pretty(&pending)?,
    )?;
    super::recovery_failpoint("audit_after_pending")?;
    finish_pending(root, &pending, current.as_ref())
}

pub(super) fn verify(root: &Path, event_files: &BTreeMap<String, Vec<u8>>) -> anyhow::Result<()> {
    recover_pending(root)?;
    let identity = auth::load_identity(root)?;
    let external = load_external_head(root, &identity.vault_id)?;
    if event_files.is_empty() {
        anyhow::ensure!(
            external.is_none(),
            "Recovery audit head exists without events"
        );
        return Ok(());
    }
    let external = external.ok_or_else(|| {
        anyhow::anyhow!(
            "Recovery audit chain is missing; run `hig recovery migrate-auth` before use"
        )
    })?;
    validate_head(root, &external, &identity.lineage_id, &identity.vault_id)?;
    let internal: AuditChainHead = read_json(&internal_head_path(root))?;
    anyhow::ensure!(
        internal == external,
        "Recovery audit internal head differs from the external authenticated head"
    );
    anyhow::ensure!(
        external.sequence == u64::try_from(event_files.len())?,
        "Recovery audit event count differs from the authenticated chain"
    );
    let mut previous_hash = None;
    let mut chained_events = BTreeMap::new();
    for sequence in 1..=external.sequence {
        let entry: AuditChainEntry = read_json(&entry_path(root, sequence))?;
        validate_entry(
            &entry,
            &identity.vault_id,
            sequence,
            previous_hash.as_deref(),
        )?;
        let event_bytes = event_files.get(&entry.event_filename).ok_or_else(|| {
            anyhow::anyhow!("Recovery authenticated audit event is missing from the journal")
        })?;
        anyhow::ensure!(
            blake3::hash(event_bytes).to_hex().as_str() == entry.event_blake3,
            "Recovery authenticated audit event hash mismatch"
        );
        anyhow::ensure!(
            chained_events
                .insert(entry.event_filename.clone(), ())
                .is_none(),
            "Recovery audit chain references an event more than once"
        );
        previous_hash = Some(entry.chain_hash);
    }
    anyhow::ensure!(
        previous_hash.as_deref() == Some(external.chain_hash.as_str())
            && chained_events.len() == event_files.len(),
        "Recovery audit chain head or event coverage mismatch"
    );
    Ok(())
}

pub(super) fn migrate_existing(
    root: &Path,
    ordered_events: &[(String, Vec<u8>)],
) -> anyhow::Result<bool> {
    recover_pending(root)?;
    let identity = auth::load_identity(root)?;
    if auth::external_audit_head_path(root, &identity.vault_id)?.exists() {
        let events = ordered_events.iter().cloned().collect::<BTreeMap<_, _>>();
        verify(root, &events)?;
        return Ok(false);
    }
    anyhow::ensure!(
        !internal_head_path(root).exists(),
        "Recovery audit migration found an untrusted internal head"
    );
    for (filename, bytes) in ordered_events {
        anyhow::ensure!(
            root.join("events").join(filename).exists(),
            "Recovery audit migration event is missing"
        );
        append_event_internal(root, filename, bytes, true)?;
    }
    Ok(!ordered_events.is_empty())
}

fn recover_pending(root: &Path) -> anyhow::Result<()> {
    if !auth::is_authenticated(root) {
        return Ok(());
    }
    let identity = auth::load_identity(root)?;
    let path = auth::external_audit_pending_path(root, &identity.vault_id)?;
    if !path.exists() {
        cleanup_staging(root)?;
        return Ok(());
    }
    let pending: AuditChainPending = read_json(&path)?;
    validate_pending(root, &pending, &identity.lineage_id, &identity.vault_id)?;
    let current = load_external_head(root, &identity.vault_id)?;
    if current.as_ref() == Some(&pending.target_head) {
        verify_published_pending(root, &pending)?;
        atomic_write(
            &internal_head_path(root),
            &serde_json::to_vec_pretty(&pending.target_head)?,
        )?;
        cleanup_pending(root, &path)?;
        return Ok(());
    }
    finish_pending(root, &pending, current.as_ref())
}

fn finish_pending(
    root: &Path,
    pending: &AuditChainPending,
    current: Option<&AuditChainHead>,
) -> anyhow::Result<()> {
    validate_pending(root, pending, &pending.lineage_id, &pending.vault_id)?;
    match current {
        Some(head) => {
            validate_head(root, head, &pending.lineage_id, &pending.vault_id)?;
            anyhow::ensure!(
                head.sequence == pending.previous_sequence
                    && pending.previous_chain_hash.as_deref() == Some(head.chain_hash.as_str())
                    && pending.entry.sequence == head.sequence.saturating_add(1),
                "Recovery audit pending event is not anchored to the external head"
            );
        }
        None => anyhow::ensure!(
            pending.previous_sequence == 0
                && pending.previous_chain_hash.is_none()
                && pending.entry.sequence == 1,
            "Recovery audit initial pending event is not canonical"
        ),
    }
    let staged = read_bounded(&staging_event_path(root, &pending.entry.event_filename))?;
    anyhow::ensure!(
        blake3::hash(&staged).to_hex().as_str() == pending.entry.event_blake3,
        "Recovery audit staged event hash mismatch"
    );
    let event_path = root.join("events").join(&pending.entry.event_filename);
    if event_path.exists() {
        anyhow::ensure!(
            read_bounded(&event_path)? == staged,
            "Recovery immutable audit event conflicts with pending publication"
        );
    } else {
        atomic_write_new(&event_path, &staged)?;
    }
    super::recovery_failpoint("audit_after_event_publication")?;
    let entry_path = entry_path(root, pending.entry.sequence);
    let entry_bytes = serde_json::to_vec_pretty(&pending.entry)?;
    if entry_path.exists() {
        anyhow::ensure!(
            read_bounded(&entry_path)? == entry_bytes,
            "Recovery audit chain entry conflicts at the same sequence"
        );
    } else {
        atomic_write_new(&entry_path, &entry_bytes)?;
    }
    super::recovery_failpoint("audit_after_chain_entry")?;
    let head_bytes = serde_json::to_vec_pretty(&pending.target_head)?;
    atomic_write(&internal_head_path(root), &head_bytes)?;
    super::recovery_failpoint("audit_after_internal_head")?;
    atomic_write(
        &auth::external_audit_head_path(root, &pending.vault_id)?,
        &head_bytes,
    )?;
    super::recovery_failpoint("audit_after_external_head")?;
    cleanup_pending(
        root,
        &auth::external_audit_pending_path(root, &pending.vault_id)?,
    )
}

fn verify_published_pending(root: &Path, pending: &AuditChainPending) -> anyhow::Result<()> {
    let event = read_bounded(&root.join("events").join(&pending.entry.event_filename))?;
    anyhow::ensure!(
        blake3::hash(&event).to_hex().as_str() == pending.entry.event_blake3,
        "Recovery published audit event hash mismatch"
    );
    let entry: AuditChainEntry = read_json(&entry_path(root, pending.entry.sequence))?;
    anyhow::ensure!(
        entry == pending.entry,
        "Recovery published audit chain entry mismatch"
    );
    Ok(())
}

fn validate_entry(
    entry: &AuditChainEntry,
    vault_id: &str,
    sequence: u64,
    previous_hash: Option<&str>,
) -> anyhow::Result<()> {
    validate_event_filename(&entry.event_filename)?;
    anyhow::ensure!(
        entry.schema == AUDIT_CHAIN_SCHEMA
            && entry.vault_id == vault_id
            && entry.sequence == sequence
            && entry.previous_chain_hash.as_deref() == previous_hash
            && entry.chain_hash == entry_hash(entry)?,
        "Recovery audit chain entry validation failed"
    );
    Ok(())
}

fn validate_head(
    root: &Path,
    head: &AuditChainHead,
    lineage_id: &str,
    vault_id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        head.schema == AUDIT_CHAIN_SCHEMA
            && head.lineage_id == lineage_id
            && head.vault_id == vault_id
            && head.sequence > 0,
        "Recovery audit head identity is invalid"
    );
    auth::verify_keyed_signature(
        root,
        &head.lineage_id,
        &head.key_id,
        AUDIT_HEAD_MAC_DOMAIN,
        &serde_json::to_vec(&UnsignedAuditChainHead {
            schema: head.schema,
            lineage_id: &head.lineage_id,
            vault_id: &head.vault_id,
            key_id: &head.key_id,
            sequence: head.sequence,
            chain_hash: &head.chain_hash,
        })?,
        &head.head_mac,
    )
}

fn validate_pending(
    root: &Path,
    pending: &AuditChainPending,
    lineage_id: &str,
    vault_id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        pending.schema == AUDIT_CHAIN_SCHEMA
            && pending.lineage_id == lineage_id
            && pending.vault_id == vault_id
            && pending.entry.vault_id == vault_id
            && pending.target_head.lineage_id == lineage_id
            && pending.target_head.vault_id == vault_id
            && pending.target_head.key_id == pending.key_id
            && pending.target_head.sequence == pending.entry.sequence
            && pending.target_head.chain_hash == pending.entry.chain_hash,
        "Recovery audit pending identity is invalid"
    );
    validate_head(root, &pending.target_head, lineage_id, vault_id)?;
    auth::verify_keyed_signature(
        root,
        &pending.lineage_id,
        &pending.key_id,
        AUDIT_PENDING_MAC_DOMAIN,
        &serde_json::to_vec(&UnsignedAuditChainPending {
            schema: pending.schema,
            lineage_id: &pending.lineage_id,
            vault_id: &pending.vault_id,
            key_id: &pending.key_id,
            previous_sequence: pending.previous_sequence,
            previous_chain_hash: pending.previous_chain_hash.as_deref(),
            entry: &pending.entry,
            target_head: &pending.target_head,
        })?,
        &pending.pending_mac,
    )
}

fn sign_head(root: &Path, head: &AuditChainHead) -> anyhow::Result<String> {
    let (identity, signature) = auth::sign_current(
        root,
        AUDIT_HEAD_MAC_DOMAIN,
        &serde_json::to_vec(&UnsignedAuditChainHead {
            schema: head.schema,
            lineage_id: &head.lineage_id,
            vault_id: &head.vault_id,
            key_id: &head.key_id,
            sequence: head.sequence,
            chain_hash: &head.chain_hash,
        })?,
    )?;
    anyhow::ensure!(
        identity.lineage_id == head.lineage_id
            && identity.vault_id == head.vault_id
            && identity.key_id == head.key_id,
        "Recovery audit signer identity changed"
    );
    Ok(signature)
}

fn sign_pending(root: &Path, pending: &AuditChainPending) -> anyhow::Result<String> {
    let (identity, signature) = auth::sign_current(
        root,
        AUDIT_PENDING_MAC_DOMAIN,
        &serde_json::to_vec(&UnsignedAuditChainPending {
            schema: pending.schema,
            lineage_id: &pending.lineage_id,
            vault_id: &pending.vault_id,
            key_id: &pending.key_id,
            previous_sequence: pending.previous_sequence,
            previous_chain_hash: pending.previous_chain_hash.as_deref(),
            entry: &pending.entry,
            target_head: &pending.target_head,
        })?,
    )?;
    anyhow::ensure!(
        identity.lineage_id == pending.lineage_id
            && identity.vault_id == pending.vault_id
            && identity.key_id == pending.key_id,
        "Recovery audit pending signer identity changed"
    );
    Ok(signature)
}

fn entry_hash(entry: &AuditChainEntry) -> anyhow::Result<String> {
    let mut bytes = AUDIT_CHAIN_HASH_DOMAIN.to_vec();
    bytes.extend(serde_json::to_vec(&UnsignedAuditChainEntry {
        schema: entry.schema,
        vault_id: &entry.vault_id,
        sequence: entry.sequence,
        event_filename: &entry.event_filename,
        event_blake3: &entry.event_blake3,
        previous_chain_hash: entry.previous_chain_hash.as_deref(),
    })?);
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn prepare_event_staging(root: &Path, filename: &str, bytes: &[u8]) -> anyhow::Result<()> {
    cleanup_staging(root)?;
    secure_create_dir(&staging_root(root))?;
    atomic_write(&staging_event_path(root, filename), bytes)
}

fn cleanup_pending(root: &Path, pending: &Path) -> anyhow::Result<()> {
    if pending.exists() {
        fs::remove_file(pending)?;
    }
    cleanup_staging(root)
}

fn cleanup_staging(root: &Path) -> anyhow::Result<()> {
    let staging = staging_root(root);
    if staging.exists() {
        let metadata = fs::symlink_metadata(&staging)?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Recovery audit transition staging is not a physical directory"
        );
        fs::remove_dir_all(staging)?;
    }
    Ok(())
}

fn load_external_head(root: &Path, vault_id: &str) -> anyhow::Result<Option<AuditChainHead>> {
    let path = auth::external_audit_head_path(root, vault_id)?;
    if path.exists() {
        Ok(Some(read_json(&path)?))
    } else {
        Ok(None)
    }
}

fn validate_event_filename(filename: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !filename.is_empty()
            && filename.len() <= 256
            && filename
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')),
        "Recovery audit event filename is not canonical"
    );
    Ok(())
}

fn chain_root(root: &Path) -> PathBuf {
    root.join("audit-chain")
}

fn entry_path(root: &Path, sequence: u64) -> PathBuf {
    chain_root(root).join(format!("{sequence:020}.json"))
}

fn internal_head_path(root: &Path) -> PathBuf {
    chain_root(root).join("head.json")
}

fn staging_root(root: &Path) -> PathBuf {
    root.join(".audit-transition")
}

fn staging_event_path(root: &Path, filename: &str) -> PathBuf {
    staging_root(root).join(filename)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&read_bounded(path)?)?)
}

fn read_bounded(path: &Path) -> anyhow::Result<Vec<u8>> {
    enforce_private_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file(),
        "Recovery audit chain path is not a file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_RECOVERY_DOCUMENT_BYTES,
        "Recovery audit chain document exceeds resource limit"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take(MAX_RECOVERY_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECOVERY_DOCUMENT_BYTES,
        "Recovery audit chain document exceeds resource limit"
    );
    Ok(bytes)
}

pub(super) fn read_event_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    read_bounded(path)
}
