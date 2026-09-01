use super::{
    MAX_RECOVERY_DOCUMENT_BYTES, atomic_write, enforce_private_file, now_unix_ns, secure_create_dir,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

const AUTH_SCHEMA: u16 = 1;
const AUTH_DOMAIN_IDENTITY: &[u8] = b"hig-recovery-identity-v1\0";
const AUTH_DOMAIN_STATE: &[u8] = b"hig-recovery-state-v1\0";
const AUTH_DOMAIN_TRANSITION_PREVIOUS: &[u8] = b"hig-recovery-transition-previous-v1\0";
const AUTH_DOMAIN_TRANSITION_TARGET: &[u8] = b"hig-recovery-transition-target-v1\0";
const AUTH_DOMAIN_BUNDLE: &[u8] = b"hig-recovery-custody-bundle-v1\0";
const AUTH_BUNDLE_SCHEMA: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum VaultRole {
    Primary,
    Mirror,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct VaultIdentity {
    pub schema: u16,
    pub lineage_id: String,
    pub vault_id: String,
    pub role: VaultRole,
    pub primary_vault_id: String,
    pub key_id: String,
    pub identity_mac: String,
}

#[derive(Serialize)]
struct UnsignedVaultIdentity<'a> {
    schema: u16,
    lineage_id: &'a str,
    vault_id: &'a str,
    role: VaultRole,
    primary_vault_id: &'a str,
    key_id: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct VaultStateSeal {
    schema: u16,
    vault_id: String,
    sequence: u64,
    catalog_generation: u64,
    config_blake3: String,
    catalog_blake3: String,
    identity_blake3: String,
    state_mac: String,
}

#[derive(Serialize)]
struct UnsignedVaultStateSeal<'a> {
    schema: u16,
    vault_id: &'a str,
    sequence: u64,
    catalog_generation: u64,
    config_blake3: &'a str,
    catalog_blake3: &'a str,
    identity_blake3: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct VaultPendingTransition {
    schema: u16,
    vault_id: String,
    previous_sequence: u64,
    previous_state_mac: Option<String>,
    previous_key_id: String,
    target_key_id: String,
    write_config: bool,
    write_catalog: bool,
    write_identity: bool,
    target: VaultStateSeal,
    transition_mac: String,
    target_transition_mac: String,
}

#[derive(Serialize)]
struct UnsignedVaultPendingTransition<'a> {
    schema: u16,
    vault_id: &'a str,
    previous_sequence: u64,
    previous_state_mac: Option<&'a str>,
    previous_key_id: &'a str,
    target_key_id: &'a str,
    write_config: bool,
    write_catalog: bool,
    write_identity: bool,
    target: &'a VaultStateSeal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RecoveryAuthBundle {
    schema: u16,
    exported_unix_ns: i128,
    lineage_id: String,
    vault_id: String,
    key_id: String,
    key_hex: String,
    checkpoint: VaultStateSeal,
    bundle_mac: String,
}

#[derive(Serialize)]
struct UnsignedRecoveryAuthBundle<'a> {
    schema: u16,
    exported_unix_ns: i128,
    lineage_id: &'a str,
    vault_id: &'a str,
    key_id: &'a str,
    key_hex: &'a str,
    checkpoint: &'a VaultStateSeal,
}

#[derive(Default)]
struct TransitionDocuments<'a> {
    config: Option<&'a [u8]>,
    catalog: Option<&'a [u8]>,
    identity: Option<&'a [u8]>,
}

pub(super) fn initialize_primary_identity(root: &Path) -> anyhow::Result<VaultIdentity> {
    let identity_path = identity_path(root);
    if identity_path.exists() {
        anyhow::ensure!(
            !state_path(root).exists(),
            "recovery Vault identity already belongs to initialized state"
        );
        return require_primary(root);
    }
    let lineage_id = hex::encode(crate::random_bytes::<16>());
    let vault_id = hex::encode(crate::random_bytes::<16>());
    let key = crate::random_bytes::<32>();
    let key_id = blake3::hash(&key).to_hex().to_string();
    write_new_key(root, &lineage_id, &key_id, &key)?;
    write_identity(
        root,
        &lineage_id,
        &vault_id,
        VaultRole::Primary,
        &vault_id,
        &key,
    )
}

pub(super) fn initialize_mirror_identity(
    root: &Path,
    primary: &VaultIdentity,
    allow_rebind: bool,
) -> anyhow::Result<VaultIdentity> {
    let key = load_key(root, &primary.lineage_id, &primary.key_id)?;
    validate_identity(primary, &key)?;
    if identity_path(root).exists() {
        let existing = load_identity(root)?;
        anyhow::ensure!(
            existing.lineage_id == primary.lineage_id,
            "recovery mirror belongs to a different authenticated lineage"
        );
        anyhow::ensure!(
            existing.role == VaultRole::Mirror,
            "recovery mirror target is an authenticated primary Vault"
        );
        if existing.primary_vault_id == primary.vault_id {
            return Ok(existing);
        }
        anyhow::ensure!(
            allow_rebind,
            "recovery mirror is bound to a different authenticated primary"
        );
        let rebound = build_identity(
            &existing.lineage_id,
            &existing.vault_id,
            VaultRole::Mirror,
            &primary.vault_id,
            &key,
        )?;
        let bytes = serde_json::to_vec_pretty(&rebound)?;
        let generation = current_catalog_generation(root, &existing, &key)?;
        commit_transition(
            root,
            &existing,
            &key,
            generation,
            TransitionDocuments {
                identity: Some(&bytes),
                ..TransitionDocuments::default()
            },
        )?;
        return Ok(rebound);
    }
    let vault_id = hex::encode(crate::random_bytes::<16>());
    write_identity(
        root,
        &primary.lineage_id,
        &vault_id,
        VaultRole::Mirror,
        &primary.vault_id,
        &key,
    )
}

pub(super) fn load_identity(root: &Path) -> anyhow::Result<VaultIdentity> {
    let path = identity_path(root);
    anyhow::ensure!(
        path.exists(),
        "Recovery Vault is not authenticated; run `hig recovery migrate-auth` before use"
    );
    enforce_private_file(&path)?;
    let identity: VaultIdentity = serde_json::from_slice(&read_bounded(&path)?)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    validate_identity(&identity, &key)?;
    Ok(identity)
}

pub(super) fn is_authenticated(root: &Path) -> bool {
    identity_path(root).exists()
}

pub(super) fn require_primary(root: &Path) -> anyhow::Result<VaultIdentity> {
    let identity = load_identity(root)?;
    anyhow::ensure!(
        identity.role == VaultRole::Primary,
        "operation requires an authenticated primary Recovery Vault"
    );
    anyhow::ensure!(
        identity.primary_vault_id == identity.vault_id,
        "authenticated primary identity is inconsistent"
    );
    Ok(identity)
}

pub(super) fn require_mirror_for(
    root: &Path,
    primary: &VaultIdentity,
) -> anyhow::Result<VaultIdentity> {
    let identity = load_identity(root)?;
    anyhow::ensure!(
        identity.role == VaultRole::Mirror
            && identity.lineage_id == primary.lineage_id
            && identity.primary_vault_id == primary.vault_id,
        "Recovery Vault is not an authenticated mirror of this primary"
    );
    Ok(identity)
}

pub(super) fn promote_identity(
    root: &Path,
    catalog_generation: u64,
) -> anyhow::Result<VaultIdentity> {
    let existing = load_identity(root)?;
    if existing.role == VaultRole::Primary && existing.primary_vault_id == existing.vault_id {
        return Ok(existing);
    }
    let key = load_key(root, &existing.lineage_id, &existing.key_id)?;
    let promoted = build_identity(
        &existing.lineage_id,
        &existing.vault_id,
        VaultRole::Primary,
        &existing.vault_id,
        &key,
    )?;
    let bytes = serde_json::to_vec_pretty(&promoted)?;
    commit_transition(
        root,
        &existing,
        &key,
        catalog_generation,
        TransitionDocuments {
            identity: Some(&bytes),
            ..TransitionDocuments::default()
        },
    )?;
    Ok(promoted)
}

pub(super) fn create_rotation_key(root: &Path) -> anyhow::Result<String> {
    let identity = load_identity(root)?;
    let key = crate::random_bytes::<32>();
    let key_id = blake3::hash(&key).to_hex().to_string();
    write_new_key(root, &identity.lineage_id, &key_id, &key)?;
    Ok(key_id)
}

pub(super) fn rotate_identity_key(
    root: &Path,
    target_key_id: &str,
    catalog_generation: u64,
) -> anyhow::Result<(VaultIdentity, VaultIdentity)> {
    let existing = load_identity(root)?;
    if existing.key_id == target_key_id {
        return Ok((existing.clone(), existing));
    }
    let previous_key = load_key(root, &existing.lineage_id, &existing.key_id)?;
    let target_key = load_key(root, &existing.lineage_id, target_key_id)?;
    let rotated = build_identity(
        &existing.lineage_id,
        &existing.vault_id,
        existing.role,
        &existing.primary_vault_id,
        &target_key,
    )?;
    let bytes = serde_json::to_vec_pretty(&rotated)?;
    commit_transition_with_target_key(
        root,
        &existing,
        &previous_key,
        &target_key,
        target_key_id,
        catalog_generation,
        TransitionDocuments {
            identity: Some(&bytes),
            ..TransitionDocuments::default()
        },
    )?;
    Ok((existing, rotated))
}

pub(super) fn initialize_state(
    root: &Path,
    catalog_generation: u64,
    config: &[u8],
    catalog: &[u8],
) -> anyhow::Result<()> {
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    anyhow::ensure!(
        !external_state_path(root, &identity.vault_id)?.exists(),
        "authenticated Recovery Vault state already exists"
    );
    commit_transition(
        root,
        &identity,
        &key,
        catalog_generation,
        TransitionDocuments {
            config: Some(config),
            catalog: Some(catalog),
            identity: None,
        },
    )
}

pub(super) fn write_catalog(
    root: &Path,
    catalog_generation: u64,
    catalog: &[u8],
) -> anyhow::Result<()> {
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    commit_transition(
        root,
        &identity,
        &key,
        catalog_generation,
        TransitionDocuments {
            catalog: Some(catalog),
            ..TransitionDocuments::default()
        },
    )
}

pub(super) fn write_config(
    root: &Path,
    catalog_generation: u64,
    config: &[u8],
) -> anyhow::Result<()> {
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    commit_transition(
        root,
        &identity,
        &key,
        catalog_generation,
        TransitionDocuments {
            config: Some(config),
            ..TransitionDocuments::default()
        },
    )
}

pub(super) fn verify_state(root: &Path, catalog_generation: u64) -> anyhow::Result<()> {
    let initial_identity = load_identity(root)?;
    let initial_key = load_key(root, &initial_identity.lineage_id, &initial_identity.key_id)?;
    recover_pending_transition(root, &initial_identity, &initial_key)?;
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    let vault_path = state_path(root);
    let external_path = external_state_path(root, &identity.vault_id)?;
    anyhow::ensure!(
        vault_path.exists() && external_path.exists(),
        "authenticated Recovery Vault checkpoint is missing"
    );
    let vault = read_state(&vault_path)?;
    let external = read_state(&external_path)?;
    validate_state_seal(&vault, &identity, &key)?;
    validate_state_seal(&external, &identity, &key)?;
    anyhow::ensure!(
        vault == external,
        "Recovery Vault checkpoint does not match the external monotonic checkpoint"
    );
    verify_seal_files(root, &vault)?;
    anyhow::ensure!(
        vault.catalog_generation == catalog_generation,
        "Recovery Vault catalog generation does not match its checkpoint"
    );
    Ok(())
}

pub(super) fn export_custody_bundle(
    root: &Path,
    output: &Path,
) -> anyhow::Result<(VaultIdentity, u64)> {
    recover_state(root)?;
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    let checkpoint = read_state(&external_state_path(root, &identity.vault_id)?)?;
    validate_state_seal(&checkpoint, &identity, &key)?;
    verify_seal_files(root, &checkpoint)?;
    anyhow::ensure!(
        read_state(&state_path(root))? == checkpoint,
        "Recovery Vault local checkpoint differs from custody state"
    );
    let mut bundle = RecoveryAuthBundle {
        schema: AUTH_BUNDLE_SCHEMA,
        exported_unix_ns: now_unix_ns(),
        lineage_id: identity.lineage_id.clone(),
        vault_id: identity.vault_id.clone(),
        key_id: identity.key_id.clone(),
        key_hex: hex::encode(key),
        checkpoint,
        bundle_mac: String::new(),
    };
    bundle.bundle_mac = bundle_mac(&bundle, &key)?;
    write_private_new_file(output, &serde_json::to_vec_pretty(&bundle)?)?;
    Ok((identity, bundle.checkpoint.sequence))
}

pub(super) fn import_custody_bundle(
    root: &Path,
    input: &Path,
) -> anyhow::Result<(VaultIdentity, u64)> {
    enforce_private_file(input)?;
    let bundle: RecoveryAuthBundle = serde_json::from_slice(&read_bounded(input)?)?;
    let key = validate_bundle(&bundle)?;
    let identity: VaultIdentity = serde_json::from_slice(&read_bounded(&identity_path(root))?)?;
    validate_identity(&identity, &key)?;
    anyhow::ensure!(
        identity.lineage_id == bundle.lineage_id && identity.vault_id == bundle.vault_id,
        "Recovery custody bundle does not belong to this Vault"
    );
    validate_state_seal(&bundle.checkpoint, &identity, &key)?;
    verify_seal_files(root, &bundle.checkpoint)?;
    anyhow::ensure!(
        read_state(&state_path(root))? == bundle.checkpoint,
        "Recovery custody bundle checkpoint does not match local Vault state"
    );

    let key_path = key_path(root, &bundle.lineage_id, &bundle.key_id)?;
    if key_path.exists() {
        anyhow::ensure!(
            load_key(root, &bundle.lineage_id, &bundle.key_id)? == key,
            "Recovery custody key conflicts with the installed key"
        );
    } else {
        write_new_key(root, &bundle.lineage_id, &bundle.key_id, &key)?;
    }
    let external_path = external_state_path(root, &identity.vault_id)?;
    if external_path.exists() {
        let existing = read_state(&external_path)?;
        validate_state_seal(&existing, &identity, &key)?;
        anyhow::ensure!(
            existing.sequence <= bundle.checkpoint.sequence,
            "Recovery custody bundle is older than the installed monotonic checkpoint"
        );
        if existing.sequence == bundle.checkpoint.sequence {
            anyhow::ensure!(
                existing == bundle.checkpoint,
                "Recovery custody checkpoint conflicts at the same sequence"
            );
        }
    }
    atomic_write(
        &external_path,
        &serde_json::to_vec_pretty(&bundle.checkpoint)?,
    )?;
    Ok((identity, bundle.checkpoint.sequence))
}

pub(super) fn recover_state(root: &Path) -> anyhow::Result<()> {
    if !identity_path(root).exists() {
        return Ok(());
    }
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    recover_pending_transition(root, &identity, &key)
}

fn commit_transition(
    root: &Path,
    identity: &VaultIdentity,
    key: &[u8; 32],
    catalog_generation: u64,
    documents: TransitionDocuments<'_>,
) -> anyhow::Result<()> {
    commit_transition_with_target_key(
        root,
        identity,
        key,
        key,
        &identity.key_id,
        catalog_generation,
        documents,
    )
}

fn commit_transition_with_target_key(
    root: &Path,
    identity: &VaultIdentity,
    previous_key: &[u8; 32],
    target_key: &[u8; 32],
    target_key_id: &str,
    catalog_generation: u64,
    documents: TransitionDocuments<'_>,
) -> anyhow::Result<()> {
    validate_hash_id("target key", target_key_id)?;
    anyhow::ensure!(
        blake3::hash(target_key).to_hex().as_str() == target_key_id,
        "Recovery Vault target key identity mismatch"
    );
    recover_pending_transition(root, identity, previous_key)?;
    let external_path = external_state_path(root, &identity.vault_id)?;
    let current = if external_path.exists() {
        let current = read_state(&external_path)?;
        validate_state_seal(&current, identity, previous_key)?;
        verify_seal_files(root, &current)?;
        anyhow::ensure!(
            state_path(root).exists() && read_state(&state_path(root))? == current,
            "Recovery Vault local checkpoint differs from external state"
        );
        Some(current)
    } else {
        anyhow::ensure!(
            !state_path(root).exists(),
            "Recovery Vault external monotonic checkpoint is missing"
        );
        None
    };
    anyhow::ensure!(
        documents.config.is_some() || documents.catalog.is_some() || documents.identity.is_some(),
        "Recovery Vault transition contains no document changes"
    );

    prepare_transition_staging(root, &documents)?;
    let mut target = VaultStateSeal {
        schema: AUTH_SCHEMA,
        vault_id: identity.vault_id.clone(),
        sequence: current.as_ref().map_or(Ok(1), |state| {
            state
                .sequence
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("recovery state sequence exhausted"))
        })?,
        catalog_generation,
        config_blake3: target_file_hash(root, "config.json", documents.config)?,
        catalog_blake3: target_file_hash(root, "catalog.json", documents.catalog)?,
        identity_blake3: target_file_hash(root, "identity.json", documents.identity)?,
        state_mac: String::new(),
    };
    target.state_mac = state_mac(&target, target_key)?;
    let mut pending = VaultPendingTransition {
        schema: AUTH_SCHEMA,
        vault_id: identity.vault_id.clone(),
        previous_sequence: current.as_ref().map_or(0, |state| state.sequence),
        previous_state_mac: current.as_ref().map(|state| state.state_mac.clone()),
        previous_key_id: identity.key_id.clone(),
        target_key_id: target_key_id.to_string(),
        write_config: documents.config.is_some(),
        write_catalog: documents.catalog.is_some(),
        write_identity: documents.identity.is_some(),
        target,
        transition_mac: String::new(),
        target_transition_mac: String::new(),
    };
    pending.transition_mac =
        transition_mac(&pending, previous_key, AUTH_DOMAIN_TRANSITION_PREVIOUS)?;
    pending.target_transition_mac =
        transition_mac(&pending, target_key, AUTH_DOMAIN_TRANSITION_TARGET)?;
    let pending_path = pending_path(root, &identity.vault_id)?;
    atomic_write(&pending_path, &serde_json::to_vec_pretty(&pending)?)?;
    finish_pending_transition(
        root,
        identity,
        previous_key,
        target_key,
        &pending,
        current.as_ref(),
    )
}

fn recover_pending_transition(
    root: &Path,
    identity: &VaultIdentity,
    _identity_key: &[u8; 32],
) -> anyhow::Result<()> {
    let pending_path = pending_path(root, &identity.vault_id)?;
    let external_path = external_state_path(root, &identity.vault_id)?;
    if !pending_path.exists() {
        cleanup_staging(root)?;
        return Ok(());
    }
    let pending: VaultPendingTransition = serde_json::from_slice(&read_bounded(&pending_path)?)?;
    let previous_key = load_key(root, &identity.lineage_id, &pending.previous_key_id)?;
    let target_key = load_key(root, &identity.lineage_id, &pending.target_key_id)?;
    validate_pending_transition(&pending, identity, &previous_key, &target_key)?;
    let current = if external_path.exists() {
        let current = read_state(&external_path)?;
        if current == pending.target {
            validate_state_seal(&current, identity, &target_key)?;
        } else {
            validate_state_seal(&current, identity, &previous_key)?;
        }
        Some(current)
    } else {
        None
    };
    if current.as_ref() == Some(&pending.target) {
        verify_seal_files(root, &pending.target)?;
        atomic_write(
            &state_path(root),
            &serde_json::to_vec_pretty(&pending.target)?,
        )?;
        cleanup_pending(root, &pending_path)?;
        return Ok(());
    }
    finish_pending_transition(
        root,
        identity,
        &previous_key,
        &target_key,
        &pending,
        current.as_ref(),
    )
}

fn finish_pending_transition(
    root: &Path,
    identity: &VaultIdentity,
    previous_key: &[u8; 32],
    target_key: &[u8; 32],
    pending: &VaultPendingTransition,
    current: Option<&VaultStateSeal>,
) -> anyhow::Result<()> {
    validate_pending_transition(pending, identity, previous_key, target_key)?;
    match current {
        Some(current) => {
            anyhow::ensure!(
                current.sequence == pending.previous_sequence
                    && pending.previous_state_mac.as_deref() == Some(current.state_mac.as_str())
                    && pending.target.sequence == current.sequence.saturating_add(1),
                "Recovery Vault pending transition is not anchored to the external checkpoint"
            );
        }
        None => anyhow::ensure!(
            pending.previous_sequence == 0
                && pending.previous_state_mac.is_none()
                && pending.target.sequence == 1,
            "Recovery Vault initial transition is not canonical"
        ),
    }
    publish_staged_document(
        root,
        "config.json",
        pending.write_config,
        &pending.target.config_blake3,
    )?;
    super::recovery_failpoint("auth_after_config_publication")?;
    publish_staged_document(
        root,
        "catalog.json",
        pending.write_catalog,
        &pending.target.catalog_blake3,
    )?;
    super::recovery_failpoint("auth_after_catalog_publication")?;
    publish_staged_document(
        root,
        "identity.json",
        pending.write_identity,
        &pending.target.identity_blake3,
    )?;
    super::recovery_failpoint("auth_after_identity_publication")?;
    verify_seal_files(root, &pending.target)?;
    let bytes = serde_json::to_vec_pretty(&pending.target)?;
    atomic_write(&state_path(root), &bytes)?;
    super::recovery_failpoint("auth_after_local_checkpoint")?;
    atomic_write(&external_state_path(root, &identity.vault_id)?, &bytes)?;
    super::recovery_failpoint("auth_after_external_checkpoint")?;
    cleanup_pending(root, &pending_path(root, &identity.vault_id)?)
}

fn prepare_transition_staging(
    root: &Path,
    documents: &TransitionDocuments<'_>,
) -> anyhow::Result<()> {
    cleanup_staging(root)?;
    let staging = staging_root(root);
    secure_create_dir(&staging)?;
    for (name, bytes) in [
        ("config.json", documents.config),
        ("catalog.json", documents.catalog),
        ("identity.json", documents.identity),
    ] {
        if let Some(bytes) = bytes {
            atomic_write(&staging.join(name), bytes)?;
        }
    }
    Ok(())
}

fn publish_staged_document(
    root: &Path,
    name: &str,
    changed: bool,
    expected_hash: &str,
) -> anyhow::Result<()> {
    if changed {
        let bytes = read_bounded(&staging_root(root).join(name))?;
        anyhow::ensure!(
            blake3::hash(&bytes).to_hex().as_str() == expected_hash,
            "Recovery Vault staged transition document authentication failed"
        );
        atomic_write(&root.join(name), &bytes)?;
    }
    Ok(())
}

fn cleanup_pending(root: &Path, pending_path: &Path) -> anyhow::Result<()> {
    if pending_path.exists() {
        fs::remove_file(pending_path)?;
    }
    cleanup_staging(root)
}

fn cleanup_staging(root: &Path) -> anyhow::Result<()> {
    let staging = staging_root(root);
    if staging.exists() {
        let metadata = fs::symlink_metadata(&staging)?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Recovery Vault transition staging is not a physical directory"
        );
        fs::remove_dir_all(staging)?;
    }
    Ok(())
}

fn target_file_hash(root: &Path, name: &str, bytes: Option<&[u8]>) -> anyhow::Result<String> {
    match bytes {
        Some(bytes) => Ok(blake3::hash(bytes).to_hex().to_string()),
        None => file_hash(&root.join(name)),
    }
}

fn current_catalog_generation(
    root: &Path,
    identity: &VaultIdentity,
    key: &[u8; 32],
) -> anyhow::Result<u64> {
    recover_pending_transition(root, identity, key)?;
    let identity = load_identity(root)?;
    let key = load_key(root, &identity.lineage_id, &identity.key_id)?;
    let state = read_state(&external_state_path(root, &identity.vault_id)?)?;
    validate_state_seal(&state, &identity, &key)?;
    verify_seal_files(root, &state)?;
    Ok(state.catalog_generation)
}

fn write_identity(
    root: &Path,
    lineage_id: &str,
    vault_id: &str,
    role: VaultRole,
    primary_vault_id: &str,
    key: &[u8; 32],
) -> anyhow::Result<VaultIdentity> {
    let identity = build_identity(lineage_id, vault_id, role, primary_vault_id, key)?;
    atomic_write(&identity_path(root), &serde_json::to_vec_pretty(&identity)?)?;
    Ok(identity)
}

fn build_identity(
    lineage_id: &str,
    vault_id: &str,
    role: VaultRole,
    primary_vault_id: &str,
    key: &[u8; 32],
) -> anyhow::Result<VaultIdentity> {
    validate_hex_id("lineage", lineage_id)?;
    validate_hex_id("vault", vault_id)?;
    validate_hex_id("primary Vault", primary_vault_id)?;
    let key_id = blake3::hash(key).to_hex().to_string();
    let mut identity = VaultIdentity {
        schema: AUTH_SCHEMA,
        lineage_id: lineage_id.to_string(),
        vault_id: vault_id.to_string(),
        role,
        primary_vault_id: primary_vault_id.to_string(),
        key_id,
        identity_mac: String::new(),
    };
    identity.identity_mac = identity_mac(&identity, key)?;
    Ok(identity)
}

fn validate_identity(identity: &VaultIdentity, key: &[u8; 32]) -> anyhow::Result<()> {
    anyhow::ensure!(
        identity.schema == AUTH_SCHEMA,
        "unsupported Recovery Vault auth schema"
    );
    validate_hex_id("lineage", &identity.lineage_id)?;
    validate_hex_id("vault", &identity.vault_id)?;
    validate_hex_id("primary Vault", &identity.primary_vault_id)?;
    validate_hash_id("key", &identity.key_id)?;
    anyhow::ensure!(
        identity.key_id == blake3::hash(key).to_hex().as_str(),
        "Recovery Vault authentication key identity mismatch"
    );
    anyhow::ensure!(
        identity.identity_mac == identity_mac(identity, key)?,
        "Recovery Vault identity authentication failed"
    );
    Ok(())
}

fn validate_state_seal(
    seal: &VaultStateSeal,
    identity: &VaultIdentity,
    key: &[u8; 32],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        seal.schema == AUTH_SCHEMA && seal.vault_id == identity.vault_id,
        "Recovery Vault checkpoint identity mismatch"
    );
    anyhow::ensure!(
        seal.sequence > 0,
        "Recovery Vault checkpoint sequence is invalid"
    );
    anyhow::ensure!(
        seal.state_mac == state_mac(seal, key)?,
        "Recovery Vault checkpoint authentication failed"
    );
    Ok(())
}

fn validate_pending_transition(
    pending: &VaultPendingTransition,
    identity: &VaultIdentity,
    previous_key: &[u8; 32],
    target_key: &[u8; 32],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        pending.schema == AUTH_SCHEMA && pending.vault_id == identity.vault_id,
        "Recovery Vault pending transition identity mismatch"
    );
    anyhow::ensure!(
        pending.write_config || pending.write_catalog || pending.write_identity,
        "Recovery Vault pending transition contains no changes"
    );
    validate_hash_id("previous key", &pending.previous_key_id)?;
    validate_hash_id("target key", &pending.target_key_id)?;
    anyhow::ensure!(
        blake3::hash(previous_key).to_hex().as_str() == pending.previous_key_id
            && blake3::hash(target_key).to_hex().as_str() == pending.target_key_id,
        "Recovery Vault pending transition key identity mismatch"
    );
    validate_state_seal(&pending.target, identity, target_key)?;
    anyhow::ensure!(
        pending.transition_mac
            == transition_mac(pending, previous_key, AUTH_DOMAIN_TRANSITION_PREVIOUS)?,
        "Recovery Vault pending transition authentication failed"
    );
    anyhow::ensure!(
        pending.target_transition_mac
            == transition_mac(pending, target_key, AUTH_DOMAIN_TRANSITION_TARGET)?,
        "Recovery Vault target transition authentication failed"
    );
    Ok(())
}

fn verify_seal_files(root: &Path, seal: &VaultStateSeal) -> anyhow::Result<()> {
    anyhow::ensure!(
        seal.config_blake3 == file_hash(&root.join("config.json"))?
            && seal.catalog_blake3 == file_hash(&root.join("catalog.json"))?
            && seal.identity_blake3 == file_hash(&identity_path(root))?,
        "authenticated Recovery Vault state does not match its checkpoint"
    );
    Ok(())
}

fn identity_mac(identity: &VaultIdentity, key: &[u8; 32]) -> anyhow::Result<String> {
    keyed_json_hash(
        key,
        AUTH_DOMAIN_IDENTITY,
        &UnsignedVaultIdentity {
            schema: identity.schema,
            lineage_id: &identity.lineage_id,
            vault_id: &identity.vault_id,
            role: identity.role,
            primary_vault_id: &identity.primary_vault_id,
            key_id: &identity.key_id,
        },
    )
}

fn state_mac(seal: &VaultStateSeal, key: &[u8; 32]) -> anyhow::Result<String> {
    keyed_json_hash(
        key,
        AUTH_DOMAIN_STATE,
        &UnsignedVaultStateSeal {
            schema: seal.schema,
            vault_id: &seal.vault_id,
            sequence: seal.sequence,
            catalog_generation: seal.catalog_generation,
            config_blake3: &seal.config_blake3,
            catalog_blake3: &seal.catalog_blake3,
            identity_blake3: &seal.identity_blake3,
        },
    )
}

fn transition_mac(
    transition: &VaultPendingTransition,
    key: &[u8; 32],
    domain: &[u8],
) -> anyhow::Result<String> {
    keyed_json_hash(
        key,
        domain,
        &UnsignedVaultPendingTransition {
            schema: transition.schema,
            vault_id: &transition.vault_id,
            previous_sequence: transition.previous_sequence,
            previous_state_mac: transition.previous_state_mac.as_deref(),
            previous_key_id: &transition.previous_key_id,
            target_key_id: &transition.target_key_id,
            write_config: transition.write_config,
            write_catalog: transition.write_catalog,
            write_identity: transition.write_identity,
            target: &transition.target,
        },
    )
}

fn bundle_mac(bundle: &RecoveryAuthBundle, key: &[u8; 32]) -> anyhow::Result<String> {
    keyed_json_hash(
        key,
        AUTH_DOMAIN_BUNDLE,
        &UnsignedRecoveryAuthBundle {
            schema: bundle.schema,
            exported_unix_ns: bundle.exported_unix_ns,
            lineage_id: &bundle.lineage_id,
            vault_id: &bundle.vault_id,
            key_id: &bundle.key_id,
            key_hex: &bundle.key_hex,
            checkpoint: &bundle.checkpoint,
        },
    )
}

fn validate_bundle(bundle: &RecoveryAuthBundle) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(
        bundle.schema == AUTH_BUNDLE_SCHEMA,
        "unsupported Recovery custody bundle schema"
    );
    validate_hex_id("lineage", &bundle.lineage_id)?;
    validate_hex_id("vault", &bundle.vault_id)?;
    validate_hash_id("key", &bundle.key_id)?;
    let key: [u8; 32] = hex::decode(&bundle.key_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Recovery custody bundle key must contain 32 bytes"))?;
    anyhow::ensure!(
        blake3::hash(&key).to_hex().as_str() == bundle.key_id,
        "Recovery custody bundle key identity mismatch"
    );
    anyhow::ensure!(
        bundle.bundle_mac == bundle_mac(bundle, &key)?,
        "Recovery custody bundle authentication failed"
    );
    Ok(key)
}

fn keyed_json_hash(
    key: &[u8; 32],
    domain: &[u8],
    value: &impl Serialize,
) -> anyhow::Result<String> {
    let mut input = domain.to_vec();
    input.extend(serde_json::to_vec(value)?);
    Ok(blake3::keyed_hash(key, &input).to_hex().to_string())
}

fn write_new_key(
    root: &Path,
    lineage_id: &str,
    key_id: &str,
    key: &[u8; 32],
) -> anyhow::Result<()> {
    let path = key_path(root, lineage_id, key_id)?;
    if let Some(parent) = path.parent() {
        secure_create_dir(parent)?;
    }
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
    let mut file = options.open(&path)?;
    use std::io::Write;
    file.write_all(hex::encode(key).as_bytes())?;
    file.sync_all()?;
    enforce_private_file(&path)?;
    Ok(())
}

fn write_private_new_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(!path.exists(), "Recovery custody output already exists");
    if let Some(parent) = path.parent() {
        if parent.exists() {
            let metadata = fs::symlink_metadata(parent)?;
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Recovery custody output parent is not a physical directory"
            );
        } else {
            secure_create_dir(parent)?;
        }
    }
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
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    enforce_private_file(path)?;
    Ok(())
}

fn load_key(root: &Path, lineage_id: &str, key_id: &str) -> anyhow::Result<[u8; 32]> {
    validate_hex_id("lineage", lineage_id)?;
    validate_hash_id("key", key_id)?;
    let path = key_path(root, lineage_id, key_id)?;
    anyhow::ensure!(
        path.exists(),
        "Recovery Vault external authentication key is unavailable; import the lineage key before promotion"
    );
    enforce_private_file(&path)?;
    let value = String::from_utf8(read_bounded(&path)?)?;
    let bytes = hex::decode(value.trim())?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Recovery Vault authentication key must contain 32 bytes"))
}

fn auth_root(_root: &Path) -> anyhow::Result<PathBuf> {
    #[cfg(test)]
    {
        return Ok(_root
            .parent()
            .ok_or_else(|| anyhow::anyhow!("test Vault has no parent"))?
            .join(".hig-recovery-auth"));
    }
    #[cfg(not(test))]
    {
        if let Some(configured) = std::env::var_os("HIG_RECOVERY_AUTH_DIR")
            && !configured.is_empty()
        {
            return Ok(PathBuf::from(configured));
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or_else(|| {
                anyhow::anyhow!("HOME or USERPROFILE is required for Recovery Vault authentication")
            })?;
        Ok(PathBuf::from(home).join(".hig").join("recovery-auth"))
    }
}

fn key_path(root: &Path, lineage_id: &str, key_id: &str) -> anyhow::Result<PathBuf> {
    Ok(auth_root(root)?
        .join("keys")
        .join(format!("{lineage_id}.{key_id}.key")))
}

fn external_state_path(root: &Path, vault_id: &str) -> anyhow::Result<PathBuf> {
    Ok(auth_root(root)?
        .join("checkpoints")
        .join(format!("{vault_id}.state.json")))
}

fn pending_path(root: &Path, vault_id: &str) -> anyhow::Result<PathBuf> {
    Ok(auth_root(root)?
        .join("checkpoints")
        .join(format!("{vault_id}.pending.json")))
}

fn identity_path(root: &Path) -> PathBuf {
    root.join("identity.json")
}

fn state_path(root: &Path) -> PathBuf {
    root.join("state.json")
}

fn staging_root(root: &Path) -> PathBuf {
    root.join(".auth-transition")
}

fn file_hash(path: &Path) -> anyhow::Result<String> {
    Ok(blake3::hash(&read_bounded(path)?).to_hex().to_string())
}

fn read_state(path: &Path) -> anyhow::Result<VaultStateSeal> {
    Ok(serde_json::from_slice(&read_bounded(path)?)?)
}

fn read_bounded(path: &Path) -> anyhow::Result<Vec<u8>> {
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
        "Recovery authentication path is not a file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_RECOVERY_DOCUMENT_BYTES,
        "Recovery authentication document exceeds resource limit"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take(MAX_RECOVERY_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECOVERY_DOCUMENT_BYTES,
        "Recovery authentication document exceeds resource limit"
    );
    Ok(bytes)
}

fn validate_hex_id(label: &str, value: &str) -> anyhow::Result<()> {
    let bytes = hex::decode(value)?;
    anyhow::ensure!(
        bytes.len() == 16 && hex::encode(bytes) == value,
        "Recovery {label} identity is not canonical"
    );
    Ok(())
}

fn validate_hash_id(label: &str, value: &str) -> anyhow::Result<()> {
    let bytes = hex::decode(value)?;
    anyhow::ensure!(
        bytes.len() == 32 && hex::encode(bytes) == value,
        "Recovery {label} identity is not canonical"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::{
        RecoveryRetentionPolicy, capture_recovery_point, checked_json_bytes, init_recovery_vault,
        load_catalog, load_vault_config, promote_recovery_vault, recovery_vault_config,
        with_recovery_failpoint, write_checked_json,
    };
    use crate::{init_repository, snapshot_repository};

    #[test]
    fn authenticated_transition_recovers_every_commit_interruption_window() {
        for failpoint in [
            "auth_after_config_publication",
            "auth_after_catalog_publication",
            "auth_after_identity_publication",
            "auth_after_local_checkpoint",
            "auth_after_external_checkpoint",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let vault = temp.path().join("vault");
            init_recovery_vault(Some(&vault), Vec::new()).unwrap();
            let mut config = load_vault_config(&vault).unwrap();
            config.retention.minimum_retention_days = 91;
            let generation = load_catalog(&vault).unwrap().generation;
            let bytes = checked_json_bytes(&config).unwrap();

            let error = with_recovery_failpoint(failpoint, || {
                write_config(&vault, generation, &bytes).unwrap_err()
            });
            assert!(error.to_string().contains(failpoint));

            let recovered = load_vault_config(&vault).unwrap();
            assert_eq!(recovered.retention.minimum_retention_days, 91);
            assert!(!staging_root(&vault).exists());
            let identity = load_identity(&vault).unwrap();
            assert!(!pending_path(&vault, &identity.vault_id).unwrap().exists());
            assert_eq!(
                read_state(&state_path(&vault)).unwrap(),
                read_state(&external_state_path(&vault, &identity.vault_id).unwrap()).unwrap()
            );
        }
    }

    #[test]
    fn interrupted_initial_document_publication_is_completed_on_retry() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        let error = with_recovery_failpoint("auth_after_config_publication", || {
            init_recovery_vault(Some(&vault), Vec::new()).unwrap_err()
        });
        assert!(error.to_string().contains("auth_after_config_publication"));
        assert!(vault.join("config.json").exists());

        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        load_vault_config(&vault).unwrap();
        load_catalog(&vault).unwrap();
        assert!(!staging_root(&vault).exists());
    }

    #[test]
    fn recomputed_public_checksum_cannot_bypass_authenticated_state() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let mut config = load_vault_config(&vault).unwrap();
        config.retention = RecoveryRetentionPolicy {
            minimum_retention_days: 777,
            ..config.retention
        };
        write_checked_json(&vault.join("config.json"), &config).unwrap();

        let error = recovery_vault_config(Some(&vault)).unwrap_err();
        assert!(error.to_string().contains("does not match its checkpoint"));
    }

    #[test]
    fn rollback_of_valid_local_catalog_and_checkpoint_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "one").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "one".into(), None).unwrap();
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let old_catalog = fs::read(vault.join("catalog.json")).unwrap();
        let old_state = fs::read(state_path(&vault)).unwrap();

        capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        fs::write(vault.join("catalog.json"), old_catalog).unwrap();
        fs::write(state_path(&vault), old_state).unwrap();

        let error = load_catalog(&vault).unwrap_err();
        assert!(error.to_string().contains("external monotonic checkpoint"));
    }

    #[test]
    fn forged_identity_without_external_key_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let mut identity: serde_json::Value =
            serde_json::from_slice(&fs::read(identity_path(&vault)).unwrap()).unwrap();
        identity["lineage_id"] = serde_json::Value::String("a5".repeat(16));
        identity["key_id"] = serde_json::Value::String("b6".repeat(32));
        identity["identity_mac"] = serde_json::Value::String("c7".repeat(32));
        fs::write(
            identity_path(&vault),
            serde_json::to_vec_pretty(&identity).unwrap(),
        )
        .unwrap();

        let error = load_identity(&vault).unwrap_err();
        assert!(error.to_string().contains("external authentication key"));
    }

    #[test]
    fn mirror_promotion_fails_closed_when_external_lineage_key_is_lost() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "protected").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "protected".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        let identity = load_identity(&mirror).unwrap();
        fs::remove_file(key_path(&mirror, &identity.lineage_id, &identity.key_id).unwrap())
            .unwrap();
        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&primary).unwrap();

        let error = promote_recovery_vault(Some(&mirror), Vec::new()).unwrap_err();
        assert!(error.to_string().contains("external authentication key"));
    }

    #[test]
    fn custody_bundle_restores_key_and_monotonic_checkpoint_on_a_new_host() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        let bundle = temp.path().join("vault-custody.json");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let (exported_identity, exported_sequence) =
            export_custody_bundle(&vault, &bundle).unwrap();
        fs::remove_dir_all(auth_root(&vault).unwrap()).unwrap();
        assert!(load_vault_config(&vault).is_err());

        let (imported_identity, imported_sequence) =
            import_custody_bundle(&vault, &bundle).unwrap();
        assert_eq!(imported_identity, exported_identity);
        assert_eq!(imported_sequence, exported_sequence);
        load_vault_config(&vault).unwrap();
    }

    #[test]
    fn custody_bundle_tampering_is_rejected_before_key_installation() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        let bundle = temp.path().join("vault-custody.json");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        export_custody_bundle(&vault, &bundle).unwrap();
        fs::remove_dir_all(auth_root(&vault).unwrap()).unwrap();
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
        document["checkpoint"]["sequence"] = serde_json::Value::from(99_u64);
        fs::write(&bundle, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

        let error = import_custody_bundle(&vault, &bundle).unwrap_err();
        assert!(error.to_string().contains("bundle authentication failed"));
        assert!(!auth_root(&vault).unwrap().join("keys").exists());
    }

    #[test]
    fn cross_key_rotation_recovers_every_authenticated_commit_window() {
        for failpoint in [
            "auth_after_config_publication",
            "auth_after_catalog_publication",
            "auth_after_identity_publication",
            "auth_after_local_checkpoint",
            "auth_after_external_checkpoint",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let vault = temp.path().join("vault");
            init_recovery_vault(Some(&vault), Vec::new()).unwrap();
            let previous = load_identity(&vault).unwrap();
            let generation = load_catalog(&vault).unwrap().generation;
            let target_key_id = create_rotation_key(&vault).unwrap();

            let error = with_recovery_failpoint(failpoint, || {
                rotate_identity_key(&vault, &target_key_id, generation).unwrap_err()
            });
            assert!(error.to_string().contains(failpoint));
            load_vault_config(&vault).unwrap();
            let rotated = load_identity(&vault).unwrap();
            assert_eq!(rotated.key_id, target_key_id);
            assert_ne!(rotated.key_id, previous.key_id);
            verify_state(&vault, generation).unwrap();
        }
    }
}
