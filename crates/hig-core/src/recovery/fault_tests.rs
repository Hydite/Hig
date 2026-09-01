use super::*;
use crate::{init_repository, snapshot_repository};

fn recovery_source_fixture(
    with_mirror: bool,
) -> (tempfile::TempDir, PathBuf, PathBuf, Option<PathBuf>, String) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let vault = temp.path().join("vault");
    let mirror = with_mirror.then(|| temp.path().join("mirror"));
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("file.txt"), b"baseline\0exact\xff").unwrap();
    let initialized = init_repository(&source, Vec::new()).unwrap();
    snapshot_repository(&source, "baseline".into(), Some("fault-test".into())).unwrap();
    init_recovery_vault(Some(&vault), mirror.iter().cloned().collect::<Vec<_>>()).unwrap();
    (
        temp,
        source,
        vault,
        mirror,
        hex::encode(initialized.repository_id),
    )
}

fn assert_failed_audit(vault: &Path, operation: RecoveryAuditOperation, failpoint: &str) {
    let audit = recovery_audit_log(Some(vault)).unwrap();
    assert!(audit.incomplete_operation_ids.is_empty());
    assert!(audit.events.iter().any(|event| {
        event.operation == operation
            && event.outcome == RecoveryAuditOutcome::Failed
            && event
                .error
                .as_deref()
                .is_some_and(|error| error.contains(failpoint))
    }));
}

fn assert_exact_fault_fixture_restore(
    vault: &Path,
    repository_id: &str,
    point_id: &str,
    output: &Path,
) {
    restore_recovery_point(Some(vault), repository_id, point_id, output, None, true).unwrap();
    assert_eq!(
        fs::read(output.join("file.txt")).unwrap(),
        b"baseline\0exact\xff"
    );
}

fn recovery_gc_fixture() -> (
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    PathBuf,
    String,
    Vec<String>,
) {
    let (temp, source, vault, mirror, repository_id) = recovery_source_fixture(true);
    let mirror = mirror.unwrap();
    let mut points = vec![
        capture_recovery_point(&source, "HEAD", Some(&vault))
            .unwrap()
            .recovery_point
            .recovery_point_id,
    ];
    for value in [b"two\0exact\xff".as_slice(), b"three\0exact\xff".as_slice()] {
        fs::write(source.join("file.txt"), value).unwrap();
        snapshot_repository(&source, "gc fault revision".into(), None).unwrap();
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
    (temp, source, vault, mirror, repository_id, points)
}

#[test]
fn capture_faults_before_catalog_publication_are_invisible_and_retryable() {
    for failpoint in [
        "capture_after_prepared",
        "capture_after_primary_replication",
        "capture_before_catalog_publish",
    ] {
        let (temp, source, vault, _mirror, repository_id) = recovery_source_fixture(false);
        let error = with_recovery_failpoint(failpoint, || {
            capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap_err()
        });
        assert!(error.to_string().contains(failpoint));
        assert!(
            list_recovery_vault(Some(&vault))
                .unwrap()
                .repositories
                .is_empty()
        );
        assert_failed_audit(&vault, RecoveryAuditOperation::Capture, failpoint);

        let retried = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        assert!(retried.created);
        verify_recovery_point(
            Some(&vault),
            &repository_id,
            &retried.recovery_point.recovery_point_id,
        )
        .unwrap();
        fs::remove_dir_all(&source).unwrap();
        assert_exact_fault_fixture_restore(
            &vault,
            &repository_id,
            &retried.recovery_point.recovery_point_id,
            &temp.path().join("restored"),
        );
    }
}

#[test]
fn capture_fault_after_catalog_publication_preserves_committed_recovery_state() {
    let (temp, source, vault, _mirror, repository_id) = recovery_source_fixture(false);
    let point_id = repository_revision_id(&source, "HEAD").unwrap().to_hex();
    let error = with_recovery_failpoint("capture_after_catalog_publish", || {
        capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap_err()
    });
    assert!(error.to_string().contains("capture_after_catalog_publish"));
    assert_failed_audit(
        &vault,
        RecoveryAuditOperation::Capture,
        "capture_after_catalog_publish",
    );

    let listed = list_recovery_vault(Some(&vault)).unwrap();
    assert_eq!(listed.repositories.len(), 1);
    assert!(
        listed.repositories[0]
            .recovery_points
            .contains_key(&point_id)
    );
    verify_recovery_point(Some(&vault), &repository_id, &point_id).unwrap();
    let retried = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
    assert!(!retried.created);
    fs::remove_dir_all(&source).unwrap();
    assert_exact_fault_fixture_restore(
        &vault,
        &repository_id,
        &point_id,
        &temp.path().join("restored"),
    );
}

#[test]
fn mirror_capture_fault_is_reported_as_degraded_and_retry_recovers_protection() {
    let (temp, source, vault, mirror, repository_id) = recovery_source_fixture(true);
    let mirror = mirror.unwrap();
    let degraded = with_recovery_failpoint("capture_mirror_after_replication", || {
        capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap()
    });
    assert_eq!(
        degraded.recovery_point.durability,
        RecoveryDurability::Degraded
    );
    assert_eq!(degraded.recovery_point.replicas.len(), 1);
    assert!(!degraded.recovery_point.replicas[0].verified);
    assert!(
        degraded.recovery_point.replicas[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("capture_mirror_after_replication"))
    );
    assert_failed_audit(
        &mirror,
        RecoveryAuditOperation::MirrorSynchronize,
        "capture_mirror_after_replication",
    );

    let protected = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
    assert!(!protected.created);
    assert_eq!(
        protected.recovery_point.durability,
        RecoveryDurability::Protected
    );
    fs::remove_dir_all(&source).unwrap();
    fs::remove_dir_all(&vault).unwrap();
    assert_exact_fault_fixture_restore(
        &mirror,
        &repository_id,
        &protected.recovery_point.recovery_point_id,
        &temp.path().join("mirror-restored"),
    );
}

#[test]
fn restore_faults_preserve_atomic_publication_and_exact_retry() {
    for failpoint in [
        "restore_after_prepared",
        "restore_after_verification",
        "restore_after_publication",
    ] {
        let (temp, source, vault, _mirror, repository_id) = recovery_source_fixture(false);
        let captured = capture_recovery_point(&source, "HEAD", Some(&vault)).unwrap();
        fs::remove_dir_all(&source).unwrap();
        let output = temp.path().join("restored");
        let error = with_recovery_failpoint(failpoint, || {
            restore_recovery_point(
                Some(&vault),
                &repository_id,
                &captured.recovery_point.recovery_point_id,
                &output,
                None,
                false,
            )
            .unwrap_err()
        });
        assert!(error.to_string().contains(failpoint));
        assert_failed_audit(&vault, RecoveryAuditOperation::Restore, failpoint);

        if failpoint == "restore_after_publication" {
            assert_eq!(
                fs::read(output.join("file.txt")).unwrap(),
                b"baseline\0exact\xff"
            );
        } else {
            assert!(!output.exists());
        }
        assert_exact_fault_fixture_restore(
            &vault,
            &repository_id,
            &captured.recovery_point.recovery_point_id,
            &output,
        );
    }
}

#[test]
fn recovery_gc_faults_are_resumable_across_both_catalog_publications() {
    for failpoint in [
        "gc_after_prepared",
        "gc_after_pending_catalog",
        "gc_after_mirror_deletion",
        "gc_after_primary_deletion",
        "gc_before_final_catalog",
        "gc_after_final_catalog",
    ] {
        let (temp, source, vault, mirror, repository_id, points) = recovery_gc_fixture();
        let error = with_recovery_failpoint(failpoint, || {
            gc_recovery_vault(Some(&vault), false).unwrap_err()
        });
        assert!(error.to_string().contains(failpoint));
        assert_failed_audit(&vault, RecoveryAuditOperation::GarbageCollection, failpoint);

        let interrupted = list_recovery_vault(Some(&vault)).unwrap();
        let interrupted_points = &interrupted.repositories[0].recovery_points;
        if failpoint == "gc_after_prepared" {
            assert_eq!(interrupted_points.len(), 3);
            assert!(
                interrupted_points
                    .values()
                    .all(|point| point.state == RecoveryPointState::Available)
            );
        } else if failpoint == "gc_after_final_catalog" {
            assert_eq!(interrupted_points.len(), 1);
            assert!(interrupted_points.contains_key(&points[2]));
        } else {
            assert_eq!(interrupted_points.len(), 3);
            assert_eq!(
                interrupted_points
                    .values()
                    .filter(|point| point.state == RecoveryPointState::PendingDeletion)
                    .count(),
                2
            );
            let denied = restore_recovery_point(
                Some(&vault),
                &repository_id,
                &points[0],
                &temp.path().join("pending-restore"),
                None,
                false,
            )
            .unwrap_err();
            assert!(denied.to_string().contains("pending deletion"));
        }

        let resumed = gc_recovery_vault(Some(&vault), false).unwrap();
        if failpoint == "gc_after_final_catalog" {
            assert_eq!(resumed.removed_recovery_points, 0);
        } else {
            assert_eq!(resumed.removed_recovery_points, 2);
        }
        let final_state = list_recovery_vault(Some(&vault)).unwrap();
        assert_eq!(final_state.repositories[0].recovery_points.len(), 1);
        assert!(
            final_state.repositories[0]
                .recovery_points
                .contains_key(&points[2])
        );
        verify_recovery_point(Some(&vault), &repository_id, &points[2]).unwrap();
        verify_recovery_point(Some(&mirror), &repository_id, &points[2]).unwrap();

        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&vault).unwrap();
        let output = temp.path().join("mirror-restored");
        restore_recovery_point(
            Some(&mirror),
            &repository_id,
            &points[2],
            &output,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("file.txt")).unwrap(),
            b"three\0exact\xff"
        );
    }
}
