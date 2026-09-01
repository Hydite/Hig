use super::*;
use crate::{init_repository, snapshot_repository};
use std::sync::{Arc, Barrier};

#[test]
fn concurrent_first_capture_has_one_creator_and_one_protected_point() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let primary = temp.path().join("primary");
    let mirror = temp.path().join("mirror");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("file.txt"), b"concurrent\0capture\xff").unwrap();
    let initialized = init_repository(&source, Vec::new()).unwrap();
    snapshot_repository(&source, "concurrent baseline".into(), None).unwrap();
    init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();

    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let handles = (0..workers)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let source = source.clone();
            let primary = primary.clone();
            std::thread::spawn(move || {
                barrier.wait();
                capture_recovery_point(&source, "HEAD", Some(&primary))
            })
        })
        .collect::<Vec<_>>();
    let reports = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(reports.iter().filter(|report| report.created).count(), 1);
    assert!(reports.iter().all(|report| {
        report.recovery_point.durability == RecoveryDurability::Protected
            && report.recovery_point.recovery_point_id
                == reports[0].recovery_point.recovery_point_id
    }));
    let listed = list_recovery_vault(Some(&primary)).unwrap();
    assert_eq!(listed.repositories.len(), 1);
    assert_eq!(listed.repositories[0].recovery_points.len(), 1);
    let repository_id = hex::encode(initialized.repository_id);
    let point_id = &reports[0].recovery_point.recovery_point_id;
    verify_recovery_point(Some(&primary), &repository_id, point_id).unwrap();
    verify_recovery_point(Some(&mirror), &repository_id, point_id).unwrap();
    assert!(
        recovery_audit_log(Some(&primary))
            .unwrap()
            .incomplete_operation_ids
            .is_empty()
    );
}

#[test]
fn restore_and_gc_race_has_only_atomic_legal_outcomes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let primary = temp.path().join("primary");
    let mirror = temp.path().join("mirror");
    fs::create_dir_all(&source).unwrap();
    let initialized = init_repository(&source, Vec::new()).unwrap();
    init_recovery_vault(Some(&primary), vec![mirror.clone()]).unwrap();
    let mut points = Vec::new();
    for value in [
        b"one\0exact\xff".as_slice(),
        b"two\0exact\xff",
        b"three\0exact\xff",
    ] {
        fs::write(source.join("file.txt"), value).unwrap();
        snapshot_repository(&source, "concurrent GC revision".into(), None).unwrap();
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
    fs::remove_dir_all(&source).unwrap();

    let repository_id = hex::encode(initialized.repository_id);
    let oldest_output = temp.path().join("oldest-output");
    let latest_output = temp.path().join("latest-output");
    let barrier = Arc::new(Barrier::new(3));
    let gc = {
        let barrier = Arc::clone(&barrier);
        let primary = primary.clone();
        std::thread::spawn(move || {
            barrier.wait();
            gc_recovery_vault(Some(&primary), false)
        })
    };
    let oldest = {
        let barrier = Arc::clone(&barrier);
        let primary = primary.clone();
        let repository_id = repository_id.clone();
        let point_id = points[0].clone();
        let output = oldest_output.clone();
        std::thread::spawn(move || {
            barrier.wait();
            restore_recovery_point(
                Some(&primary),
                &repository_id,
                &point_id,
                &output,
                None,
                false,
            )
        })
    };
    let latest = {
        let barrier = Arc::clone(&barrier);
        let primary = primary.clone();
        let repository_id = repository_id.clone();
        let point_id = points[2].clone();
        let output = latest_output.clone();
        std::thread::spawn(move || {
            barrier.wait();
            restore_recovery_point(
                Some(&primary),
                &repository_id,
                &point_id,
                &output,
                None,
                false,
            )
        })
    };

    let gc = gc.join().unwrap().unwrap();
    assert_eq!(gc.removed_recovery_points, 2);
    match oldest.join().unwrap() {
        Ok(_) => assert_eq!(
            fs::read(oldest_output.join("file.txt")).unwrap(),
            b"one\0exact\xff"
        ),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("pending deletion")
                    || message.contains("recovery point not found"),
                "unexpected restore/GC race failure: {message}"
            );
            assert!(!oldest_output.exists());
        }
    }
    latest.join().unwrap().unwrap();
    assert_eq!(
        fs::read(latest_output.join("file.txt")).unwrap(),
        b"three\0exact\xff"
    );

    let listed = list_recovery_vault(Some(&primary)).unwrap();
    assert_eq!(listed.repositories[0].recovery_points.len(), 1);
    assert!(
        listed.repositories[0]
            .recovery_points
            .contains_key(&points[2])
    );
    verify_recovery_point(Some(&primary), &repository_id, &points[2]).unwrap();
    verify_recovery_point(Some(&mirror), &repository_id, &points[2]).unwrap();
    assert!(scrub_recovery_vault(Some(&primary)).unwrap().healthy);
}
