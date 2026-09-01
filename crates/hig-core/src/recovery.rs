use crate::repository::{
    RepositoryReplicationReport, gc_repository_excluding_recovery_refs, repair_repository_revision,
    replicate_repository_revision, repository_revision_id, repository_root_and_config,
};
use crate::{
    RepositoryGcReport, RepositoryObjectId, RepositoryRefKind, RepositoryRestoreReport,
    RepositoryVerifyReport, repository_refs, restore_repository, verify_repository,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod audit;
mod auth;
#[cfg(test)]
mod concurrency_tests;
#[cfg(test)]
mod fault_tests;
#[cfg(test)]
mod schema_tests;

use audit::run_audited;
pub use audit::{
    RecoveryAuditActor, RecoveryAuditEvent, RecoveryAuditOperation, RecoveryAuditOutcome,
    RecoveryAuditReport, recovery_audit_log,
};
#[cfg(test)]
use audit::{atomic_write_new, begin_audit};

const VAULT_SCHEMA: u16 = 1;
const CATALOG_SCHEMA: u16 = 1;
const DOCUMENT_SCHEMA: u16 = 1;
const RECOVERY_REPORT_SCHEMA: u16 = 1;
const MAX_RECOVERY_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RECOVERY_REPOSITORIES: usize = 100_000;
const MAX_RECOVERY_POINTS_PER_REPOSITORY: usize = 100_000;
const MAX_RECOVERY_SOURCE_PATHS: usize = 1_024;
const MAX_RECOVERY_TOMBSTONES_PER_REPOSITORY: usize = 10_000;
const MAX_RECOVERY_REPLICAS_PER_POINT: usize = 1_024;
const MAX_RECOVERY_PATH_BYTES: usize = 32 * 1024;
const MAX_RECOVERY_TEXT_BYTES: usize = 4 * 1024;

#[cfg(not(test))]
#[inline]
fn recovery_failpoint(_name: &'static str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    static RECOVERY_FAILPOINT: std::cell::RefCell<Option<&'static str>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn recovery_failpoint(name: &'static str) -> anyhow::Result<()> {
    let triggered = RECOVERY_FAILPOINT.with(|armed| {
        let mut armed = armed.borrow_mut();
        if armed.as_ref().is_some_and(|candidate| *candidate == name) {
            armed.take();
            true
        } else {
            false
        }
    });
    anyhow::ensure!(!triggered, "injected recovery fault: {name}");
    Ok(())
}

#[cfg(test)]
fn with_recovery_failpoint<T>(name: &'static str, operation: impl FnOnce() -> T) -> T {
    RECOVERY_FAILPOINT.with(|armed| {
        assert!(
            armed.borrow().is_none(),
            "nested recovery failpoints are unsupported"
        );
        armed.replace(Some(name));
    });
    let result = operation();
    RECOVERY_FAILPOINT.with(|armed| {
        assert!(
            armed.borrow().is_none(),
            "recovery failpoint was not reached: {name}"
        );
    });
    result
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryVaultConfig {
    pub schema: u16,
    pub created_unix_ns: i128,
    pub mirror_roots: Vec<PathBuf>,
    #[serde(default)]
    pub retention: RecoveryRetentionPolicy,
    #[serde(default)]
    pub at_rest_policy: RecoveryAtRestPolicy,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAtRestPolicy {
    #[default]
    ExternalEncryptionRequired,
}

impl RecoveryAtRestPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExternalEncryptionRequired => "external_encryption_required",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRetentionPolicy {
    pub schema: u16,
    pub minimum_points_per_repository: u32,
    pub minimum_retention_days: u32,
    pub maximum_points_per_repository: Option<u32>,
    pub maximum_vault_bytes: Option<u64>,
}

impl Default for RecoveryRetentionPolicy {
    fn default() -> Self {
        Self {
            schema: VAULT_SCHEMA,
            minimum_points_per_repository: 10,
            minimum_retention_days: 30,
            maximum_points_per_repository: None,
            maximum_vault_bytes: None,
        }
    }
}

impl RecoveryRetentionPolicy {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema == VAULT_SCHEMA,
            "unsupported recovery retention schema"
        );
        anyhow::ensure!(
            self.maximum_points_per_repository
                .is_none_or(|maximum| maximum >= self.minimum_points_per_repository),
            "maximum recovery points cannot be less than the protected minimum"
        );
        anyhow::ensure!(
            self.maximum_vault_bytes.is_none_or(|bytes| bytes > 0),
            "maximum vault bytes must be greater than zero"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDurability {
    Captured,
    Protected,
    Degraded,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPointState {
    #[default]
    Available,
    PendingDeletion,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryTombstoneKind {
    File,
    Workspace,
    Registration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryTombstone {
    pub schema: u16,
    pub tombstone_id: [u8; 16],
    pub kind: RecoveryTombstoneKind,
    pub observed_unix_ns: i128,
    pub source_path: Option<String>,
    pub relative_path: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryReplicaStatus {
    pub vault_root: String,
    pub verified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryPoint {
    pub schema: u16,
    pub recovery_point_id: String,
    pub commit_id: RepositoryObjectId,
    pub ref_name: String,
    pub captured_unix_ns: i128,
    pub last_verified_unix_ns: i128,
    pub reachable_objects: u64,
    pub stored_objects_written: u64,
    pub stored_bytes_written: u64,
    pub durability: RecoveryDurability,
    pub replicas: Vec<RecoveryReplicaStatus>,
    pub pinned: bool,
    #[serde(default)]
    pub state: RecoveryPointState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRegistration {
    pub schema: u16,
    pub registration_id: [u8; 16],
    pub repository_id: [u8; 16],
    pub created_unix_ns: i128,
    pub updated_unix_ns: i128,
    pub source_paths: Vec<String>,
    pub recovery_points: BTreeMap<String, RecoveryPoint>,
    #[serde(default)]
    pub tombstones: Vec<RecoveryTombstone>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RecoveryCatalog {
    schema: u16,
    generation: u64,
    repositories: BTreeMap<String, RecoveryRegistration>,
}

impl Default for RecoveryCatalog {
    fn default() -> Self {
        Self {
            schema: CATALOG_SCHEMA,
            generation: 0,
            repositories: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CheckedDocument<T> {
    schema: u16,
    payload_blake3: String,
    payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryVaultInitReport {
    pub schema: u16,
    pub vault_root: String,
    pub created: bool,
    pub mirror_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRegistrationReport {
    pub schema: u16,
    pub vault_root: String,
    pub repository_id: [u8; 16],
    pub registration_id: [u8; 16],
    pub source_root: String,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryCaptureReport {
    pub schema: u16,
    pub vault_root: String,
    pub repository_id: [u8; 16],
    pub recovery_point: RecoveryPoint,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryVaultListReport {
    pub schema: u16,
    pub vault_root: String,
    pub generation: u64,
    pub repositories: Vec<RecoveryRegistration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryVaultStatusReport {
    pub schema: u16,
    pub vault_root: String,
    pub generation: u64,
    pub created_unix_ns: i128,
    pub at_rest_policy: RecoveryAtRestPolicy,
    pub configured_mirrors: u64,
    pub repositories: u64,
    pub recovery_points: u64,
    pub available_points: u64,
    pub pending_deletion_points: u64,
    pub captured_points: u64,
    pub protected_points: u64,
    pub degraded_points: u64,
    pub durability_lag_points: u64,
    pub latest_capture_unix_ns: Option<i128>,
    pub rpo_lag_millis: Option<u64>,
    pub incomplete_audit_operations: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryVerifyReport {
    pub schema: u16,
    pub vault_root: String,
    pub repository_id: [u8; 16],
    pub recovery_point_id: String,
    pub repository: RepositoryVerifyReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRestoreReport {
    pub schema: u16,
    pub vault_root: String,
    pub repository_id: [u8; 16],
    pub recovery_point_id: String,
    pub restore: RepositoryRestoreReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryPinReport {
    pub schema: u16,
    pub vault_root: String,
    pub repository_id: [u8; 16],
    pub recovery_point_id: String,
    pub pinned: bool,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryTombstoneReport {
    pub schema: u16,
    pub vault_root: String,
    pub repository_id: [u8; 16],
    pub tombstone: RecoveryTombstone,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryGcCandidate {
    pub repository_id: [u8; 16],
    pub recovery_point_id: String,
    pub captured_unix_ns: i128,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryVaultGcReport {
    pub schema: u16,
    pub vault_root: String,
    pub dry_run: bool,
    pub total_recovery_points: u64,
    pub retained_recovery_points: u64,
    pub candidate_recovery_points: u64,
    pub removed_recovery_points: u64,
    pub stored_bytes_before: u64,
    pub projected_stored_bytes: u64,
    pub policy_satisfied: bool,
    pub candidates: Vec<RecoveryGcCandidate>,
    pub repositories: BTreeMap<String, RepositoryGcReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryScrubLocationReport {
    pub schema: u16,
    pub vault_root: String,
    pub primary: bool,
    pub healthy: bool,
    pub checked_repositories: u64,
    pub checked_recovery_points: u64,
    pub checked_objects: u64,
    pub checked_raw_bytes: u64,
    pub checked_audit_events: u64,
    pub incomplete_audit_operations: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryScrubReport {
    pub schema: u16,
    pub healthy: bool,
    pub locations: Vec<RecoveryScrubLocationReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryRepairReport {
    pub schema: u16,
    pub vault_root: String,
    pub mirror_root: String,
    pub repository_id: [u8; 16],
    pub recovery_point_id: String,
    pub objects_written: u64,
    pub objects_repaired: u64,
    pub object_bytes_written: u64,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryPromotionReport {
    pub schema: u16,
    pub vault_root: String,
    pub generation_before: u64,
    pub generation_after: u64,
    pub changed: bool,
    pub repositories: u64,
    pub recovery_points: u64,
    pub mirror_roots: Vec<String>,
    pub objects_written: u64,
    pub object_bytes_written: u64,
    pub durability: RecoveryDurability,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuthCustodyReport {
    pub schema: u16,
    pub vault_root: String,
    pub custody_file: String,
    pub lineage_id: String,
    pub vault_id: String,
    pub key_id: String,
    pub checkpoint_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuthMigrationReport {
    pub schema: u16,
    pub vault_root: String,
    pub created: bool,
    pub lineage_id: String,
    pub vault_id: String,
    pub key_id: String,
    pub migrated_mirrors: Vec<String>,
    pub verified_repositories: u64,
    pub verified_recovery_points: u64,
    pub verified_objects: u64,
    pub verified_raw_bytes: u64,
    pub verified_audit_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuthRotationReport {
    pub schema: u16,
    pub vault_root: String,
    pub lineage_id: String,
    pub previous_key_ids: Vec<String>,
    pub key_id: String,
    pub rotated_vaults: Vec<String>,
    pub old_keys_retained: bool,
}

pub fn default_recovery_vault_root() -> anyhow::Result<PathBuf> {
    if let Some(value) = std::env::var_os("HIG_RECOVERY_VAULT")
        && !value.is_empty()
    {
        return absolute_path(Path::new(&value));
    }

    #[cfg(target_os = "macos")]
    {
        let home = required_environment_path("HOME")?;
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("HIG")
            .join("recovery-vault"))
    }
    #[cfg(windows)]
    {
        Ok(required_environment_path("LOCALAPPDATA")?
            .join("HIG")
            .join("recovery-vault"))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(root) = std::env::var_os("XDG_DATA_HOME")
            && !root.is_empty()
        {
            return Ok(PathBuf::from(root).join("hig").join("recovery-vault"));
        }
        Ok(required_environment_path("HOME")?
            .join(".local")
            .join("share")
            .join("hig")
            .join("recovery-vault"))
    }
}

pub fn init_recovery_vault(
    requested_root: Option<&Path>,
    mirror_roots: Vec<PathBuf>,
) -> anyhow::Result<RecoveryVaultInitReport> {
    let root = resolve_vault_root(requested_root)?;
    let existed = vault_config_path(&root).exists();
    let normalized_mirrors = normalize_mirror_roots(&root, mirror_roots)?;
    secure_create_dir(&root)?;
    secure_create_dir(&root.join("events"))?;
    secure_create_dir(&root.join("locks"))?;
    let _lock = lock_vault(&root)?;
    let existing_config = if existed {
        Some(load_vault_config(&root)?)
    } else {
        None
    };
    let generation_before = if catalog_path(&root).exists() {
        enforce_private_file(&catalog_path(&root))?;
        Some(load_catalog(&root)?.generation)
    } else {
        None
    };
    let effective_mirrors = match existing_config.as_ref() {
        Some(config) if normalized_mirrors.is_empty() => config.mirror_roots.clone(),
        _ => normalized_mirrors,
    };
    if let Some(config) = existing_config.as_ref()
        && config.mirror_roots != effective_mirrors
    {
        let catalog = load_catalog(&root)?;
        anyhow::ensure!(
            catalog
                .repositories
                .values()
                .all(|registration| registration.recovery_points.is_empty()),
            "cannot change mirrors with recovery init after capture; use recovery promote so every existing point is replicated and verified"
        );
    }
    let details = BTreeMap::from([
        ("existing".to_string(), existed.to_string()),
        (
            "mirror_count".to_string(),
            effective_mirrors.len().to_string(),
        ),
    ]);
    run_audited(
        &root,
        RecoveryAuditOperation::VaultInitialize,
        generation_before,
        None,
        None,
        details,
        || {
            let primary_identity = initialize_vault_root(&root, effective_mirrors.clone())?;
            for mirror in &effective_mirrors {
                initialize_mirror_vault_with_audit(mirror, &primary_identity, false)?;
            }
            let generation_after = load_catalog(&root)?.generation;
            Ok((
                RecoveryVaultInitReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    created: !existed,
                    mirror_roots: effective_mirrors
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                },
                Some(generation_after),
            ))
        },
    )
}

pub fn recovery_vault_config(requested_root: Option<&Path>) -> anyhow::Result<RecoveryVaultConfig> {
    let root = resolve_vault_root(requested_root)?;
    load_vault_config(&root)
}

pub fn export_recovery_auth_custody(
    requested_root: Option<&Path>,
    output: &Path,
) -> anyhow::Result<RecoveryAuthCustodyReport> {
    let root = resolve_vault_root(requested_root)?;
    let output = absolute_path(output)?;
    anyhow::ensure!(
        !output.starts_with(&root),
        "Recovery custody bundle cannot be stored inside the Vault"
    );
    let _lock = lock_vault(&root)?;
    load_vault_config(&root)?;
    let (identity, checkpoint_sequence) = auth::export_custody_bundle(&root, &output)?;
    Ok(RecoveryAuthCustodyReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        custody_file: output.display().to_string(),
        lineage_id: identity.lineage_id,
        vault_id: identity.vault_id,
        key_id: identity.key_id,
        checkpoint_sequence,
    })
}

pub fn import_recovery_auth_custody(
    requested_root: Option<&Path>,
    input: &Path,
) -> anyhow::Result<RecoveryAuthCustodyReport> {
    let root = resolve_vault_root(requested_root)?;
    let input = absolute_path(input)?;
    anyhow::ensure!(
        !input.starts_with(&root),
        "Recovery custody bundle cannot be read from inside the Vault"
    );
    let _lock = lock_vault(&root)?;
    let (identity, checkpoint_sequence) = auth::import_custody_bundle(&root, &input)?;
    load_vault_config(&root)?;
    Ok(RecoveryAuthCustodyReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        custody_file: input.display().to_string(),
        lineage_id: identity.lineage_id,
        vault_id: identity.vault_id,
        key_id: identity.key_id,
        checkpoint_sequence,
    })
}

pub fn migrate_recovery_auth(
    requested_root: Option<&Path>,
) -> anyhow::Result<RecoveryAuthMigrationReport> {
    let root = resolve_vault_root(requested_root)?;
    secure_create_dir(&root)?;
    secure_create_dir(&root.join("locks"))?;
    let _lock = lock_vault(&root)?;
    let created = !auth::is_authenticated(&root);
    let (config, catalog) = load_and_verify_recovery_documents(&root)?;
    let primary_verification = verify_unsealed_vault(&root, &catalog)?;
    let identity = if created {
        let identity = auth::initialize_primary_identity(&root)?;
        auth::initialize_state(
            &root,
            catalog.generation,
            &checked_json_bytes(&config)?,
            &checked_json_bytes(&catalog)?,
        )?;
        identity
    } else {
        let identity = auth::require_primary(&root)?;
        auth::verify_state(&root, catalog.generation)?;
        identity
    };

    let mut migrated_mirrors = Vec::new();
    let mut verified_repositories = primary_verification.0;
    let mut verified_recovery_points = primary_verification.1;
    let mut verified_objects = primary_verification.2;
    let mut verified_raw_bytes = primary_verification.3;
    let mut verified_audit_events = primary_verification.4;
    for mirror in &config.mirror_roots {
        let _mirror_lock = lock_vault(mirror)?;
        let mirror_created = !auth::is_authenticated(mirror);
        let (mirror_config, mirror_catalog) = load_and_verify_recovery_documents(mirror)?;
        anyhow::ensure!(
            mirror_config.mirror_roots.is_empty(),
            "legacy recovery mirror is configured as a primary Vault"
        );
        anyhow::ensure!(
            mirror_config.retention == config.retention
                && mirror_config.at_rest_policy == config.at_rest_policy,
            "legacy recovery mirror policy differs from the primary Vault"
        );
        validate_migration_mirror_catalog(&catalog, &mirror_catalog)?;
        let verification = verify_unsealed_vault(mirror, &mirror_catalog)?;
        if mirror_created {
            auth::initialize_mirror_identity(mirror, &identity, false)?;
            auth::initialize_state(
                mirror,
                mirror_catalog.generation,
                &checked_json_bytes(&mirror_config)?,
                &checked_json_bytes(&mirror_catalog)?,
            )?;
            migrated_mirrors.push(mirror.display().to_string());
        } else {
            auth::require_mirror_for(mirror, &identity)?;
            auth::verify_state(mirror, mirror_catalog.generation)?;
        }
        verified_repositories = verified_repositories.saturating_add(verification.0);
        verified_recovery_points = verified_recovery_points.saturating_add(verification.1);
        verified_objects = verified_objects.saturating_add(verification.2);
        verified_raw_bytes = verified_raw_bytes.saturating_add(verification.3);
        verified_audit_events = verified_audit_events.saturating_add(verification.4);
    }

    Ok(RecoveryAuthMigrationReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        created,
        lineage_id: identity.lineage_id,
        vault_id: identity.vault_id,
        key_id: identity.key_id,
        migrated_mirrors,
        verified_repositories,
        verified_recovery_points,
        verified_objects,
        verified_raw_bytes,
        verified_audit_events,
    })
}

pub fn rotate_recovery_auth_key(
    requested_root: Option<&Path>,
) -> anyhow::Result<RecoveryAuthRotationReport> {
    let root = resolve_vault_root(requested_root)?;
    let _lock = lock_vault(&root)?;
    let primary_identity = auth::require_primary(&root)?;
    let config = load_vault_config(&root)?;
    let catalog = load_catalog(&root)?;
    verify_unsealed_vault(&root, &catalog)?;

    let mut previous_key_ids = BTreeSet::from([primary_identity.key_id.clone()]);
    let mut mirror_states = Vec::with_capacity(config.mirror_roots.len());
    for mirror in &config.mirror_roots {
        let mirror_lock = lock_vault(mirror)?;
        let mirror_config = load_vault_config(mirror)?;
        anyhow::ensure!(
            mirror_config.mirror_roots.is_empty(),
            "Recovery auth rotation target is not a mirror Vault"
        );
        let mirror_catalog = load_catalog(mirror)?;
        auth::require_mirror_for(mirror, &primary_identity)?;
        verify_unsealed_vault(mirror, &mirror_catalog)?;
        let mirror_identity = auth::load_identity(mirror)?;
        previous_key_ids.insert(mirror_identity.key_id);
        mirror_states.push((mirror.clone(), mirror_lock, mirror_catalog.generation));
    }

    let generation = catalog.generation;
    run_audited(
        &root,
        RecoveryAuditOperation::AuthKeyRotate,
        Some(generation),
        None,
        None,
        BTreeMap::from([(
            "vault_count".to_string(),
            (mirror_states.len() + 1).to_string(),
        )]),
        || {
            let key_id = auth::create_rotation_key(&root)?;
            let mut rotated_vaults = Vec::with_capacity(mirror_states.len() + 1);
            for (mirror, _lock, mirror_generation) in &mirror_states {
                auth::rotate_identity_key(mirror, &key_id, *mirror_generation)?;
                rotated_vaults.push(mirror.display().to_string());
            }
            recovery_failpoint("auth_rotation_after_mirrors")?;
            auth::rotate_identity_key(&root, &key_id, generation)?;
            rotated_vaults.push(root.display().to_string());
            recovery_failpoint("auth_rotation_after_primary")?;

            let rotated_primary = auth::require_primary(&root)?;
            anyhow::ensure!(
                rotated_primary.key_id == key_id,
                "Recovery primary did not adopt the rotated key"
            );
            for (mirror, _lock, mirror_generation) in &mirror_states {
                let rotated_mirror = auth::require_mirror_for(mirror, &rotated_primary)?;
                anyhow::ensure!(
                    rotated_mirror.key_id == key_id,
                    "Recovery mirror did not adopt the rotated key"
                );
                auth::verify_state(mirror, *mirror_generation)?;
            }
            auth::verify_state(&root, generation)?;
            Ok((
                RecoveryAuthRotationReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    lineage_id: rotated_primary.lineage_id,
                    previous_key_ids: previous_key_ids.iter().cloned().collect(),
                    key_id,
                    rotated_vaults,
                    old_keys_retained: true,
                },
                Some(generation),
            ))
        },
    )
}

pub fn update_recovery_retention(
    requested_root: Option<&Path>,
    retention: RecoveryRetentionPolicy,
) -> anyhow::Result<RecoveryVaultConfig> {
    retention.validate()?;
    let root = resolve_vault_root(requested_root)?;
    let _lock = lock_vault(&root)?;
    auth::require_primary(&root)?;
    let mut config = load_vault_config(&root)?;
    let generation = load_catalog(&root)?.generation;
    let details = BTreeMap::from([
        (
            "minimum_points".to_string(),
            retention.minimum_points_per_repository.to_string(),
        ),
        (
            "minimum_days".to_string(),
            retention.minimum_retention_days.to_string(),
        ),
        (
            "maximum_points".to_string(),
            retention
                .maximum_points_per_repository
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ),
        (
            "maximum_bytes".to_string(),
            retention
                .maximum_vault_bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ),
    ]);
    run_audited(
        &root,
        RecoveryAuditOperation::RetentionUpdate,
        Some(generation),
        None,
        None,
        details,
        || {
            for mirror in &config.mirror_roots {
                update_mirror_retention(&root, mirror, &retention)?;
            }
            config.retention = retention;
            save_vault_config(&root, &config)?;
            Ok((config, Some(generation)))
        },
    )
}

pub fn register_recovery_repository(
    source: &Path,
    requested_root: Option<&Path>,
) -> anyhow::Result<RecoveryRegistrationReport> {
    let (source_root, config) = repository_root_and_config(source)?;
    let root = resolve_vault_root(requested_root)?;
    ensure_separate_roots(&source_root, &root)?;
    ensure_vault_root(&root)?;
    let _lock = lock_vault(&root)?;
    let mut catalog = load_catalog(&root)?;
    let key = hex::encode(config.repository_id);
    let source_label = source_root.display().to_string();
    let generation_before = catalog.generation;
    let details = BTreeMap::from([("source_root".to_string(), source_label.clone())]);
    run_audited(
        &root,
        RecoveryAuditOperation::RepositoryRegister,
        Some(generation_before),
        Some(config.repository_id),
        None,
        details,
        || {
            let now = now_unix_ns();
            let created = !catalog.repositories.contains_key(&key);
            let registration =
                catalog
                    .repositories
                    .entry(key)
                    .or_insert_with(|| RecoveryRegistration {
                        schema: VAULT_SCHEMA,
                        registration_id: crate::random_bytes(),
                        repository_id: config.repository_id,
                        created_unix_ns: now,
                        updated_unix_ns: now,
                        source_paths: Vec::new(),
                        recovery_points: BTreeMap::new(),
                        tombstones: Vec::new(),
                    });
            anyhow::ensure!(
                registration.repository_id == config.repository_id,
                "recovery registration identity mismatch"
            );
            if !registration.source_paths.contains(&source_label) {
                registration.source_paths.push(source_label.clone());
                registration.source_paths.sort();
            }
            registration.updated_unix_ns = now;
            let registration_id = registration.registration_id;
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(&root, &catalog)?;
            Ok((
                RecoveryRegistrationReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    repository_id: config.repository_id,
                    registration_id,
                    source_root: source_label,
                    created,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn capture_recovery_point(
    source: &Path,
    revision: &str,
    requested_root: Option<&Path>,
) -> anyhow::Result<RecoveryCaptureReport> {
    let (source_root, source_config) = repository_root_and_config(source)?;
    let root = resolve_vault_root(requested_root)?;
    ensure_separate_roots(&source_root, &root)?;
    ensure_vault_root(&root)?;
    let commit_id = repository_revision_id(&source_root, revision)?;
    let point_id = commit_id.to_hex();
    let _lock = lock_vault(&root)?;
    let config = load_vault_config(&root)?;
    let mut catalog = load_catalog(&root)?;
    let generation_before = catalog.generation;
    let details = BTreeMap::from([
        ("revision".to_string(), revision.to_string()),
        ("source_root".to_string(), source_root.display().to_string()),
    ]);
    run_audited(
        &root,
        RecoveryAuditOperation::Capture,
        Some(generation_before),
        Some(source_config.repository_id),
        Some(point_id.clone()),
        details,
        || {
            recovery_failpoint("capture_after_prepared")?;
            let key = hex::encode(source_config.repository_id);
            let now = now_unix_ns();
            let source_label = source_root.display().to_string();
            let registration =
                catalog
                    .repositories
                    .entry(key.clone())
                    .or_insert_with(|| RecoveryRegistration {
                        schema: VAULT_SCHEMA,
                        registration_id: crate::random_bytes(),
                        repository_id: source_config.repository_id,
                        created_unix_ns: now,
                        updated_unix_ns: now,
                        source_paths: vec![source_label.clone()],
                        recovery_points: BTreeMap::new(),
                        tombstones: Vec::new(),
                    });
            if !registration.source_paths.contains(&source_label) {
                registration.source_paths.push(source_label);
                registration.source_paths.sort();
            }
            let created = !registration.recovery_points.contains_key(&point_id);
            let primary = replicate_repository_revision(
                &source_root,
                revision,
                &vault_repository_root(&root, source_config.repository_id)?,
                &point_id,
            )?;
            anyhow::ensure!(
                primary.commit_id == commit_id,
                "source revision changed during capture"
            );
            recovery_failpoint("capture_after_primary_replication")?;

            let mut replicas = Vec::new();
            for mirror in &config.mirror_roots {
                let result = capture_mirror(
                    &root,
                    mirror,
                    &source_root,
                    revision,
                    &point_id,
                    registration,
                    &primary,
                );
                match result {
                    Ok(()) => replicas.push(RecoveryReplicaStatus {
                        vault_root: mirror.display().to_string(),
                        verified: true,
                        error: None,
                    }),
                    Err(error) => replicas.push(RecoveryReplicaStatus {
                        vault_root: mirror.display().to_string(),
                        verified: false,
                        error: Some(bounded_recovery_text(&error.to_string())),
                    }),
                }
            }
            let durability = if config.mirror_roots.is_empty() {
                RecoveryDurability::Captured
            } else if replicas.iter().all(|replica| replica.verified) {
                RecoveryDurability::Protected
            } else {
                RecoveryDurability::Degraded
            };
            let captured_unix_ns = registration
                .recovery_points
                .get(&point_id)
                .map(|point| point.captured_unix_ns)
                .unwrap_or(now);
            let pinned = registration
                .recovery_points
                .get(&point_id)
                .is_some_and(|point| point.pinned);
            let point = RecoveryPoint {
                schema: VAULT_SCHEMA,
                recovery_point_id: point_id.clone(),
                commit_id,
                ref_name: primary.ref_name,
                captured_unix_ns,
                last_verified_unix_ns: now,
                reachable_objects: primary.reachable_objects,
                stored_objects_written: primary.objects_written,
                stored_bytes_written: primary.object_bytes_written,
                durability,
                replicas,
                pinned,
                state: RecoveryPointState::Available,
            };
            registration.recovery_points.insert(point_id, point.clone());
            registration.updated_unix_ns = now;
            catalog.generation = catalog.generation.saturating_add(1);
            recovery_failpoint("capture_before_catalog_publish")?;
            save_catalog(&root, &catalog)?;
            recovery_failpoint("capture_after_catalog_publish")?;
            Ok((
                RecoveryCaptureReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    repository_id: source_config.repository_id,
                    recovery_point: point,
                    created,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn list_recovery_vault(
    requested_root: Option<&Path>,
) -> anyhow::Result<RecoveryVaultListReport> {
    let root = resolve_vault_root(requested_root)?;
    let catalog = load_catalog(&root)?;
    Ok(RecoveryVaultListReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        generation: catalog.generation,
        repositories: catalog.repositories.into_values().collect(),
    })
}

pub fn recovery_vault_status(
    requested_root: Option<&Path>,
) -> anyhow::Result<RecoveryVaultStatusReport> {
    let root = resolve_vault_root(requested_root)?;
    let config = load_vault_config(&root)?;
    let catalog = load_catalog(&root)?;
    let audit = recovery_audit_log(Some(&root))?;
    let mut recovery_points = 0_u64;
    let mut available_points = 0_u64;
    let mut pending_deletion_points = 0_u64;
    let mut captured_points = 0_u64;
    let mut protected_points = 0_u64;
    let mut degraded_points = 0_u64;
    let mut latest_capture_unix_ns = None::<i128>;
    for point in catalog
        .repositories
        .values()
        .flat_map(|registration| registration.recovery_points.values())
    {
        recovery_points = recovery_points.saturating_add(1);
        match point.state {
            RecoveryPointState::Available => available_points = available_points.saturating_add(1),
            RecoveryPointState::PendingDeletion => {
                pending_deletion_points = pending_deletion_points.saturating_add(1)
            }
        }
        match point.durability {
            RecoveryDurability::Captured => captured_points = captured_points.saturating_add(1),
            RecoveryDurability::Protected => protected_points = protected_points.saturating_add(1),
            RecoveryDurability::Degraded => degraded_points = degraded_points.saturating_add(1),
        }
        latest_capture_unix_ns = Some(
            latest_capture_unix_ns.map_or(point.captured_unix_ns, |latest| {
                latest.max(point.captured_unix_ns)
            }),
        );
    }
    let rpo_lag_millis = latest_capture_unix_ns.map(|captured| {
        let elapsed = now_unix_ns().saturating_sub(captured).max(0) / 1_000_000;
        u64::try_from(elapsed).unwrap_or(u64::MAX)
    });
    Ok(RecoveryVaultStatusReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        generation: catalog.generation,
        created_unix_ns: config.created_unix_ns,
        at_rest_policy: config.at_rest_policy,
        configured_mirrors: config.mirror_roots.len().try_into()?,
        repositories: catalog.repositories.len().try_into()?,
        recovery_points,
        available_points,
        pending_deletion_points,
        captured_points,
        protected_points,
        degraded_points,
        durability_lag_points: captured_points.saturating_add(degraded_points),
        latest_capture_unix_ns,
        rpo_lag_millis,
        incomplete_audit_operations: audit.incomplete_operation_ids.len().try_into()?,
    })
}

pub fn verify_recovery_point(
    requested_root: Option<&Path>,
    repository_id: &str,
    recovery_point_id: &str,
) -> anyhow::Result<RecoveryVerifyReport> {
    let root = resolve_vault_root(requested_root)?;
    let id = parse_repository_id(repository_id)?;
    let catalog = load_catalog(&root)?;
    let registration = find_registration(&catalog, id)?;
    let point = registration
        .recovery_points
        .get(recovery_point_id)
        .ok_or_else(|| anyhow::anyhow!("recovery point not found"))?;
    anyhow::ensure!(
        point.commit_id.to_hex() == recovery_point_id,
        "recovery point identity mismatch"
    );
    let repository_root = vault_repository_root(&root, id)?;
    let expected_tag = point
        .ref_name
        .strip_prefix("tags/")
        .ok_or_else(|| anyhow::anyhow!("invalid recovery point ref namespace"))?;
    let refs = repository_refs(&repository_root)?;
    anyhow::ensure!(
        refs.refs.iter().any(|reference| {
            reference.kind == RepositoryRefKind::Tag
                && reference.name == expected_tag
                && reference.commit_id == point.commit_id
        }),
        "published recovery ref is missing or does not match the catalog"
    );
    let repository = verify_repository(&repository_root)?;
    Ok(RecoveryVerifyReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        repository_id: id,
        recovery_point_id: recovery_point_id.to_string(),
        repository,
    })
}

pub fn restore_recovery_point(
    requested_root: Option<&Path>,
    repository_id: &str,
    recovery_point_id: &str,
    output_dir: &Path,
    selected_path: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<RecoveryRestoreReport> {
    let root = resolve_vault_root(requested_root)?;
    let output_root = absolute_path(output_dir)?;
    ensure_disjoint_roots(
        &root,
        &output_root,
        "recovery Vault and restore output cannot contain each other",
    )?;
    let id = parse_repository_id(repository_id)?;
    let _lock = lock_vault(&root)?;
    let catalog = load_catalog(&root)?;
    let registration = find_registration(&catalog, id)?;
    let point = registration
        .recovery_points
        .get(recovery_point_id)
        .ok_or_else(|| anyhow::anyhow!("recovery point not found"))?;
    anyhow::ensure!(
        point.state == RecoveryPointState::Available,
        "recovery point is pending deletion"
    );
    let details = BTreeMap::from([
        (
            "output_dir".to_string(),
            absolute_path(output_dir)?.display().to_string(),
        ),
        (
            "selected_path".to_string(),
            selected_path.unwrap_or(".").to_string(),
        ),
        ("overwrite".to_string(), overwrite.to_string()),
    ]);
    run_audited(
        &root,
        RecoveryAuditOperation::Restore,
        Some(catalog.generation),
        Some(id),
        Some(recovery_point_id.to_string()),
        details,
        || {
            recovery_failpoint("restore_after_prepared")?;
            verify_recovery_point(Some(&root), repository_id, recovery_point_id)?;
            recovery_failpoint("restore_after_verification")?;
            let restore = restore_repository(
                &vault_repository_root(&root, id)?,
                &point.ref_name,
                output_dir,
                selected_path,
                overwrite,
            )?;
            anyhow::ensure!(
                restore.commit_id == point.commit_id,
                "restored commit mismatch"
            );
            recovery_failpoint("restore_after_publication")?;
            Ok((
                RecoveryRestoreReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    repository_id: id,
                    recovery_point_id: recovery_point_id.to_string(),
                    restore,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn set_recovery_point_pin(
    requested_root: Option<&Path>,
    repository_id: &str,
    recovery_point_id: &str,
    pinned: bool,
) -> anyhow::Result<RecoveryPinReport> {
    let root = resolve_vault_root(requested_root)?;
    let id = parse_repository_id(repository_id)?;
    let _lock = lock_vault(&root)?;
    auth::require_primary(&root)?;
    let config = load_vault_config(&root)?;
    let mut catalog = load_catalog(&root)?;
    let generation_before = catalog.generation;
    let details = BTreeMap::from([("pinned".to_string(), pinned.to_string())]);
    run_audited(
        &root,
        RecoveryAuditOperation::PinUpdate,
        Some(generation_before),
        Some(id),
        Some(recovery_point_id.to_string()),
        details,
        || {
            let registration = catalog
                .repositories
                .get_mut(&hex::encode(id))
                .ok_or_else(|| anyhow::anyhow!("recovery repository not found"))?;
            let point = registration
                .recovery_points
                .get_mut(recovery_point_id)
                .ok_or_else(|| anyhow::anyhow!("recovery point not found"))?;
            anyhow::ensure!(
                point.state == RecoveryPointState::Available,
                "recovery point is pending deletion"
            );
            let changed = point.pinned != pinned;
            point.pinned = pinned;
            registration.updated_unix_ns = now_unix_ns();
            let mirrored = registration.clone();
            for mirror in &config.mirror_roots {
                update_mirror_registration(&root, mirror, &mirrored)?;
            }
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(&root, &catalog)?;
            Ok((
                RecoveryPinReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    repository_id: id,
                    recovery_point_id: recovery_point_id.to_string(),
                    pinned,
                    changed,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn record_recovery_tombstone(
    requested_root: Option<&Path>,
    repository_id: &str,
    kind: RecoveryTombstoneKind,
    source_path: Option<String>,
    relative_path: Option<String>,
    reason: String,
) -> anyhow::Result<RecoveryTombstoneReport> {
    anyhow::ensure!(
        !reason.trim().is_empty(),
        "tombstone reason must not be empty"
    );
    validate_recovery_text("tombstone reason", &reason, MAX_RECOVERY_TEXT_BYTES)?;
    match kind {
        RecoveryTombstoneKind::File => anyhow::ensure!(
            relative_path.as_ref().is_some_and(|path| !path.is_empty()),
            "file tombstones require a relative path"
        ),
        RecoveryTombstoneKind::Workspace | RecoveryTombstoneKind::Registration => {
            anyhow::ensure!(
                relative_path.is_none(),
                "non-file tombstones cannot name a relative path"
            )
        }
    }
    if let Some(path) = relative_path.as_deref() {
        validate_relative_label(path)?;
    }
    if let Some(path) = source_path.as_deref() {
        validate_recovery_text("tombstone source path", path, MAX_RECOVERY_PATH_BYTES)?;
    }
    let source_path = source_path
        .map(|path| {
            absolute_path(Path::new(&path)).map(|normalized| normalized.display().to_string())
        })
        .transpose()?;
    let root = resolve_vault_root(requested_root)?;
    let id = parse_repository_id(repository_id)?;
    let _lock = lock_vault(&root)?;
    auth::require_primary(&root)?;
    let config = load_vault_config(&root)?;
    let mut catalog = load_catalog(&root)?;
    let generation_before = catalog.generation;
    let mut details = BTreeMap::from([(
        "kind".to_string(),
        match kind {
            RecoveryTombstoneKind::File => "file",
            RecoveryTombstoneKind::Workspace => "workspace",
            RecoveryTombstoneKind::Registration => "registration",
        }
        .to_string(),
    )]);
    if let Some(path) = source_path.as_ref() {
        details.insert("source_path".to_string(), path.clone());
    }
    if let Some(path) = relative_path.as_ref() {
        details.insert("relative_path".to_string(), path.clone());
    }
    run_audited(
        &root,
        RecoveryAuditOperation::TombstoneRecord,
        Some(generation_before),
        Some(id),
        None,
        details,
        || {
            let registration = catalog
                .repositories
                .get_mut(&hex::encode(id))
                .ok_or_else(|| anyhow::anyhow!("recovery repository not found"))?;
            anyhow::ensure!(
                registration.tombstones.len() < MAX_RECOVERY_TOMBSTONES_PER_REPOSITORY,
                "recovery tombstone capacity is exhausted"
            );
            if let Some(source_path) = source_path.as_deref() {
                anyhow::ensure!(
                    registration
                        .source_paths
                        .iter()
                        .any(|path| path == source_path),
                    "tombstone source path is not registered"
                );
            }
            let tombstone = RecoveryTombstone {
                schema: VAULT_SCHEMA,
                tombstone_id: crate::random_bytes(),
                kind,
                observed_unix_ns: now_unix_ns(),
                source_path,
                relative_path,
                reason,
            };
            registration.tombstones.push(tombstone.clone());
            registration.updated_unix_ns = tombstone.observed_unix_ns;
            let mirrored = registration.clone();
            for mirror in &config.mirror_roots {
                update_mirror_registration(&root, mirror, &mirrored)?;
            }
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(&root, &catalog)?;
            Ok((
                RecoveryTombstoneReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    repository_id: id,
                    tombstone,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn gc_recovery_vault(
    requested_root: Option<&Path>,
    dry_run: bool,
) -> anyhow::Result<RecoveryVaultGcReport> {
    let root = resolve_vault_root(requested_root)?;
    let _lock = lock_vault(&root)?;
    auth::require_primary(&root)?;
    let config = load_vault_config(&root)?;
    let mut catalog = load_catalog(&root)?;
    let selection = select_gc_candidates(&root, &catalog, &config.retention)?;
    let total_points = catalog
        .repositories
        .values()
        .map(|registration| registration.recovery_points.len() as u64)
        .sum::<u64>();
    let candidates = selection
        .selected
        .iter()
        .filter_map(|(repository_key, point_ids)| {
            let registration = catalog.repositories.get(repository_key)?;
            Some(point_ids.iter().filter_map(move |point_id| {
                registration
                    .recovery_points
                    .get(point_id)
                    .map(|point| RecoveryGcCandidate {
                        repository_id: registration.repository_id,
                        recovery_point_id: point_id.clone(),
                        captured_unix_ns: point.captured_unix_ns,
                        reason: if point.state == RecoveryPointState::PendingDeletion {
                            "resume_pending_deletion".to_string()
                        } else {
                            "retention_or_quota".to_string()
                        },
                    })
            }))
        })
        .flatten()
        .collect::<Vec<_>>();

    if dry_run || candidates.is_empty() {
        return Ok(RecoveryVaultGcReport {
            schema: RECOVERY_REPORT_SCHEMA,
            vault_root: root.display().to_string(),
            dry_run,
            total_recovery_points: total_points,
            retained_recovery_points: total_points.saturating_sub(candidates.len() as u64),
            candidate_recovery_points: candidates.len() as u64,
            removed_recovery_points: 0,
            stored_bytes_before: selection.stored_bytes_before,
            projected_stored_bytes: selection.projected_stored_bytes,
            policy_satisfied: selection.policy_satisfied,
            candidates,
            repositories: selection.repository_reports,
        });
    }

    let generation_before = catalog.generation;
    let details = BTreeMap::from([("candidate_count".to_string(), candidates.len().to_string())]);
    run_audited(
        &root,
        RecoveryAuditOperation::GarbageCollection,
        Some(generation_before),
        None,
        None,
        details,
        || {
            recovery_failpoint("gc_after_prepared")?;
            for (repository_key, point_ids) in &selection.selected {
                let registration = catalog
                    .repositories
                    .get_mut(repository_key)
                    .ok_or_else(|| anyhow::anyhow!("recovery repository disappeared during GC"))?;
                for point_id in point_ids {
                    let point = registration
                        .recovery_points
                        .get_mut(point_id)
                        .ok_or_else(|| anyhow::anyhow!("recovery point disappeared during GC"))?;
                    anyhow::ensure!(!point.pinned, "pinned recovery point selected for GC");
                    point.state = RecoveryPointState::PendingDeletion;
                }
                registration.updated_unix_ns = now_unix_ns();
            }
            let pending_registrations = selection
                .selected
                .keys()
                .filter_map(|key| catalog.repositories.get(key).cloned())
                .collect::<Vec<_>>();
            for mirror in &config.mirror_roots {
                for registration in &pending_registrations {
                    update_mirror_registration(&root, mirror, registration)?;
                }
            }
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(&root, &catalog)?;
            recovery_failpoint("gc_after_pending_catalog")?;

            for mirror in &config.mirror_roots {
                for (repository_key, point_ids) in &selection.selected {
                    let registration = catalog
                        .repositories
                        .get(repository_key)
                        .ok_or_else(|| anyhow::anyhow!("recovery repository missing"))?;
                    reconcile_retained_mirror_points(&root, mirror, registration, point_ids)?;
                    gc_repository_excluding_recovery_refs(
                        &vault_repository_root(mirror, registration.repository_id)?,
                        point_ids,
                        false,
                    )?;
                    verify_retained_recovery_points(mirror, registration, point_ids)?;
                }
            }
            recovery_failpoint("gc_after_mirror_deletion")?;
            let mut applied_reports = BTreeMap::new();
            for (repository_key, point_ids) in &selection.selected {
                let registration = catalog
                    .repositories
                    .get(repository_key)
                    .ok_or_else(|| anyhow::anyhow!("recovery repository missing"))?;
                let report = gc_repository_excluding_recovery_refs(
                    &vault_repository_root(&root, registration.repository_id)?,
                    point_ids,
                    false,
                )?;
                verify_retained_recovery_points(&root, registration, point_ids)?;
                applied_reports.insert(repository_key.clone(), report);
            }
            recovery_failpoint("gc_after_primary_deletion")?;

            for (repository_key, point_ids) in &selection.selected {
                let registration = catalog
                    .repositories
                    .get_mut(repository_key)
                    .ok_or_else(|| anyhow::anyhow!("recovery repository missing"))?;
                for point_id in point_ids {
                    registration.recovery_points.remove(point_id);
                }
                registration.updated_unix_ns = now_unix_ns();
            }
            let final_registrations = selection
                .selected
                .keys()
                .filter_map(|key| catalog.repositories.get(key).cloned())
                .collect::<Vec<_>>();
            for mirror in &config.mirror_roots {
                for registration in &final_registrations {
                    update_mirror_registration(&root, mirror, registration)?;
                    verify_retained_recovery_points(mirror, registration, &BTreeSet::new())?;
                }
            }
            catalog.generation = catalog.generation.saturating_add(1);
            recovery_failpoint("gc_before_final_catalog")?;
            save_catalog(&root, &catalog)?;
            recovery_failpoint("gc_after_final_catalog")?;
            let projected_stored_bytes = applied_reports
                .values()
                .map(|report| report.total_bytes.saturating_sub(report.removed_bytes))
                .sum();
            Ok((
                RecoveryVaultGcReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    dry_run: false,
                    total_recovery_points: total_points,
                    retained_recovery_points: total_points.saturating_sub(candidates.len() as u64),
                    candidate_recovery_points: candidates.len() as u64,
                    removed_recovery_points: candidates.len() as u64,
                    stored_bytes_before: selection.stored_bytes_before,
                    projected_stored_bytes,
                    policy_satisfied: selection.policy_satisfied,
                    candidates,
                    repositories: applied_reports,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn scrub_recovery_vault(requested_root: Option<&Path>) -> anyhow::Result<RecoveryScrubReport> {
    let root = resolve_vault_root(requested_root)?;
    let config = load_vault_config(&root)?;
    let mut locations = vec![scrub_vault_location(&root, true)];
    locations.extend(
        config
            .mirror_roots
            .iter()
            .map(|mirror| scrub_vault_location(mirror, false)),
    );
    Ok(RecoveryScrubReport {
        schema: RECOVERY_REPORT_SCHEMA,
        healthy: locations.iter().all(|location| location.healthy),
        locations,
    })
}

pub fn repair_recovery_point(
    requested_root: Option<&Path>,
    repository_id: &str,
    recovery_point_id: &str,
    requested_mirror: Option<&Path>,
) -> anyhow::Result<RecoveryRepairReport> {
    let root = resolve_vault_root(requested_root)?;
    let id = parse_repository_id(repository_id)?;
    let _lock = lock_vault(&root)?;
    auth::require_primary(&root)?;
    let config = load_vault_config(&root)?;
    let mut catalog = load_catalog(&root)?;
    let primary_point = find_registration(&catalog, id)?
        .recovery_points
        .get(recovery_point_id)
        .ok_or_else(|| anyhow::anyhow!("recovery point not found"))?
        .clone();
    anyhow::ensure!(
        primary_point.state == RecoveryPointState::Available,
        "recovery point is pending deletion"
    );
    let mirror = match requested_mirror {
        Some(mirror) => {
            let mirror = absolute_path(mirror)?;
            anyhow::ensure!(
                config.mirror_roots.contains(&mirror),
                "repair mirror is not configured for this vault"
            );
            mirror
        }
        None => config
            .mirror_roots
            .iter()
            .find(|mirror| {
                verify_recovery_point(Some(mirror), repository_id, recovery_point_id).is_ok()
            })
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no verified recovery mirror is available"))?,
    };
    let mirror_catalog = load_catalog(&mirror)?;
    let mirror_point = find_registration(&mirror_catalog, id)?
        .recovery_points
        .get(recovery_point_id)
        .ok_or_else(|| anyhow::anyhow!("recovery point is missing from mirror catalog"))?;
    anyhow::ensure!(
        mirror_point.commit_id == primary_point.commit_id,
        "mirror recovery point commit mismatch"
    );
    let generation_before = catalog.generation;
    let details = BTreeMap::from([("mirror_root".to_string(), mirror.display().to_string())]);
    run_audited(
        &root,
        RecoveryAuditOperation::Repair,
        Some(generation_before),
        Some(id),
        Some(recovery_point_id.to_string()),
        details,
        || {
            verify_recovery_point(Some(&mirror), repository_id, recovery_point_id)?;
            let repaired = repair_repository_revision(
                &vault_repository_root(&mirror, id)?,
                &mirror_point.ref_name,
                &vault_repository_root(&root, id)?,
                recovery_point_id,
            )?;
            verify_recovery_point(Some(&root), repository_id, recovery_point_id)?;
            let registration = catalog
                .repositories
                .get_mut(&hex::encode(id))
                .ok_or_else(|| anyhow::anyhow!("recovery repository not found"))?;
            let point = registration
                .recovery_points
                .get_mut(recovery_point_id)
                .ok_or_else(|| anyhow::anyhow!("recovery point not found"))?;
            point.last_verified_unix_ns = now_unix_ns();
            if let Some(replica) = point
                .replicas
                .iter_mut()
                .find(|replica| replica.vault_root == mirror.display().to_string())
            {
                replica.verified = true;
                replica.error = None;
            }
            point.durability = if point.replicas.iter().all(|replica| replica.verified)
                && !point.replicas.is_empty()
            {
                RecoveryDurability::Protected
            } else if point.replicas.is_empty() {
                RecoveryDurability::Captured
            } else {
                RecoveryDurability::Degraded
            };
            registration.updated_unix_ns = point.last_verified_unix_ns;
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(&root, &catalog)?;
            Ok((
                RecoveryRepairReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    mirror_root: mirror.display().to_string(),
                    repository_id: id,
                    recovery_point_id: recovery_point_id.to_string(),
                    objects_written: repaired.objects_written,
                    objects_repaired: repaired.objects_repaired,
                    object_bytes_written: repaired.object_bytes_written,
                    verified: true,
                },
                Some(catalog.generation),
            ))
        },
    )
}

pub fn promote_recovery_vault(
    requested_root: Option<&Path>,
    requested_mirrors: Vec<PathBuf>,
) -> anyhow::Result<RecoveryPromotionReport> {
    let root = resolve_vault_root(requested_root)?;
    let mirrors = normalize_mirror_roots(&root, requested_mirrors)?;
    let _lock = lock_vault(&root)?;
    let config = load_vault_config(&root)?;
    let mut catalog = load_catalog(&root)?;
    anyhow::ensure!(
        !catalog.repositories.is_empty(),
        "cannot promote an empty Recovery Vault"
    );
    let recovery_points = catalog
        .repositories
        .values()
        .map(|registration| registration.recovery_points.len() as u64)
        .sum::<u64>();
    anyhow::ensure!(
        recovery_points > 0,
        "cannot promote a Recovery Vault without recovery points"
    );
    anyhow::ensure!(
        catalog
            .repositories
            .values()
            .all(|registration| registration
                .recovery_points
                .values()
                .all(|point| point.state == RecoveryPointState::Available)),
        "cannot promote while a recovery point is pending deletion"
    );
    let local_scrub = scrub_vault_location(&root, true);
    anyhow::ensure!(
        local_scrub.healthy,
        "cannot promote an unhealthy Recovery Vault: {}",
        local_scrub.errors.join(" | ")
    );

    let generation_before = catalog.generation;
    let details = BTreeMap::from([
        ("scope".to_string(), "verified_mirror_failover".to_string()),
        ("mirror_count".to_string(), mirrors.len().to_string()),
    ]);
    run_audited(
        &root,
        RecoveryAuditOperation::VaultPromote,
        Some(generation_before),
        None,
        None,
        details,
        || {
            auth::promote_identity(&root, catalog.generation)?;
            let mut objects_written = 0_u64;
            let mut object_bytes_written = 0_u64;
            for mirror in &mirrors {
                let (objects, bytes) =
                    synchronize_promoted_mirror(&root, &config, &catalog, mirror)?;
                objects_written = objects_written.saturating_add(objects);
                object_bytes_written = object_bytes_written.saturating_add(bytes);
            }
            recovery_failpoint("promotion_after_mirror_replication")?;

            let now = now_unix_ns();
            let desired_replicas = mirrors
                .iter()
                .map(|mirror| RecoveryReplicaStatus {
                    vault_root: mirror.display().to_string(),
                    verified: true,
                    error: None,
                })
                .collect::<Vec<_>>();
            let durability = if mirrors.is_empty() {
                RecoveryDurability::Captured
            } else {
                RecoveryDurability::Protected
            };
            let catalog_changed = catalog.repositories.values().any(|registration| {
                registration.recovery_points.values().any(|point| {
                    point.durability != durability || point.replicas != desired_replicas
                })
            });
            let changed = config.mirror_roots != mirrors || catalog_changed;
            if changed {
                let promoted_config = RecoveryVaultConfig {
                    schema: VAULT_SCHEMA,
                    created_unix_ns: config.created_unix_ns,
                    mirror_roots: mirrors.clone(),
                    retention: config.retention.clone(),
                    at_rest_policy: config.at_rest_policy,
                };
                // Publishing mirror policy first is conservative: an interruption
                // cannot make an unreplicated point appear protected.
                save_vault_config(&root, &promoted_config)?;
                recovery_failpoint("promotion_after_config_publish")?;
                for registration in catalog.repositories.values_mut() {
                    for point in registration.recovery_points.values_mut() {
                        point.replicas = desired_replicas.clone();
                        point.durability = durability;
                        point.last_verified_unix_ns = now;
                    }
                    registration.updated_unix_ns = now;
                }
                catalog.generation = catalog.generation.saturating_add(1);
                save_catalog(&root, &catalog)?;
                recovery_failpoint("promotion_after_catalog_publish")?;
            }
            Ok((
                RecoveryPromotionReport {
                    schema: RECOVERY_REPORT_SCHEMA,
                    vault_root: root.display().to_string(),
                    generation_before,
                    generation_after: catalog.generation,
                    changed,
                    repositories: catalog.repositories.len().try_into()?,
                    recovery_points,
                    mirror_roots: mirrors
                        .iter()
                        .map(|mirror| mirror.display().to_string())
                        .collect(),
                    objects_written,
                    object_bytes_written,
                    durability,
                },
                Some(catalog.generation),
            ))
        },
    )
}

fn synchronize_promoted_mirror(
    source_root: &Path,
    source_config: &RecoveryVaultConfig,
    source_catalog: &RecoveryCatalog,
    mirror: &Path,
) -> anyhow::Result<(u64, u64)> {
    let primary_identity = auth::require_primary(source_root)?;
    ensure_mirror_vault_with_audit(mirror, &primary_identity, true)?;
    let _lock = lock_vault(mirror)?;
    let mut mirror_config = load_vault_config(mirror)?;
    anyhow::ensure!(
        mirror_config.mirror_roots.is_empty(),
        "promotion target is already configured as a primary Recovery Vault"
    );
    let mut mirror_catalog = load_catalog(mirror)?;
    anyhow::ensure!(
        mirror_catalog
            .repositories
            .keys()
            .all(|key| source_catalog.repositories.contains_key(key)),
        "promotion target contains an unrelated recovery repository"
    );
    for (key, source_registration) in &source_catalog.repositories {
        if let Some(existing) = mirror_catalog.repositories.get(key) {
            validate_promotion_registration(existing, source_registration)?;
        }
    }
    let generation_before = mirror_catalog.generation;
    let details = BTreeMap::from([
        (
            "scope".to_string(),
            "promoted_vault_replication".to_string(),
        ),
        (
            "source_vault".to_string(),
            source_root.display().to_string(),
        ),
    ]);
    run_audited(
        mirror,
        RecoveryAuditOperation::MirrorSynchronize,
        Some(generation_before),
        None,
        None,
        details,
        || {
            let now = now_unix_ns();
            let mut objects_written = 0_u64;
            let mut object_bytes_written = 0_u64;
            for (key, source_registration) in &source_catalog.repositories {
                for (point_id, point) in &source_registration.recovery_points {
                    let replicated = replicate_repository_revision(
                        &vault_repository_root(source_root, source_registration.repository_id)?,
                        &point.ref_name,
                        &vault_repository_root(mirror, source_registration.repository_id)?,
                        point_id,
                    )?;
                    anyhow::ensure!(
                        replicated.commit_id == point.commit_id
                            && replicated.reachable_objects == point.reachable_objects,
                        "promoted mirror recovery graph mismatch"
                    );
                    objects_written = objects_written.saturating_add(replicated.objects_written);
                    object_bytes_written =
                        object_bytes_written.saturating_add(replicated.object_bytes_written);
                }
                let mut mirrored = source_registration.clone();
                for point in mirrored.recovery_points.values_mut() {
                    point.durability = RecoveryDurability::Captured;
                    point.replicas.clear();
                    point.last_verified_unix_ns = now;
                }
                mirrored.updated_unix_ns = now;
                mirror_catalog.repositories.insert(key.clone(), mirrored);
            }
            mirror_config.retention = source_config.retention.clone();
            mirror_config.at_rest_policy = source_config.at_rest_policy;
            save_vault_config(mirror, &mirror_config)?;
            mirror_catalog.generation = mirror_catalog.generation.saturating_add(1);
            save_catalog(mirror, &mirror_catalog)?;
            Ok((
                (objects_written, object_bytes_written),
                Some(mirror_catalog.generation),
            ))
        },
    )
}

fn validate_promotion_registration(
    existing: &RecoveryRegistration,
    source: &RecoveryRegistration,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        existing.registration_id == source.registration_id
            && existing.repository_id == source.repository_id,
        "promotion target registration identity conflicts with the source Vault"
    );
    anyhow::ensure!(
        existing
            .source_paths
            .iter()
            .all(|path| source.source_paths.contains(path)),
        "promotion target contains divergent source-path history"
    );
    anyhow::ensure!(
        existing
            .recovery_points
            .iter()
            .all(|(point_id, point)| source
                .recovery_points
                .get(point_id)
                .is_some_and(|source_point| source_point.commit_id == point.commit_id)),
        "promotion target contains a divergent recovery point"
    );
    anyhow::ensure!(
        existing
            .tombstones
            .iter()
            .all(|tombstone| source.tombstones.contains(tombstone)),
        "promotion target contains divergent deletion history"
    );
    Ok(())
}

fn scrub_vault_location(root: &Path, primary: bool) -> RecoveryScrubLocationReport {
    let mut report = RecoveryScrubLocationReport {
        schema: RECOVERY_REPORT_SCHEMA,
        vault_root: root.display().to_string(),
        primary,
        healthy: true,
        checked_repositories: 0,
        checked_recovery_points: 0,
        checked_objects: 0,
        checked_raw_bytes: 0,
        checked_audit_events: 0,
        incomplete_audit_operations: 0,
        errors: Vec::new(),
    };
    let result = (|| -> anyhow::Result<()> {
        load_vault_config(root)?;
        let audit = recovery_audit_log(Some(root))?;
        report.checked_audit_events = audit.events.len().try_into()?;
        report.incomplete_audit_operations = audit.incomplete_operation_ids.len().try_into()?;
        let catalog = load_catalog(root)?;
        for registration in catalog.repositories.values() {
            report.checked_repositories += 1;
            let repository_root = vault_repository_root(root, registration.repository_id)?;
            let (_, repository_config) = repository_root_and_config(&repository_root)?;
            anyhow::ensure!(
                repository_config.repository_id == registration.repository_id,
                "vault repository identity does not match catalog"
            );
            let refs = repository_refs(&repository_root)?;
            for point in registration.recovery_points.values() {
                if point.state != RecoveryPointState::Available {
                    continue;
                }
                report.checked_recovery_points += 1;
                let expected_tag = point
                    .ref_name
                    .strip_prefix("tags/")
                    .ok_or_else(|| anyhow::anyhow!("invalid recovery ref namespace"))?;
                anyhow::ensure!(
                    refs.refs.iter().any(|reference| {
                        reference.kind == RepositoryRefKind::Tag
                            && reference.name == expected_tag
                            && reference.commit_id == point.commit_id
                    }),
                    "recovery ref missing or mismatched: {}",
                    point.recovery_point_id
                );
            }
            for reference in refs.refs.iter().filter(|reference| {
                reference.kind == RepositoryRefKind::Tag && reference.name.starts_with("recovery/")
            }) {
                let point_id = reference
                    .name
                    .strip_prefix("recovery/")
                    .ok_or_else(|| anyhow::anyhow!("invalid recovery ref"))?;
                anyhow::ensure!(
                    registration.recovery_points.contains_key(point_id),
                    "orphan recovery ref is not present in catalog: {}",
                    reference.name
                );
            }
            let verified = verify_repository(&repository_root)?;
            report.checked_objects = report
                .checked_objects
                .saturating_add(verified.checked_objects);
            report.checked_raw_bytes = report
                .checked_raw_bytes
                .saturating_add(verified.checked_raw_bytes);
        }
        Ok(())
    })();
    if let Err(error) = result {
        report.healthy = false;
        report.errors.push(error.to_string());
    }
    report
}

fn reconcile_retained_mirror_points(
    primary: &Path,
    mirror: &Path,
    registration: &RecoveryRegistration,
    selected: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let source_repository = vault_repository_root(primary, registration.repository_id)?;
    let mirror_repository = vault_repository_root(mirror, registration.repository_id)?;
    for (point_id, point) in &registration.recovery_points {
        if selected.contains(point_id) {
            continue;
        }
        let replicated = replicate_repository_revision(
            &source_repository,
            &point.ref_name,
            &mirror_repository,
            point_id,
        )?;
        anyhow::ensure!(
            replicated.commit_id == point.commit_id
                && replicated.reachable_objects == point.reachable_objects,
            "retained mirror recovery graph does not match the primary"
        );
    }
    verify_retained_recovery_points(mirror, registration, selected)
}

fn verify_retained_recovery_points(
    vault: &Path,
    registration: &RecoveryRegistration,
    selected: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let repository_id = hex::encode(registration.repository_id);
    for point_id in registration.recovery_points.keys() {
        if !selected.contains(point_id) {
            verify_recovery_point(Some(vault), &repository_id, point_id)?;
        }
    }
    Ok(())
}

struct GcSelection {
    selected: BTreeMap<String, BTreeSet<String>>,
    stored_bytes_before: u64,
    projected_stored_bytes: u64,
    policy_satisfied: bool,
    repository_reports: BTreeMap<String, RepositoryGcReport>,
}

fn select_gc_candidates(
    root: &Path,
    catalog: &RecoveryCatalog,
    policy: &RecoveryRetentionPolicy,
) -> anyhow::Result<GcSelection> {
    policy.validate()?;
    let now = now_unix_ns();
    let minimum_age_ns = i128::from(policy.minimum_retention_days)
        .saturating_mul(24 * 60 * 60)
        .saturating_mul(1_000_000_000);
    let mut selected = BTreeMap::<String, BTreeSet<String>>::new();
    let mut eligible = Vec::<(i128, String, String)>::new();

    for (repository_key, registration) in &catalog.repositories {
        let mut available = registration
            .recovery_points
            .values()
            .filter(|point| point.state == RecoveryPointState::Available)
            .collect::<Vec<_>>();
        available.sort_by(|left, right| {
            right
                .captured_unix_ns
                .cmp(&left.captured_unix_ns)
                .then_with(|| right.recovery_point_id.cmp(&left.recovery_point_id))
        });
        let protected_newest = policy.minimum_points_per_repository as usize;
        for point in registration.recovery_points.values() {
            if point.state == RecoveryPointState::PendingDeletion {
                selected
                    .entry(repository_key.clone())
                    .or_default()
                    .insert(point.recovery_point_id.clone());
            }
        }
        for (index, point) in available.into_iter().enumerate() {
            if point.pinned || index < protected_newest {
                continue;
            }
            let age = now.saturating_sub(point.captured_unix_ns);
            if age < minimum_age_ns {
                continue;
            }
            eligible.push((
                point.captured_unix_ns,
                repository_key.clone(),
                point.recovery_point_id.clone(),
            ));
        }
    }
    eligible.sort();

    if let Some(maximum) = policy.maximum_points_per_repository {
        for (repository_key, registration) in &catalog.repositories {
            let available_count = registration
                .recovery_points
                .values()
                .filter(|point| point.state == RecoveryPointState::Available)
                .count();
            let mut excess = available_count.saturating_sub(maximum as usize);
            for (_, candidate_repository, point_id) in &eligible {
                if excess == 0 {
                    break;
                }
                if candidate_repository == repository_key
                    && selected
                        .entry(repository_key.clone())
                        .or_default()
                        .insert(point_id.clone())
                {
                    excess -= 1;
                }
            }
        }
    }

    let mut reports = preview_gc_selection(root, catalog, &selected)?;
    let stored_bytes_before = reports.values().map(|report| report.total_bytes).sum();
    let mut projected = reports
        .values()
        .map(|report| report.total_bytes.saturating_sub(report.unreachable_bytes))
        .sum();
    if let Some(maximum_bytes) = policy.maximum_vault_bytes {
        for (_, repository_key, point_id) in &eligible {
            if projected <= maximum_bytes {
                break;
            }
            if selected
                .entry(repository_key.clone())
                .or_default()
                .insert(point_id.clone())
            {
                reports = preview_gc_selection(root, catalog, &selected)?;
                projected = reports
                    .values()
                    .map(|report| report.total_bytes.saturating_sub(report.unreachable_bytes))
                    .sum();
            }
        }
    }
    let count_policy_satisfied = policy.maximum_points_per_repository.is_none_or(|maximum| {
        catalog.repositories.iter().all(|(key, registration)| {
            let selected_count = selected.get(key).map_or(0, BTreeSet::len);
            registration
                .recovery_points
                .len()
                .saturating_sub(selected_count)
                <= maximum as usize
        })
    });
    let byte_policy_satisfied = policy
        .maximum_vault_bytes
        .is_none_or(|maximum| projected <= maximum);
    Ok(GcSelection {
        selected,
        stored_bytes_before,
        projected_stored_bytes: projected,
        policy_satisfied: count_policy_satisfied && byte_policy_satisfied,
        repository_reports: reports,
    })
}

fn preview_gc_selection(
    root: &Path,
    catalog: &RecoveryCatalog,
    selected: &BTreeMap<String, BTreeSet<String>>,
) -> anyhow::Result<BTreeMap<String, RepositoryGcReport>> {
    let mut reports = BTreeMap::new();
    for (repository_key, registration) in &catalog.repositories {
        let empty = BTreeSet::new();
        let point_ids = selected.get(repository_key).unwrap_or(&empty);
        reports.insert(
            repository_key.clone(),
            gc_repository_excluding_recovery_refs(
                &vault_repository_root(root, registration.repository_id)?,
                point_ids,
                true,
            )?,
        );
    }
    Ok(reports)
}

fn capture_mirror(
    primary_vault: &Path,
    mirror: &Path,
    source_root: &Path,
    revision: &str,
    point_id: &str,
    registration: &RecoveryRegistration,
    primary: &RepositoryReplicationReport,
) -> anyhow::Result<()> {
    ensure_separate_roots(source_root, mirror)?;
    let primary_identity = auth::require_primary(primary_vault)?;
    ensure_mirror_vault_with_audit(mirror, &primary_identity, false)?;
    let _lock = lock_vault(mirror)?;
    let mirror_config = load_vault_config(mirror)?;
    anyhow::ensure!(
        mirror_config.mirror_roots.is_empty(),
        "recovery mirror target is configured as a primary Recovery Vault"
    );
    let mut catalog = load_catalog(mirror)?;
    let key = hex::encode(registration.repository_id);
    if let Some(existing) = catalog.repositories.get(&key) {
        validate_promotion_registration(existing, registration)?;
    }
    let generation_before = catalog.generation;
    let details = BTreeMap::from([("source_root".to_string(), source_root.display().to_string())]);
    run_audited(
        mirror,
        RecoveryAuditOperation::MirrorSynchronize,
        Some(generation_before),
        Some(registration.repository_id),
        Some(point_id.to_string()),
        details,
        || {
            let report = replicate_repository_revision(
                source_root,
                revision,
                &vault_repository_root(mirror, registration.repository_id)?,
                point_id,
            )?;
            anyhow::ensure!(
                report.commit_id == primary.commit_id,
                "mirror commit mismatch"
            );
            anyhow::ensure!(
                report.reachable_objects == primary.reachable_objects,
                "mirror reachable graph mismatch"
            );
            recovery_failpoint("capture_mirror_after_replication")?;
            let now = now_unix_ns();
            let mut mirrored_registration = registration.clone();
            let captured_unix_ns = mirrored_registration
                .recovery_points
                .get(point_id)
                .map(|point| point.captured_unix_ns)
                .unwrap_or(now);
            let pinned = mirrored_registration
                .recovery_points
                .get(point_id)
                .is_some_and(|point| point.pinned);
            mirrored_registration.recovery_points.insert(
                point_id.to_string(),
                RecoveryPoint {
                    schema: VAULT_SCHEMA,
                    recovery_point_id: point_id.to_string(),
                    commit_id: report.commit_id,
                    ref_name: report.ref_name,
                    captured_unix_ns,
                    last_verified_unix_ns: now,
                    reachable_objects: report.reachable_objects,
                    stored_objects_written: report.objects_written,
                    stored_bytes_written: report.object_bytes_written,
                    durability: RecoveryDurability::Captured,
                    replicas: Vec::new(),
                    pinned,
                    state: RecoveryPointState::Available,
                },
            );
            mirrored_registration.updated_unix_ns = now;
            catalog.repositories.insert(key, mirrored_registration);
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(mirror, &catalog)?;
            Ok(((), Some(catalog.generation)))
        },
    )
}

fn initialize_vault_root(
    root: &Path,
    mirror_roots: Vec<PathBuf>,
) -> anyhow::Result<auth::VaultIdentity> {
    secure_create_dir(root)?;
    secure_create_dir(&root.join("locks"))?;
    secure_create_dir(&root.join("repositories"))?;
    secure_create_dir(&root.join("events"))?;
    auth::recover_state(root)?;
    let config_path = vault_config_path(root);
    let catalog_exists = catalog_path(root).exists();
    anyhow::ensure!(
        config_path.exists() == catalog_exists,
        "Recovery Vault config/catalog initialization is incomplete"
    );
    let identity = if config_path.exists() {
        auth::require_primary(root)?
    } else {
        auth::initialize_primary_identity(root)?
    };
    if config_path.exists() {
        enforce_private_file(&config_path)?;
        let existing = load_vault_config(root)?;
        anyhow::ensure!(
            existing.schema == VAULT_SCHEMA,
            "unsupported recovery vault schema"
        );
        if existing.mirror_roots != mirror_roots && !mirror_roots.is_empty() {
            let updated = RecoveryVaultConfig {
                schema: VAULT_SCHEMA,
                created_unix_ns: existing.created_unix_ns,
                mirror_roots,
                retention: existing.retention,
                at_rest_policy: existing.at_rest_policy,
            };
            save_vault_config(root, &updated)?;
        }
    } else {
        let catalog = RecoveryCatalog::default();
        let config = RecoveryVaultConfig {
            schema: VAULT_SCHEMA,
            created_unix_ns: now_unix_ns(),
            mirror_roots,
            retention: RecoveryRetentionPolicy::default(),
            at_rest_policy: RecoveryAtRestPolicy::default(),
        };
        let config_bytes = checked_json_bytes(&config)?;
        let catalog_bytes = checked_json_bytes(&catalog)?;
        auth::initialize_state(root, catalog.generation, &config_bytes, &catalog_bytes)?;
    }
    if catalog_exists {
        enforce_private_file(&catalog_path(root))?;
    } else {
        enforce_private_file(&config_path)?;
        enforce_private_file(&catalog_path(root))?;
    }
    Ok(identity)
}

fn ensure_vault_root(root: &Path) -> anyhow::Result<()> {
    if !vault_config_path(root).exists() {
        initialize_vault_root(root, Vec::new())?;
    } else {
        auth::require_primary(root)?;
        load_vault_config(root)?;
    }
    Ok(())
}

fn resolve_vault_root(requested_root: Option<&Path>) -> anyhow::Result<PathBuf> {
    absolute_path(match requested_root {
        Some(root) => root,
        None => return default_recovery_vault_root(),
    })
}

fn normalize_mirror_roots(root: &Path, mirrors: Vec<PathBuf>) -> anyhow::Result<Vec<PathBuf>> {
    let mut normalized = mirrors
        .into_iter()
        .map(|mirror| absolute_path(&mirror))
        .collect::<anyhow::Result<Vec<_>>>()?;
    normalized.sort();
    normalized.dedup();
    for mirror in &normalized {
        anyhow::ensure!(
            mirror != root,
            "recovery mirror cannot equal the primary vault"
        );
        anyhow::ensure!(
            !mirror.starts_with(root) && !root.starts_with(mirror),
            "recovery primary and mirror roots cannot contain each other"
        );
    }
    validate_pairwise_disjoint_mirrors(&normalized)?;
    Ok(normalized)
}

fn ensure_separate_roots(source: &Path, vault: &Path) -> anyhow::Result<()> {
    ensure_disjoint_roots(
        source,
        vault,
        "recovery vault and source repository cannot contain each other",
    )
}

fn ensure_disjoint_roots(left: &Path, right: &Path, message: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !right.starts_with(left) && !left.starts_with(right),
        "{message}"
    );
    Ok(())
}

fn validate_pairwise_disjoint_mirrors(mirrors: &[PathBuf]) -> anyhow::Result<()> {
    for (index, mirror) in mirrors.iter().enumerate() {
        for other in mirrors.iter().skip(index + 1) {
            anyhow::ensure!(
                !mirror.starts_with(other) && !other.starts_with(mirror),
                "recovery mirror roots cannot contain each other"
            );
        }
    }
    Ok(())
}

fn load_vault_config(root: &Path) -> anyhow::Result<RecoveryVaultConfig> {
    auth::recover_state(root)?;
    let path = vault_config_path(root);
    enforce_private_file(&path)?;
    let config: RecoveryVaultConfig = read_checked_json(&path)?;
    anyhow::ensure!(
        config.schema == VAULT_SCHEMA,
        "unsupported recovery vault schema"
    );
    config.retention.validate()?;
    validate_mirror_roots(root, &config.mirror_roots)?;
    let catalog: RecoveryCatalog = read_checked_json(&catalog_path(root))?;
    validate_catalog(&catalog)?;
    auth::verify_state(root, catalog.generation)?;
    Ok(config)
}

fn load_and_verify_recovery_documents(
    root: &Path,
) -> anyhow::Result<(RecoveryVaultConfig, RecoveryCatalog)> {
    let config_path = vault_config_path(root);
    let catalog_path = catalog_path(root);
    enforce_private_file(&config_path)?;
    enforce_private_file(&catalog_path)?;
    let config: RecoveryVaultConfig = read_checked_json(&config_path)?;
    anyhow::ensure!(
        config.schema == VAULT_SCHEMA,
        "unsupported recovery vault schema"
    );
    config.retention.validate()?;
    validate_mirror_roots(root, &config.mirror_roots)?;
    let catalog: RecoveryCatalog = read_checked_json(&catalog_path)?;
    validate_catalog(&catalog)?;
    Ok((config, catalog))
}

fn verify_unsealed_vault(
    root: &Path,
    catalog: &RecoveryCatalog,
) -> anyhow::Result<(u64, u64, u64, u64, u64)> {
    let events_root = root.join("events");
    let events_metadata = fs::symlink_metadata(&events_root)?;
    anyhow::ensure!(
        events_metadata.is_dir() && !events_metadata.file_type().is_symlink(),
        "Recovery Vault audit root is not a physical directory"
    );
    let audit = audit::recovery_audit_log_from_root(root)?;
    let mut repositories = 0_u64;
    let mut recovery_points = 0_u64;
    let mut objects = 0_u64;
    let mut raw_bytes = 0_u64;
    for registration in catalog.repositories.values() {
        repositories = repositories.saturating_add(1);
        if registration.recovery_points.is_empty() {
            continue;
        }
        let repository_root = vault_repository_root(root, registration.repository_id)?;
        let (_, repository_config) = repository_root_and_config(&repository_root)?;
        anyhow::ensure!(
            repository_config.repository_id == registration.repository_id,
            "Vault repository identity does not match the migration catalog"
        );
        let refs = repository_refs(&repository_root)?;
        for point in registration.recovery_points.values() {
            if point.state != RecoveryPointState::Available {
                continue;
            }
            let expected_tag = point
                .ref_name
                .strip_prefix("tags/")
                .ok_or_else(|| anyhow::anyhow!("invalid recovery point ref namespace"))?;
            anyhow::ensure!(
                refs.refs.iter().any(|reference| {
                    reference.kind == RepositoryRefKind::Tag
                        && reference.name == expected_tag
                        && reference.commit_id == point.commit_id
                }),
                "published recovery ref is missing or does not match the migration catalog"
            );
            recovery_points = recovery_points.saturating_add(1);
        }
        for reference in refs.refs.iter().filter(|reference| {
            reference.kind == RepositoryRefKind::Tag && reference.name.starts_with("recovery/")
        }) {
            let point_id = reference
                .name
                .strip_prefix("recovery/")
                .ok_or_else(|| anyhow::anyhow!("invalid recovery ref"))?;
            anyhow::ensure!(
                registration.recovery_points.contains_key(point_id),
                "orphan recovery ref is not present in the migration catalog"
            );
        }
        let verified = verify_repository(&repository_root)?;
        objects = objects.saturating_add(verified.checked_objects);
        raw_bytes = raw_bytes.saturating_add(verified.checked_raw_bytes);
    }
    Ok((
        repositories,
        recovery_points,
        objects,
        raw_bytes,
        audit.events.len().try_into()?,
    ))
}

fn validate_migration_mirror_catalog(
    primary: &RecoveryCatalog,
    mirror: &RecoveryCatalog,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        primary.repositories.len() == mirror.repositories.len(),
        "legacy recovery mirror repository set differs from the primary Vault"
    );
    for (key, primary_registration) in &primary.repositories {
        let mirror_registration = mirror.repositories.get(key).ok_or_else(|| {
            anyhow::anyhow!("legacy recovery mirror is missing a primary repository")
        })?;
        anyhow::ensure!(
            primary_registration.registration_id == mirror_registration.registration_id
                && primary_registration.repository_id == mirror_registration.repository_id
                && primary_registration.source_paths == mirror_registration.source_paths
                && primary_registration.tombstones == mirror_registration.tombstones,
            "legacy recovery mirror registration history differs from the primary Vault"
        );
        anyhow::ensure!(
            primary_registration.recovery_points.len() == mirror_registration.recovery_points.len(),
            "legacy recovery mirror point set differs from the primary Vault"
        );
        for (point_id, primary_point) in &primary_registration.recovery_points {
            let mirror_point = mirror_registration
                .recovery_points
                .get(point_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("legacy recovery mirror is missing a primary recovery point")
                })?;
            anyhow::ensure!(
                primary_point.commit_id == mirror_point.commit_id
                    && primary_point.ref_name == mirror_point.ref_name
                    && primary_point.reachable_objects == mirror_point.reachable_objects
                    && primary_point.pinned == mirror_point.pinned
                    && primary_point.state == mirror_point.state,
                "legacy recovery mirror point history differs from the primary Vault"
            );
        }
    }
    Ok(())
}

fn validate_mirror_roots(root: &Path, mirrors: &[PathBuf]) -> anyhow::Result<()> {
    let mut previous: Option<&Path> = None;
    for mirror in mirrors {
        anyhow::ensure!(mirror.is_absolute(), "recovery mirror root is not absolute");
        anyhow::ensure!(
            mirror != root && !mirror.starts_with(root) && !root.starts_with(mirror),
            "recovery primary and mirror roots cannot contain each other"
        );
        if let Some(previous) = previous {
            anyhow::ensure!(
                previous < mirror.as_path(),
                "recovery mirror roots are not canonical"
            );
        }
        previous = Some(mirror);
    }
    validate_pairwise_disjoint_mirrors(mirrors)?;
    Ok(())
}

fn update_mirror_registration(
    primary_root: &Path,
    mirror: &Path,
    registration: &RecoveryRegistration,
) -> anyhow::Result<()> {
    let primary_identity = auth::require_primary(primary_root)?;
    ensure_mirror_vault_with_audit(mirror, &primary_identity, false)?;
    let _lock = lock_vault(mirror)?;
    let mut catalog = load_catalog(mirror)?;
    let generation_before = catalog.generation;
    run_audited(
        mirror,
        RecoveryAuditOperation::MirrorSynchronize,
        Some(generation_before),
        Some(registration.repository_id),
        None,
        BTreeMap::from([("scope".to_string(), "registration".to_string())]),
        || {
            catalog.repositories.insert(
                hex::encode(registration.repository_id),
                registration.clone(),
            );
            catalog.generation = catalog.generation.saturating_add(1);
            save_catalog(mirror, &catalog)?;
            Ok(((), Some(catalog.generation)))
        },
    )
}

fn initialize_mirror_vault_with_audit(
    mirror: &Path,
    primary: &auth::VaultIdentity,
    allow_rebind: bool,
) -> anyhow::Result<()> {
    secure_create_dir(mirror)?;
    secure_create_dir(&mirror.join("events"))?;
    secure_create_dir(&mirror.join("locks"))?;
    let _lock = lock_vault(mirror)?;
    let generation_before = if catalog_path(mirror).exists() {
        enforce_private_file(&catalog_path(mirror))?;
        Some(load_catalog(mirror)?.generation)
    } else {
        None
    };
    run_audited(
        mirror,
        RecoveryAuditOperation::VaultInitialize,
        generation_before,
        None,
        None,
        BTreeMap::from([("replica_role".to_string(), "mirror".to_string())]),
        || {
            initialize_mirror_vault_root(mirror, primary, allow_rebind)?;
            Ok(((), Some(load_catalog(mirror)?.generation)))
        },
    )
}

fn ensure_mirror_vault_with_audit(
    mirror: &Path,
    primary: &auth::VaultIdentity,
    allow_rebind: bool,
) -> anyhow::Result<()> {
    if vault_config_path(mirror).exists() {
        initialize_mirror_vault_root(mirror, primary, allow_rebind)
    } else {
        initialize_mirror_vault_with_audit(mirror, primary, allow_rebind)
    }
}

fn initialize_mirror_vault_root(
    mirror: &Path,
    primary: &auth::VaultIdentity,
    allow_rebind: bool,
) -> anyhow::Result<()> {
    secure_create_dir(mirror)?;
    secure_create_dir(&mirror.join("locks"))?;
    secure_create_dir(&mirror.join("repositories"))?;
    secure_create_dir(&mirror.join("events"))?;
    auth::recover_state(mirror)?;
    let config_exists = vault_config_path(mirror).exists();
    let catalog_exists = catalog_path(mirror).exists();
    anyhow::ensure!(
        config_exists == catalog_exists,
        "Recovery mirror config/catalog initialization is incomplete"
    );
    if config_exists {
        let config = load_vault_config(mirror)?;
        anyhow::ensure!(
            config.mirror_roots.is_empty(),
            "recovery mirror target is configured as a primary Recovery Vault"
        );
        load_catalog(mirror)?;
        auth::initialize_mirror_identity(mirror, primary, allow_rebind)?;
    } else {
        auth::initialize_mirror_identity(mirror, primary, false)?;
        let config = RecoveryVaultConfig {
            schema: VAULT_SCHEMA,
            created_unix_ns: now_unix_ns(),
            mirror_roots: Vec::new(),
            retention: RecoveryRetentionPolicy::default(),
            at_rest_policy: RecoveryAtRestPolicy::default(),
        };
        let catalog = RecoveryCatalog::default();
        let config_bytes = checked_json_bytes(&config)?;
        let catalog_bytes = checked_json_bytes(&catalog)?;
        auth::initialize_state(mirror, catalog.generation, &config_bytes, &catalog_bytes)?;
    }
    auth::require_mirror_for(mirror, primary)?;
    Ok(())
}

fn update_mirror_retention(
    primary_root: &Path,
    mirror: &Path,
    retention: &RecoveryRetentionPolicy,
) -> anyhow::Result<()> {
    let primary_identity = auth::require_primary(primary_root)?;
    ensure_mirror_vault_with_audit(mirror, &primary_identity, false)?;
    let _lock = lock_vault(mirror)?;
    let mut config = load_vault_config(mirror)?;
    let generation = load_catalog(mirror)?.generation;
    run_audited(
        mirror,
        RecoveryAuditOperation::RetentionUpdate,
        Some(generation),
        None,
        None,
        BTreeMap::from([("replica_role".to_string(), "mirror".to_string())]),
        || {
            config.retention = retention.clone();
            save_vault_config(mirror, &config)?;
            Ok(((), Some(generation)))
        },
    )
}

fn validate_relative_label(path: &str) -> anyhow::Result<()> {
    validate_recovery_text("tombstone path", path, MAX_RECOVERY_PATH_BYTES)?;
    let value = Path::new(path);
    anyhow::ensure!(!value.is_absolute(), "tombstone path must be relative");
    anyhow::ensure!(
        value
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "tombstone path contains unsafe components"
    );
    Ok(())
}

fn validate_persisted_absolute_path_label(path: &str) -> anyhow::Result<()> {
    validate_recovery_text("persisted recovery path", path, MAX_RECOVERY_PATH_BYTES)?;
    let bytes = path.as_bytes();
    let unix_absolute = bytes.first() == Some(&b'/');
    let windows_drive_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    let windows_unc_absolute = bytes.starts_with(b"\\\\") && bytes.len() > 2;
    anyhow::ensure!(
        !path.contains('\0') && (unix_absolute || windows_drive_absolute || windows_unc_absolute),
        "persisted recovery path label is not absolute"
    );
    Ok(())
}

fn validate_recovery_text(label: &str, value: &str, maximum: usize) -> anyhow::Result<()> {
    anyhow::ensure!(value.len() <= maximum, "{label} exceeds resource limit");
    anyhow::ensure!(!value.contains('\0'), "{label} contains a null character");
    Ok(())
}

fn bounded_recovery_text(value: &str) -> String {
    let sanitized = value.replace('\0', " ");
    if sanitized.len() <= MAX_RECOVERY_TEXT_BYTES {
        return sanitized;
    }
    let mut end = MAX_RECOVERY_TEXT_BYTES;
    while !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    sanitized[..end].to_string()
}

fn load_catalog(root: &Path) -> anyhow::Result<RecoveryCatalog> {
    auth::recover_state(root)?;
    let path = catalog_path(root);
    enforce_private_file(&path)?;
    let catalog: RecoveryCatalog = read_checked_json(&path)?;
    validate_catalog(&catalog)?;
    auth::verify_state(root, catalog.generation)?;
    Ok(catalog)
}

fn save_catalog(root: &Path, catalog: &RecoveryCatalog) -> anyhow::Result<()> {
    validate_catalog(catalog)?;
    let bytes = checked_json_bytes(catalog)?;
    auth::write_catalog(root, catalog.generation, &bytes)
}

fn save_vault_config(root: &Path, config: &RecoveryVaultConfig) -> anyhow::Result<()> {
    config.retention.validate()?;
    validate_mirror_roots(root, &config.mirror_roots)?;
    let catalog: RecoveryCatalog = read_checked_json(&catalog_path(root))?;
    validate_catalog(&catalog)?;
    let bytes = checked_json_bytes(config)?;
    auth::write_config(root, catalog.generation, &bytes)
}

fn validate_catalog(catalog: &RecoveryCatalog) -> anyhow::Result<()> {
    anyhow::ensure!(
        catalog.schema == CATALOG_SCHEMA,
        "unsupported recovery catalog schema"
    );
    anyhow::ensure!(
        catalog.repositories.len() <= MAX_RECOVERY_REPOSITORIES,
        "recovery repository count exceeds resource limit"
    );
    for (repository_key, registration) in &catalog.repositories {
        validate_registration(repository_key, registration)?;
    }
    Ok(())
}

fn validate_registration(
    repository_key: &str,
    registration: &RecoveryRegistration,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        registration.schema == VAULT_SCHEMA,
        "unsupported recovery registration schema"
    );
    anyhow::ensure!(
        repository_key == hex::encode(registration.repository_id),
        "recovery registration key does not match repository identity"
    );
    anyhow::ensure!(
        registration.created_unix_ns <= registration.updated_unix_ns,
        "recovery registration time regresses"
    );
    anyhow::ensure!(
        !registration.source_paths.is_empty(),
        "recovery registration has no source path history"
    );
    anyhow::ensure!(
        registration.source_paths.len() <= MAX_RECOVERY_SOURCE_PATHS,
        "recovery source path history exceeds resource limit"
    );
    anyhow::ensure!(
        registration.recovery_points.len() <= MAX_RECOVERY_POINTS_PER_REPOSITORY,
        "recovery point count exceeds resource limit"
    );
    anyhow::ensure!(
        registration.tombstones.len() <= MAX_RECOVERY_TOMBSTONES_PER_REPOSITORY,
        "recovery tombstone count exceeds resource limit"
    );
    let mut previous_source: Option<&str> = None;
    for source in &registration.source_paths {
        validate_persisted_absolute_path_label(source).map_err(|error| {
            anyhow::anyhow!("recovery source path history contains an invalid path: {error}")
        })?;
        if let Some(previous) = previous_source {
            anyhow::ensure!(
                previous < source.as_str(),
                "recovery source path history is not canonical"
            );
        }
        previous_source = Some(source);
    }

    for (point_id, point) in &registration.recovery_points {
        validate_recovery_point(point_id, point)?;
    }
    let mut tombstone_ids = BTreeSet::new();
    for tombstone in &registration.tombstones {
        anyhow::ensure!(
            tombstone.schema == VAULT_SCHEMA,
            "unsupported recovery tombstone schema"
        );
        anyhow::ensure!(
            tombstone_ids.insert(tombstone.tombstone_id),
            "duplicate recovery tombstone identity"
        );
        anyhow::ensure!(
            !tombstone.reason.trim().is_empty() && !tombstone.reason.contains('\0'),
            "recovery tombstone reason is invalid"
        );
        validate_recovery_text(
            "recovery tombstone reason",
            &tombstone.reason,
            MAX_RECOVERY_TEXT_BYTES,
        )?;
        if let Some(source) = tombstone.source_path.as_deref() {
            validate_persisted_absolute_path_label(source).map_err(|error| {
                anyhow::anyhow!("recovery tombstone source path is invalid: {error}")
            })?;
        }
        match tombstone.kind {
            RecoveryTombstoneKind::File => {
                let relative = tombstone.relative_path.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("file recovery tombstone has no relative path")
                })?;
                validate_relative_label(relative)?;
            }
            RecoveryTombstoneKind::Workspace | RecoveryTombstoneKind::Registration => {
                anyhow::ensure!(
                    tombstone.relative_path.is_none(),
                    "non-file recovery tombstone contains a relative path"
                );
            }
        }
    }
    Ok(())
}

fn validate_recovery_point(point_id: &str, point: &RecoveryPoint) -> anyhow::Result<()> {
    anyhow::ensure!(
        point.schema == VAULT_SCHEMA,
        "unsupported recovery point schema"
    );
    anyhow::ensure!(
        point_id == point.recovery_point_id && point_id == point.commit_id.to_hex(),
        "recovery point identity does not match its commit"
    );
    anyhow::ensure!(
        point.ref_name == format!("tags/recovery/{point_id}"),
        "recovery point ref is not canonical"
    );
    anyhow::ensure!(
        point.captured_unix_ns <= point.last_verified_unix_ns,
        "recovery point verification time regresses"
    );
    anyhow::ensure!(
        point.replicas.len() <= MAX_RECOVERY_REPLICAS_PER_POINT,
        "recovery replica count exceeds resource limit"
    );
    let mut replica_roots = BTreeSet::new();
    for replica in &point.replicas {
        validate_persisted_absolute_path_label(&replica.vault_root)
            .map_err(|error| anyhow::anyhow!("recovery replica root is invalid: {error}"))?;
        anyhow::ensure!(
            replica_roots.insert(replica.vault_root.as_str()),
            "duplicate recovery replica root"
        );
        anyhow::ensure!(
            replica.verified == replica.error.is_none(),
            "recovery replica verification and error state disagree"
        );
        if let Some(error) = replica.error.as_deref() {
            validate_recovery_text("recovery replica error", error, MAX_RECOVERY_TEXT_BYTES)?;
        }
    }
    match point.durability {
        RecoveryDurability::Captured => {}
        RecoveryDurability::Protected => anyhow::ensure!(
            !point.replicas.is_empty() && point.replicas.iter().all(|replica| replica.verified),
            "protected recovery point has no complete verified replica set"
        ),
        RecoveryDurability::Degraded => anyhow::ensure!(
            !point.replicas.is_empty() && point.replicas.iter().any(|replica| !replica.verified),
            "degraded recovery point has no failed replica"
        ),
    }
    Ok(())
}

fn find_registration(
    catalog: &RecoveryCatalog,
    repository_id: [u8; 16],
) -> anyhow::Result<&RecoveryRegistration> {
    catalog
        .repositories
        .get(&hex::encode(repository_id))
        .ok_or_else(|| anyhow::anyhow!("recovery repository not found"))
}

fn parse_repository_id(value: &str) -> anyhow::Result<[u8; 16]> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("recovery repository id must contain 16 bytes"))
}

fn vault_repository_root(root: &Path, repository_id: [u8; 16]) -> anyhow::Result<PathBuf> {
    ensure_physical_directory(root)?;
    let repositories = root.join("repositories");
    ensure_physical_directory(&repositories)?;
    let repository = repositories.join(hex::encode(repository_id));
    if repository.exists() {
        ensure_physical_directory(&repository)?;
        let physical_root = root.canonicalize()?;
        let physical_repository = repository.canonicalize()?;
        anyhow::ensure!(
            physical_repository.starts_with(&physical_root),
            "recovery repository resolves outside the Vault root"
        );
    }
    Ok(repository)
}

fn vault_config_path(root: &Path) -> PathBuf {
    root.join("config.json")
}

fn catalog_path(root: &Path) -> PathBuf {
    root.join("catalog.json")
}

fn lock_vault(root: &Path) -> anyhow::Result<File> {
    let lock_path = root.join("locks").join("write.lock");
    if lock_path.exists() {
        enforce_private_file(&lock_path)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
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
    let file = options.open(&lock_path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "recovery lock is not a file");
    enforce_private_file(&lock_path)?;
    file.lock_exclusive()?;
    Ok(file)
}

fn read_checked_json<T: DeserializeOwned + Serialize>(path: &Path) -> anyhow::Result<T> {
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
        "recovery document is not a physical file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_RECOVERY_DOCUMENT_BYTES,
        "recovery document exceeds resource limit"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take(MAX_RECOVERY_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECOVERY_DOCUMENT_BYTES,
        "recovery document exceeds resource limit"
    );
    let document: CheckedDocument<T> = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        document.schema == DOCUMENT_SCHEMA,
        "unsupported recovery document schema"
    );
    let payload = serde_json::to_vec(&document.payload)?;
    anyhow::ensure!(
        blake3::hash(&payload).to_hex().as_str() == document.payload_blake3,
        "recovery document checksum mismatch"
    );
    Ok(document.payload)
}

#[cfg(test)]
fn write_checked_json<T: Serialize>(path: &Path, payload: &T) -> anyhow::Result<()> {
    let bytes = checked_json_bytes(payload)?;
    atomic_write(path, &bytes)
}

fn checked_json_bytes<T: Serialize>(payload: &T) -> anyhow::Result<Vec<u8>> {
    let payload_bytes = serde_json::to_vec(payload)?;
    anyhow::ensure!(
        payload_bytes.len() as u64 <= MAX_RECOVERY_DOCUMENT_BYTES,
        "recovery document exceeds resource limit"
    );
    let document = CheckedDocument {
        schema: DOCUMENT_SCHEMA,
        payload_blake3: blake3::hash(&payload_bytes).to_hex().to_string(),
        payload,
    };
    let bytes = serde_json::to_vec_pretty(&document)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECOVERY_DOCUMENT_BYTES,
        "recovery document exceeds resource limit"
    );
    Ok(bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
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
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
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

fn secure_create_dir(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    ensure_physical_directory(path)?;
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "recovery private directory is not owned by the current user: {}",
            path.display()
        );
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    windows_owner_only::apply_and_verify(path)?;
    Ok(())
}

fn ensure_physical_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "recovery private path is not a physical directory: {}",
        path.display()
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        anyhow::ensure!(
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "recovery private path is a reparse point: {}",
            path.display()
        );
    }
    Ok(())
}

fn enforce_private_file(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "recovery private path is not a physical file: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "recovery private file is not owned by the current user: {}",
            path.display()
        );
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    windows_owner_only::apply_and_verify(path)?;
    Ok(())
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

fn absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let normalized = normalize_absolute_path(&absolute)?;
    if normalized.exists() {
        return Ok(normalized.canonicalize()?);
    }
    let mut ancestor = normalized.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("recovery path has no existing ancestor"))?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("recovery path has no existing ancestor"))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn normalize_absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                anyhow::ensure!(
                    normalized.pop(),
                    "recovery path escapes the filesystem root"
                );
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    anyhow::ensure!(
        normalized.is_absolute(),
        "recovery path must resolve as absolute"
    );
    Ok(normalized)
}

fn required_environment_path(name: &str) -> anyhow::Result<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("{name} is not set; configure HIG_RECOVERY_VAULT"))
}

fn now_unix_ns() -> i128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}

#[cfg(windows)]
mod windows_owner_only {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };

    const OWNER_ONLY_SDDL: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)";

    struct LocalDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for LocalDescriptor {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    pub(super) fn apply_and_verify(path: &Path) -> anyhow::Result<()> {
        let mut sddl = OWNER_ONLY_SDDL.encode_utf16().collect::<Vec<_>>();
        sddl.push(0);
        let mut expected_descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut expected_descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let expected_descriptor = LocalDescriptor(expected_descriptor);
        let expected_dacl = descriptor_dacl(expected_descriptor.0)?;

        let mut path_wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        path_wide.push(0);
        let set_result = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                expected_dacl,
                null_mut(),
            )
        };
        if set_result != 0 {
            return Err(std::io::Error::from_raw_os_error(set_result as i32).into());
        }

        let mut actual_dacl = null_mut();
        let mut actual_descriptor = null_mut();
        let get_result = unsafe {
            GetNamedSecurityInfoW(
                path_wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut actual_dacl,
                null_mut(),
                &mut actual_descriptor,
            )
        };
        if get_result != 0 {
            return Err(std::io::Error::from_raw_os_error(get_result as i32).into());
        }
        let actual_descriptor = LocalDescriptor(actual_descriptor);
        anyhow::ensure!(!actual_dacl.is_null(), "recovery private DACL is missing");

        let mut control = 0_u16;
        let mut revision = 0_u32;
        anyhow::ensure!(
            unsafe {
                GetSecurityDescriptorControl(actual_descriptor.0, &mut control, &mut revision)
            } != 0,
            "failed to inspect recovery private DACL control"
        );
        anyhow::ensure!(
            control & SE_DACL_PROTECTED != 0,
            "recovery private DACL permits inherited access"
        );
        anyhow::ensure!(
            acl_bytes(expected_dacl) == acl_bytes(actual_dacl),
            "recovery private DACL failed exact verification"
        );
        Ok(())
    }

    fn descriptor_dacl(descriptor: PSECURITY_DESCRIPTOR) -> anyhow::Result<*mut ACL> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = null_mut();
        anyhow::ensure!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
            } != 0,
            "failed to read recovery private DACL"
        );
        anyhow::ensure!(
            present != 0 && !dacl.is_null(),
            "recovery private DACL is missing"
        );
        Ok(dacl)
    }

    fn acl_bytes(acl: *const ACL) -> Vec<u8> {
        let length = unsafe { (*acl).AclSize as usize };
        unsafe { std::slice::from_raw_parts(acl.cast(), length) }.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{init_repository, snapshot_repository};

    #[cfg(unix)]
    #[test]
    fn vault_initialization_repairs_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        fs::set_permissions(&vault, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(vault.join("locks"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(
            vault.join("repositories"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(vault_config_path(&vault), fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(catalog_path(&vault), fs::Permissions::from_mode(0o644)).unwrap();

        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let lock = lock_vault(&vault).unwrap();
        drop(lock);

        for path in [&vault, &vault.join("locks"), &vault.join("repositories")] {
            assert_eq!(
                fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777,
                0o700,
                "{}",
                path.display()
            );
        }
        for path in [
            vault_config_path(&vault),
            catalog_path(&vault),
            vault.join("locks/write.lock"),
        ] {
            assert_eq!(
                fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "{}",
                path.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn vault_control_file_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        let external = temp.path().join("external.json");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        fs::rename(vault_config_path(&vault), &external).unwrap();
        symlink(&external, vault_config_path(&vault)).unwrap();

        let error = init_recovery_vault(Some(&vault), Vec::new()).unwrap_err();
        assert!(error.to_string().contains("not a physical file"));
    }

    #[cfg(unix)]
    #[test]
    fn vault_repository_directory_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        let external = temp.path().join("external-repository");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "protected").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let captured = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        let repository = vault_repository_root(&vault, initialized.repository_id).unwrap();
        fs::rename(&repository, &external).unwrap();
        symlink(&external, &repository).unwrap();

        let error = verify_recovery_point(
            Some(&vault),
            &hex::encode(initialized.repository_id),
            &captured.recovery_point.recovery_point_id,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a physical directory"));
        assert!(external.join(".hig/repository/config.json").is_file());
    }

    #[test]
    fn nested_mirror_roots_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        let nested = mirror.join("nested");

        let error = init_recovery_vault(Some(&primary), vec![mirror, nested]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("recovery mirror roots cannot contain each other")
        );
    }

    #[test]
    fn init_cannot_replace_mirrors_after_capture() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let first_mirror = temp.path().join("first-mirror");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "protected").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![first_mirror.clone()]).unwrap();
        capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();

        let error = init_recovery_vault(Some(&primary), vec![replacement]).unwrap_err();
        assert!(error.to_string().contains("use recovery promote"));
        assert_eq!(
            recovery_vault_config(Some(&primary)).unwrap().mirror_roots,
            vec![first_mirror.canonicalize().unwrap()]
        );
        let status = recovery_vault_status(Some(&primary)).unwrap();
        assert_eq!(status.protected_points, 1);
        assert_eq!(status.durability_lag_points, 0);
    }

    #[test]
    fn restore_rejects_output_overlapping_the_vault() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "recoverable").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let captured = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        let repository_id = hex::encode(initialized.repository_id);

        for output in [&vault, temp.path()] {
            let error = restore_recovery_point(
                Some(&vault),
                &repository_id,
                &captured.recovery_point.recovery_point_id,
                output,
                None,
                true,
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("restore output cannot contain each other")
            );
        }
        assert!(
            verify_recovery_point(
                Some(&vault),
                &repository_id,
                &captured.recovery_point.recovery_point_id
            )
            .is_ok()
        );
    }

    #[test]
    fn capture_rejects_a_primary_vault_as_a_mirror() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let other_primary = temp.path().join("other-primary");
        let other_mirror = temp.path().join("other-mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "protected").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&other_primary), vec![other_mirror]).unwrap();
        let error = init_recovery_vault(Some(&primary), vec![other_primary.clone()]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("configured as a primary Recovery Vault")
        );
        assert_eq!(
            recovery_vault_status(Some(&other_primary))
                .unwrap()
                .recovery_points,
            0
        );
    }

    #[test]
    fn audit_log_records_committed_recovery_operations_with_generations() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        let restored = temp.path().join("restored");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "audited").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();

        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        register_recovery_repository(&source, Some(&vault)).unwrap();
        let captured = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        restore_recovery_point(
            Some(&vault),
            &hex::encode(initialized.repository_id),
            &captured.recovery_point.recovery_point_id,
            &restored,
            None,
            false,
        )
        .unwrap();

        let audit = recovery_audit_log(Some(&vault)).unwrap();
        assert!(audit.incomplete_operation_ids.is_empty());
        for operation in [
            RecoveryAuditOperation::VaultInitialize,
            RecoveryAuditOperation::RepositoryRegister,
            RecoveryAuditOperation::Capture,
            RecoveryAuditOperation::Restore,
        ] {
            assert!(audit.events.iter().any(|event| {
                event.operation == operation
                    && event.outcome == RecoveryAuditOutcome::Committed
                    && event.catalog_generation_after.is_some()
            }));
        }
        assert_eq!(
            fs::read_to_string(restored.join("file.txt")).unwrap(),
            "audited"
        );
    }

    #[test]
    fn failed_mutation_has_a_terminal_audit_event() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();

        let error = set_recovery_point_pin(
            Some(&vault),
            "00000000000000000000000000000000",
            &"0".repeat(64),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not found"));

        let audit = recovery_audit_log(Some(&vault)).unwrap();
        assert!(audit.incomplete_operation_ids.is_empty());
        assert!(audit.events.iter().any(|event| {
            event.operation == RecoveryAuditOperation::PinUpdate
                && event.outcome == RecoveryAuditOutcome::Failed
                && event
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("not found"))
        }));
    }

    #[test]
    fn prepared_audit_event_survives_as_an_interruption_record() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let generation = load_catalog(&vault).unwrap().generation;
        let transaction = begin_audit(
            &vault,
            RecoveryAuditOperation::GarbageCollection,
            Some(generation),
            None,
            None,
            BTreeMap::from([("fault".to_string(), "process_termination".to_string())]),
        )
        .unwrap();
        let operation_id = transaction.prepared.operation_id.clone();
        drop(transaction);

        let audit = recovery_audit_log(Some(&vault)).unwrap();
        assert_eq!(audit.incomplete_operation_ids, vec![operation_id]);
        assert_eq!(
            audit
                .events
                .iter()
                .filter(|event| event.operation == RecoveryAuditOperation::GarbageCollection)
                .count(),
            1
        );
    }

    #[test]
    fn audit_checksum_corruption_fails_closed_and_scrub_reports_it() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let event_path = fs::read_dir(vault.join("events"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut bytes = fs::read(&event_path).unwrap();
        let position = bytes.iter().position(|byte| *byte == b'0').unwrap();
        bytes[position] = b'1';
        fs::write(event_path, bytes).unwrap();

        assert!(recovery_audit_log(Some(&vault)).is_err());
        let scrub = scrub_recovery_vault(Some(&vault)).unwrap();
        assert!(!scrub.healthy);
        assert!(!scrub.locations[0].errors.is_empty());
    }

    #[test]
    fn immutable_audit_publication_never_replaces_an_existing_event() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("event.json");
        fs::write(&path, b"existing").unwrap();

        assert!(atomic_write_new(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn capture_survives_complete_source_deletion_and_restores_exact_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        let output = temp.path().join("restored");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/data.bin"), b"recovery-vault\0exact\xff").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), Some("test".into())).unwrap();

        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let capture = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        let repository_id = hex::encode(initialized.repository_id);
        fs::remove_dir_all(&source).unwrap();

        let status = recovery_vault_status(Some(&vault)).unwrap();
        assert_eq!(status.schema, RECOVERY_REPORT_SCHEMA);
        assert_eq!(status.repositories, 1);
        assert_eq!(status.recovery_points, 1);
        assert_eq!(status.available_points, 1);
        assert_eq!(status.captured_points, 1);
        assert_eq!(status.durability_lag_points, 1);
        assert!(status.rpo_lag_millis.is_some());
        assert_eq!(status.incomplete_audit_operations, 0);

        let verified = verify_recovery_point(
            Some(&vault),
            &repository_id,
            &capture.recovery_point.recovery_point_id,
        )
        .unwrap();
        assert!(verified.repository.checked_objects > 0);
        restore_recovery_point(
            Some(&vault),
            &repository_id,
            &capture.recovery_point.recovery_point_id,
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("nested/data.bin")).unwrap(),
            b"recovery-vault\0exact\xff"
        );
    }

    #[test]
    fn legacy_primary_and_mirror_migrate_only_after_full_verification() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("critical.bin"), b"legacy-exact\0\xff").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "legacy".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        let captured = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        for vault in [&primary, &mirror] {
            fs::remove_file(vault.join("identity.json")).unwrap();
            fs::remove_file(vault.join("state.json")).unwrap();
        }
        fs::remove_dir_all(temp.path().join(".hig-recovery-auth")).unwrap();

        let migrated = migrate_recovery_auth(Some(&primary)).unwrap();
        assert!(migrated.created);
        assert_eq!(
            migrated.migrated_mirrors,
            vec![mirror.canonicalize().unwrap().display().to_string()]
        );
        assert_eq!(migrated.verified_repositories, 2);
        assert_eq!(migrated.verified_recovery_points, 2);
        assert!(migrated.verified_objects > 0);
        assert!(migrated.verified_audit_events >= 8);
        let repeated = migrate_recovery_auth(Some(&primary)).unwrap();
        assert!(!repeated.created);
        assert!(repeated.migrated_mirrors.is_empty());

        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&primary).unwrap();
        let output = temp.path().join("restored");
        restore_recovery_point(
            Some(&mirror),
            &hex::encode(initialized.repository_id),
            &captured.recovery_point.recovery_point_id,
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("critical.bin")).unwrap(),
            b"legacy-exact\0\xff"
        );
    }

    #[test]
    fn legacy_migration_refuses_to_authenticate_corrupt_repository_objects() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.bin"), b"must-verify").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "legacy".into(), None).unwrap();
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        fs::remove_file(vault.join("identity.json")).unwrap();
        fs::remove_file(vault.join("state.json")).unwrap();
        fs::remove_dir_all(temp.path().join(".hig-recovery-auth")).unwrap();
        let objects = vault_repository_root(&vault, initialized.repository_id)
            .unwrap()
            .join(".hig/repository/objects");
        let object = walkdir::WalkDir::new(objects)
            .into_iter()
            .filter_map(Result::ok)
            .find(|entry| entry.file_type().is_file())
            .unwrap()
            .into_path();
        fs::write(object, b"corrupt").unwrap();

        let error = migrate_recovery_auth(Some(&vault)).unwrap_err();
        assert!(
            error.to_string().contains("checksum")
                || error.to_string().contains("object")
                || error.to_string().contains("decode")
        );
        assert!(!vault.join("identity.json").exists());
        assert!(!vault.join("state.json").exists());
    }

    #[test]
    fn auth_key_rotation_converges_after_mirror_first_interruption() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("critical.bin"), b"before-rotation").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "before rotation".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        let original_key_id = auth::load_identity(&primary).unwrap().key_id;

        let error = with_recovery_failpoint("auth_rotation_after_mirrors", || {
            rotate_recovery_auth_key(Some(&primary)).unwrap_err()
        });
        assert!(error.to_string().contains("auth_rotation_after_mirrors"));
        assert_eq!(
            auth::load_identity(&primary).unwrap().key_id,
            original_key_id
        );
        assert_ne!(
            auth::load_identity(&mirror).unwrap().key_id,
            original_key_id
        );

        let rotated = rotate_recovery_auth_key(Some(&primary)).unwrap();
        assert_ne!(rotated.key_id, original_key_id);
        assert_eq!(rotated.rotated_vaults.len(), 2);
        assert!(rotated.old_keys_retained);
        assert_eq!(
            auth::load_identity(&primary).unwrap().key_id,
            rotated.key_id
        );
        assert_eq!(auth::load_identity(&mirror).unwrap().key_id, rotated.key_id);

        fs::write(source.join("critical.bin"), b"after-rotation\0\xff").unwrap();
        snapshot_repository(&source, "after rotation".into(), None).unwrap();
        let captured = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        assert_eq!(
            captured.recovery_point.durability,
            RecoveryDurability::Protected
        );
        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&primary).unwrap();
        let output = temp.path().join("restored");
        restore_recovery_point(
            Some(&mirror),
            &hex::encode(initialized.repository_id),
            &captured.recovery_point.recovery_point_id,
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("critical.bin")).unwrap(),
            b"after-rotation\0\xff"
        );
    }

    #[test]
    fn repeated_capture_is_idempotent_and_mirror_is_independently_restorable() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "durable").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();

        let first = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        let second = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(
            second.recovery_point.durability,
            RecoveryDurability::Protected
        );
        assert_eq!(second.recovery_point.stored_objects_written, 0);

        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&primary).unwrap();
        let output = temp.path().join("mirror-restore");
        restore_recovery_point(
            Some(&mirror),
            &hex::encode(initialized.repository_id),
            &first.recovery_point.recovery_point_id,
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(output.join("file.txt")).unwrap(),
            "durable"
        );
    }

    #[test]
    fn verified_mirror_promotes_after_source_and_primary_loss() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let survivor = temp.path().join("survivor");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("critical.bin"), b"exact-after-total-loss\0\xff").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "protected baseline".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![survivor.clone()]).unwrap();
        let captured = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        assert_eq!(
            captured.recovery_point.durability,
            RecoveryDurability::Protected
        );

        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&primary).unwrap();
        let promoted = promote_recovery_vault(Some(&survivor), vec![replacement.clone()]).unwrap();
        assert_eq!(promoted.schema, RECOVERY_REPORT_SCHEMA);
        assert!(promoted.changed);
        assert_eq!(promoted.repositories, 1);
        assert_eq!(promoted.recovery_points, 1);
        assert!(promoted.objects_written > 0);
        assert_eq!(promoted.durability, RecoveryDurability::Protected);
        assert!(scrub_recovery_vault(Some(&survivor)).unwrap().healthy);

        let retried = promote_recovery_vault(Some(&survivor), vec![replacement.clone()]).unwrap();
        assert!(!retried.changed);
        assert_eq!(retried.generation_before, retried.generation_after);
        assert_eq!(retried.objects_written, 0);
        let status = recovery_vault_status(Some(&survivor)).unwrap();
        assert_eq!(status.configured_mirrors, 1);
        assert_eq!(status.protected_points, 1);
        assert_eq!(status.durability_lag_points, 0);
        let audit = recovery_audit_log(Some(&survivor)).unwrap();
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.operation == RecoveryAuditOperation::VaultPromote)
        );

        fs::remove_dir_all(&survivor).unwrap();
        let output = temp.path().join("restored-from-promoted-mirror");
        restore_recovery_point(
            Some(&replacement),
            &hex::encode(initialized.repository_id),
            &captured.recovery_point.recovery_point_id,
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("critical.bin")).unwrap(),
            b"exact-after-total-loss\0\xff"
        );
    }

    #[test]
    fn promotion_rejects_target_only_repository_history() {
        let temp = tempfile::tempdir().unwrap();
        let source_a = temp.path().join("source-a");
        let source_b = temp.path().join("source-b");
        let source_primary = temp.path().join("source-primary");
        let survivor = temp.path().join("survivor");
        let unrelated_target = temp.path().join("unrelated-target");
        for (source, content) in [(&source_a, "a"), (&source_b, "b")] {
            fs::create_dir_all(source).unwrap();
            fs::write(source.join("file.txt"), content).unwrap();
            init_repository(source, Vec::new()).unwrap();
            snapshot_repository(source, "baseline".into(), None).unwrap();
        }

        init_recovery_vault(Some(&source_primary), vec![survivor.clone()]).unwrap();
        capture_recovery_point(&source_a, "HEAD", Some(&source_primary)).unwrap();
        init_recovery_vault(Some(&unrelated_target), Vec::new()).unwrap();
        capture_recovery_point(&source_b, "HEAD", Some(&unrelated_target)).unwrap();

        let error = promote_recovery_vault(Some(&survivor), vec![unrelated_target]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("different authenticated lineage")
        );
        assert_eq!(
            recovery_vault_status(Some(&survivor))
                .unwrap()
                .protected_points,
            0
        );
    }

    #[test]
    fn catalog_checksum_corruption_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        init_recovery_vault(Some(&vault), Vec::new()).unwrap();
        let mut bytes = fs::read(catalog_path(&vault)).unwrap();
        let position = bytes.iter().position(|byte| *byte == b'0').unwrap();
        bytes[position] = b'1';
        fs::write(catalog_path(&vault), bytes).unwrap();
        assert!(list_recovery_vault(Some(&vault)).is_err());
    }

    #[test]
    fn vault_inside_source_is_rejected_before_it_is_created() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let nested_vault = source.join("private/recovery");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "source").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();

        assert!(capture_recovery_point(&source, "HEAD", Some(&nested_vault)).is_err());
        assert!(!nested_vault.exists());
    }

    #[test]
    fn verification_requires_the_catalogued_recovery_ref() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "source").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        let capture = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        let recovery_ref = vault_repository_root(&vault, initialized.repository_id)
            .unwrap()
            .join(".hig/repository/refs/tags/recovery")
            .join(&capture.recovery_point.recovery_point_id);
        fs::remove_file(recovery_ref).unwrap();

        assert!(
            verify_recovery_point(
                Some(&vault),
                &hex::encode(initialized.repository_id),
                &capture.recovery_point.recovery_point_id,
            )
            .is_err()
        );
    }

    #[test]
    fn pin_and_tombstone_state_replicate_without_removing_recovery_data() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "source").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        let capture = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        let repository_id = hex::encode(initialized.repository_id);

        let pin = set_recovery_point_pin(
            Some(&primary),
            &repository_id,
            &capture.recovery_point.recovery_point_id,
            true,
        )
        .unwrap();
        assert!(pin.changed && pin.pinned);
        let tombstone = record_recovery_tombstone(
            Some(&primary),
            &repository_id,
            RecoveryTombstoneKind::File,
            Some(source.display().to_string()),
            Some("file.txt".into()),
            "deleted by test".into(),
        )
        .unwrap();
        assert_eq!(tombstone.tombstone.kind, RecoveryTombstoneKind::File);

        for vault in [&primary, &mirror] {
            let report = list_recovery_vault(Some(vault)).unwrap();
            let registration = &report.repositories[0];
            assert!(registration.recovery_points[&capture.recovery_point.recovery_point_id].pinned);
            assert_eq!(registration.tombstones.len(), 1);
            verify_recovery_point(
                Some(vault),
                &repository_id,
                &capture.recovery_point.recovery_point_id,
            )
            .unwrap();
        }
    }

    #[test]
    fn retention_policy_rejects_limits_below_protected_minimum() {
        let policy = RecoveryRetentionPolicy {
            minimum_points_per_repository: 5,
            maximum_points_per_repository: Some(4),
            ..RecoveryRetentionPolicy::default()
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn legacy_vault_config_defaults_to_the_external_encryption_policy() {
        let legacy = r#"{
            "schema": 1,
            "created_unix_ns": 1,
            "mirror_roots": [],
            "retention": {
                "schema": 1,
                "minimum_points_per_repository": 3,
                "minimum_retention_days": 7,
                "maximum_points_per_repository": null,
                "maximum_vault_bytes": null
            }
        }"#;

        let config: RecoveryVaultConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(
            config.at_rest_policy,
            RecoveryAtRestPolicy::ExternalEncryptionRequired
        );

        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(
            encoded["at_rest_policy"],
            serde_json::Value::String("external_encryption_required".into())
        );
    }

    #[test]
    fn recovery_gc_defaults_to_report_only_with_no_implicit_expiration() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "one").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "one".into(), None).unwrap();
        capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();

        let preview = gc_recovery_vault(Some(&vault), true).unwrap();
        assert!(preview.dry_run);
        assert_eq!(preview.total_recovery_points, 1);
        assert_eq!(preview.candidate_recovery_points, 0);
        assert_eq!(preview.removed_recovery_points, 0);
        assert_eq!(
            list_recovery_vault(Some(&vault)).unwrap().repositories[0]
                .recovery_points
                .len(),
            1
        );
    }

    #[test]
    fn recovery_gc_enforces_point_limit_and_preserves_latest_exact_restore() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        let mut points = Vec::new();
        for value in ["one", "two", "three"] {
            fs::write(source.join("file.txt"), value).unwrap();
            snapshot_repository(&source, value.into(), None).unwrap();
            points.push(
                capture_recovery_point(&source, "HEAD", Some(&vault))
                    .unwrap()
                    .recovery_point
                    .recovery_point_id,
            );
        }
        update_recovery_retention(
            Some(&vault),
            RecoveryRetentionPolicy {
                minimum_points_per_repository: 1,
                minimum_retention_days: 0,
                maximum_points_per_repository: Some(1),
                ..RecoveryRetentionPolicy::default()
            },
        )
        .unwrap();

        let preview = gc_recovery_vault(Some(&vault), true).unwrap();
        assert_eq!(preview.candidate_recovery_points, 2);
        assert_eq!(preview.removed_recovery_points, 0);
        let applied = gc_recovery_vault(Some(&vault), false).unwrap();
        assert_eq!(applied.removed_recovery_points, 2);
        assert!(applied.policy_satisfied);
        let remaining = list_recovery_vault(Some(&vault)).unwrap();
        assert_eq!(remaining.repositories[0].recovery_points.len(), 1);
        assert!(
            remaining.repositories[0]
                .recovery_points
                .contains_key(&points[2])
        );

        fs::remove_dir_all(&source).unwrap();
        let output = temp.path().join("restored");
        restore_recovery_point(
            Some(&vault),
            &hex::encode(initialized.repository_id),
            &points[2],
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(output.join("file.txt")).unwrap(),
            "three"
        );
    }

    #[test]
    fn recovery_gc_repairs_and_verifies_retained_mirror_refs_before_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        let mut points = Vec::new();
        for value in ["one", "two", "three"] {
            fs::write(source.join("file.txt"), value).unwrap();
            snapshot_repository(&source, value.into(), None).unwrap();
            points.push(
                capture_recovery_point(&source, "HEAD", Some(&primary))
                    .unwrap()
                    .recovery_point
                    .recovery_point_id,
            );
        }
        update_recovery_retention(
            Some(&primary),
            RecoveryRetentionPolicy {
                minimum_points_per_repository: 1,
                minimum_retention_days: 0,
                maximum_points_per_repository: Some(1),
                ..RecoveryRetentionPolicy::default()
            },
        )
        .unwrap();

        let retained_ref = vault_repository_root(&mirror, initialized.repository_id)
            .unwrap()
            .join(".hig/repository/refs/tags/recovery")
            .join(&points[2]);
        fs::remove_file(&retained_ref).unwrap();
        assert!(
            verify_recovery_point(
                Some(&mirror),
                &hex::encode(initialized.repository_id),
                &points[2]
            )
            .is_err()
        );

        let report = gc_recovery_vault(Some(&primary), false).unwrap();
        assert_eq!(report.removed_recovery_points, 2);
        verify_recovery_point(
            Some(&mirror),
            &hex::encode(initialized.repository_id),
            &points[2],
        )
        .unwrap();

        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&primary).unwrap();
        let output = temp.path().join("restored");
        restore_recovery_point(
            Some(&mirror),
            &hex::encode(initialized.repository_id),
            &points[2],
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(output.join("file.txt")).unwrap(),
            "three"
        );
    }

    #[test]
    fn recovery_documents_and_tombstones_enforce_resource_limits() {
        let temp = tempfile::tempdir().unwrap();
        let oversized = temp.path().join("oversized.json");
        let file = File::create(&oversized).unwrap();
        file.set_len(MAX_RECOVERY_DOCUMENT_BYTES + 1).unwrap();
        let error = read_checked_json::<RecoveryCatalog>(&oversized).unwrap_err();
        assert!(error.to_string().contains("resource limit"));

        let reason = "x".repeat(MAX_RECOVERY_TEXT_BYTES + 1);
        let error = record_recovery_tombstone(
            Some(temp.path()),
            "00000000000000000000000000000000",
            RecoveryTombstoneKind::Workspace,
            None,
            None,
            reason,
        )
        .unwrap_err();
        assert!(error.to_string().contains("resource limit"));

        let path = "x".repeat(MAX_RECOVERY_PATH_BYTES + 1);
        let error = validate_relative_label(&path).unwrap_err();
        assert!(error.to_string().contains("resource limit"));
    }

    #[test]
    fn pinned_point_can_make_an_aggressive_limit_explicitly_unsatisfied() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&source).unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        let mut points = Vec::new();
        for value in ["one", "two", "three"] {
            fs::write(source.join("file.txt"), value).unwrap();
            snapshot_repository(&source, value.into(), None).unwrap();
            points.push(
                capture_recovery_point(&source, "HEAD", Some(&vault))
                    .unwrap()
                    .recovery_point
                    .recovery_point_id,
            );
        }
        set_recovery_point_pin(
            Some(&vault),
            &hex::encode(initialized.repository_id),
            &points[0],
            true,
        )
        .unwrap();
        update_recovery_retention(
            Some(&vault),
            RecoveryRetentionPolicy {
                minimum_points_per_repository: 1,
                minimum_retention_days: 0,
                maximum_points_per_repository: Some(1),
                ..RecoveryRetentionPolicy::default()
            },
        )
        .unwrap();

        let preview = gc_recovery_vault(Some(&vault), true).unwrap();
        assert_eq!(preview.candidate_recovery_points, 1);
        assert!(!preview.policy_satisfied);
        gc_recovery_vault(Some(&vault), false).unwrap();
        let remaining = list_recovery_vault(Some(&vault)).unwrap();
        assert_eq!(remaining.repositories[0].recovery_points.len(), 2);
        assert!(remaining.repositories[0].recovery_points[&points[0]].pinned);
        assert!(
            remaining.repositories[0]
                .recovery_points
                .contains_key(&points[2])
        );
    }

    #[test]
    fn scrub_detects_corruption_and_repair_uses_only_a_verified_mirror() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.bin"), b"repair-me\0exact").unwrap();
        let initialized = init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        let capture = capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        let repository_id = hex::encode(initialized.repository_id);
        let objects = vault_repository_root(&primary, initialized.repository_id)
            .unwrap()
            .join(".hig/repository/objects");
        let object = walkdir::WalkDir::new(&objects)
            .into_iter()
            .filter_map(Result::ok)
            .find(|entry| entry.file_type().is_file())
            .unwrap()
            .into_path();
        fs::write(&object, b"corrupt").unwrap();

        let damaged = scrub_recovery_vault(Some(&primary)).unwrap();
        assert!(!damaged.healthy);
        assert!(!damaged.locations[0].healthy);
        assert!(damaged.locations[1].healthy);
        let repaired = repair_recovery_point(
            Some(&primary),
            &repository_id,
            &capture.recovery_point.recovery_point_id,
            Some(&mirror),
        )
        .unwrap();
        assert_eq!(repaired.objects_repaired, 1);
        assert!(repaired.verified);
        assert!(scrub_recovery_vault(Some(&primary)).unwrap().healthy);

        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&mirror).unwrap();
        let output = temp.path().join("restored");
        restore_recovery_point(
            Some(&primary),
            &repository_id,
            &capture.recovery_point.recovery_point_id,
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("file.bin")).unwrap(),
            b"repair-me\0exact"
        );
    }

    #[test]
    fn scrub_reports_an_offline_mirror_without_hiding_primary_health() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let primary = temp.path().join("primary");
        let mirror = temp.path().join("mirror");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "source").unwrap();
        init_repository(&source, Vec::new()).unwrap();
        snapshot_repository(&source, "baseline".into(), None).unwrap();
        init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
        capture_recovery_point(&source, "HEAD", Some(&primary)).unwrap();
        fs::remove_dir_all(&mirror).unwrap();

        let scrub = scrub_recovery_vault(Some(&primary)).unwrap();
        assert!(!scrub.healthy);
        assert!(scrub.locations[0].healthy);
        assert!(!scrub.locations[1].healthy);
        assert!(!scrub.locations[1].errors.is_empty());
    }
}
