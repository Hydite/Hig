use crate::cli::{
    RecoveryAuthCommand, RecoveryCommand, RecoveryPolicyCommand, RecoveryTombstoneKindArg,
};
use hig_core::{
    RecoveryTombstoneKind, capture_recovery_point, export_recovery_auth_custody, gc_recovery_vault,
    import_recovery_auth_custody, init_recovery_vault, list_recovery_vault, migrate_recovery_auth,
    promote_recovery_vault, record_recovery_tombstone, recovery_audit_log, recovery_vault_config,
    recovery_vault_status, register_recovery_repository, repair_recovery_point,
    restore_recovery_point, rotate_recovery_auth_key, scrub_recovery_vault, set_recovery_point_pin,
    update_recovery_retention, verify_recovery_point,
};

pub(crate) fn handle(command: RecoveryCommand) -> anyhow::Result<()> {
    match command {
        RecoveryCommand::Init {
            vault_root,
            mirrors,
            json,
        } => {
            let report = init_recovery_vault(vault_root.as_deref(), mirrors)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: {} root={} mirrors={}",
                    if report.created {
                        "initialized"
                    } else {
                        "existing"
                    },
                    report.vault_root,
                    report.mirror_roots.len()
                );
            }
        }
        RecoveryCommand::Register {
            dir,
            vault_root,
            json,
        } => {
            let report = register_recovery_repository(&dir, vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: registered repository_id={} registration_id={} source={} created={}",
                    hex::encode(report.repository_id),
                    hex::encode(report.registration_id),
                    report.source_root,
                    report.created
                );
            }
        }
        RecoveryCommand::Capture {
            dir,
            revision,
            vault_root,
            json,
        } => {
            let report = capture_recovery_point(&dir, &revision, vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: captured repository_id={} point={} commit={} durability={:?} objects={} written={} bytes={} created={}",
                    hex::encode(report.repository_id),
                    report.recovery_point.recovery_point_id,
                    report.recovery_point.commit_id,
                    report.recovery_point.durability,
                    report.recovery_point.reachable_objects,
                    report.recovery_point.stored_objects_written,
                    report.recovery_point.stored_bytes_written,
                    report.created
                );
            }
        }
        RecoveryCommand::List { vault_root, json } => {
            let report = list_recovery_vault(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: vault={} generation={} repositories={}",
                    report.vault_root,
                    report.generation,
                    report.repositories.len()
                );
                for registration in report.repositories {
                    println!(
                        "{}\tpoints={}\tsources={}",
                        hex::encode(registration.repository_id),
                        registration.recovery_points.len(),
                        registration.source_paths.join(",")
                    );
                }
            }
        }
        RecoveryCommand::Status { vault_root, json } => {
            let report = recovery_vault_status(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: status vault={} generation={} repositories={} points={} available={} pending={} captured={} protected={} degraded={} durability_lag={} rpo_lag_ms={} mirrors={} incomplete_audit={}",
                    report.vault_root,
                    report.generation,
                    report.repositories,
                    report.recovery_points,
                    report.available_points,
                    report.pending_deletion_points,
                    report.captured_points,
                    report.protected_points,
                    report.degraded_points,
                    report.durability_lag_points,
                    report
                        .rpo_lag_millis
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    report.configured_mirrors,
                    report.incomplete_audit_operations
                );
            }
        }
        RecoveryCommand::Promote {
            vault_root,
            mirrors,
            json,
        } => {
            let report = promote_recovery_vault(vault_root.as_deref(), mirrors)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: promoted vault={} generation={}->{} changed={} repositories={} points={} mirrors={} written={} bytes={} durability={:?}",
                    report.vault_root,
                    report.generation_before,
                    report.generation_after,
                    report.changed,
                    report.repositories,
                    report.recovery_points,
                    report.mirror_roots.len(),
                    report.objects_written,
                    report.object_bytes_written,
                    report.durability
                );
            }
        }
        RecoveryCommand::Audit { vault_root, json } => {
            let report = recovery_audit_log(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: audit vault={} events={} incomplete={}",
                    report.vault_root,
                    report.events.len(),
                    report.incomplete_operation_ids.len()
                );
                for operation_id in report.incomplete_operation_ids {
                    println!("incomplete\t{operation_id}");
                }
            }
        }
        RecoveryCommand::Auth { command } => handle_auth(command)?,
        RecoveryCommand::MigrateAuth { vault_root, json } => {
            let report = migrate_recovery_auth(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: authentication migrated vault={} created={} lineage={} vault_id={} key_id={} mirrors={} repositories={} points={} objects={} bytes={} audit_events={}",
                    report.vault_root,
                    report.created,
                    report.lineage_id,
                    report.vault_id,
                    report.key_id,
                    report.migrated_mirrors.len(),
                    report.verified_repositories,
                    report.verified_recovery_points,
                    report.verified_objects,
                    report.verified_raw_bytes,
                    report.verified_audit_events
                );
            }
        }
        RecoveryCommand::Pin {
            repository_id,
            recovery_point_id,
            vault_root,
            json,
        } => print_pin(
            set_recovery_point_pin(
                vault_root.as_deref(),
                &repository_id,
                &recovery_point_id,
                true,
            )?,
            json,
        )?,
        RecoveryCommand::Unpin {
            repository_id,
            recovery_point_id,
            vault_root,
            json,
        } => print_pin(
            set_recovery_point_pin(
                vault_root.as_deref(),
                &repository_id,
                &recovery_point_id,
                false,
            )?,
            json,
        )?,
        RecoveryCommand::Tombstone {
            repository_id,
            kind,
            source_path,
            path,
            reason,
            vault_root,
            json,
        } => {
            let kind = match kind {
                RecoveryTombstoneKindArg::File => RecoveryTombstoneKind::File,
                RecoveryTombstoneKindArg::Workspace => RecoveryTombstoneKind::Workspace,
                RecoveryTombstoneKindArg::Registration => RecoveryTombstoneKind::Registration,
            };
            let report = record_recovery_tombstone(
                vault_root.as_deref(),
                &repository_id,
                kind,
                source_path,
                path,
                reason,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: tombstone repository_id={} tombstone_id={} kind={:?} observed_ns={}",
                    hex::encode(report.repository_id),
                    hex::encode(report.tombstone.tombstone_id),
                    report.tombstone.kind,
                    report.tombstone.observed_unix_ns
                );
            }
        }
        RecoveryCommand::Policy { command } => handle_policy(command)?,
        RecoveryCommand::Gc {
            vault_root,
            apply,
            json,
        } => {
            let report = gc_recovery_vault(vault_root.as_deref(), !apply)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: gc dry_run={} total_points={} candidates={} removed={} bytes_before={} projected_bytes={} policy_satisfied={}",
                    report.dry_run,
                    report.total_recovery_points,
                    report.candidate_recovery_points,
                    report.removed_recovery_points,
                    report.stored_bytes_before,
                    report.projected_stored_bytes,
                    report.policy_satisfied
                );
            }
        }
        RecoveryCommand::Scrub { vault_root, json } => {
            let report = scrub_recovery_vault(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: scrub healthy={} locations={}",
                    report.healthy,
                    report.locations.len()
                );
                for location in &report.locations {
                    println!(
                        "{}\tprimary={}\thealthy={}\trepositories={}\tpoints={}\tobjects={}\taudit_events={}\tincomplete_audit={}\terrors={}",
                        location.vault_root,
                        location.primary,
                        location.healthy,
                        location.checked_repositories,
                        location.checked_recovery_points,
                        location.checked_objects,
                        location.checked_audit_events,
                        location.incomplete_audit_operations,
                        location.errors.join(" | ")
                    );
                }
            }
            anyhow::ensure!(
                report.healthy,
                "Recovery Vault scrub detected corruption or an unavailable replica"
            );
        }
        RecoveryCommand::Repair {
            repository_id,
            recovery_point_id,
            mirror,
            vault_root,
            json,
        } => {
            let report = repair_recovery_point(
                vault_root.as_deref(),
                &repository_id,
                &recovery_point_id,
                mirror.as_deref(),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: repaired repository_id={} point={} mirror={} written={} repaired={} bytes={} verified={}",
                    hex::encode(report.repository_id),
                    report.recovery_point_id,
                    report.mirror_root,
                    report.objects_written,
                    report.objects_repaired,
                    report.object_bytes_written,
                    report.verified
                );
            }
        }
        RecoveryCommand::Verify {
            repository_id,
            recovery_point_id,
            vault_root,
            json,
        } => {
            let report =
                verify_recovery_point(vault_root.as_deref(), &repository_id, &recovery_point_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: verified repository_id={} point={} objects={} bytes={}",
                    hex::encode(report.repository_id),
                    report.recovery_point_id,
                    report.repository.checked_objects,
                    report.repository.checked_raw_bytes
                );
            }
        }
        RecoveryCommand::Restore {
            repository_id,
            recovery_point_id,
            output_dir,
            path,
            overwrite,
            vault_root,
            json,
        } => {
            let report = restore_recovery_point(
                vault_root.as_deref(),
                &repository_id,
                &recovery_point_id,
                &output_dir,
                path.as_deref(),
                overwrite,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: restored repository_id={} point={} files={} bytes={} output={}",
                    hex::encode(report.repository_id),
                    report.recovery_point_id,
                    report.restore.files,
                    report.restore.bytes,
                    report.restore.output_dir
                );
            }
        }
    }
    Ok(())
}

fn handle_auth(command: RecoveryAuthCommand) -> anyhow::Result<()> {
    let (report, action) = match command {
        RecoveryAuthCommand::Export {
            vault_root,
            output,
            json,
        } => {
            let report = export_recovery_auth_custody(vault_root.as_deref(), &output)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            (report, "exported")
        }
        RecoveryAuthCommand::Import {
            vault_root,
            input,
            json,
        } => {
            let report = import_recovery_auth_custody(vault_root.as_deref(), &input)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            (report, "imported")
        }
        RecoveryAuthCommand::Rotate { vault_root, json } => {
            let report = rotate_recovery_auth_key(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "recovery: authentication rotated vault={} lineage={} key_id={} previous_keys={} vaults={} old_keys_retained={}",
                    report.vault_root,
                    report.lineage_id,
                    report.key_id,
                    report.previous_key_ids.join(","),
                    report.rotated_vaults.len(),
                    report.old_keys_retained
                );
            }
            return Ok(());
        }
    };
    println!(
        "recovery: custody {action} vault={} file={} lineage={} vault_id={} key_id={} checkpoint={}",
        report.vault_root,
        report.custody_file,
        report.lineage_id,
        report.vault_id,
        report.key_id,
        report.checkpoint_sequence
    );
    Ok(())
}

fn print_pin(report: hig_core::RecoveryPinReport, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "recovery: pin repository_id={} point={} pinned={} changed={}",
            hex::encode(report.repository_id),
            report.recovery_point_id,
            report.pinned,
            report.changed
        );
    }
    Ok(())
}

fn handle_policy(command: RecoveryPolicyCommand) -> anyhow::Result<()> {
    match command {
        RecoveryPolicyCommand::Show { vault_root, json } => {
            let config = recovery_vault_config(vault_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                print_policy(&config);
            }
        }
        RecoveryPolicyCommand::Set {
            vault_root,
            minimum_points,
            minimum_retention_days,
            maximum_points,
            maximum_vault_bytes,
            clear_maximum_points,
            clear_maximum_vault_bytes,
            json,
        } => {
            anyhow::ensure!(
                !(maximum_points.is_some() && clear_maximum_points),
                "--maximum-points conflicts with --clear-maximum-points"
            );
            anyhow::ensure!(
                !(maximum_vault_bytes.is_some() && clear_maximum_vault_bytes),
                "--maximum-vault-bytes conflicts with --clear-maximum-vault-bytes"
            );
            let mut policy = recovery_vault_config(vault_root.as_deref())?.retention;
            if let Some(value) = minimum_points {
                policy.minimum_points_per_repository = value;
            }
            if let Some(value) = minimum_retention_days {
                policy.minimum_retention_days = value;
            }
            if let Some(value) = maximum_points {
                policy.maximum_points_per_repository = Some(value);
            } else if clear_maximum_points {
                policy.maximum_points_per_repository = None;
            }
            if let Some(value) = maximum_vault_bytes {
                policy.maximum_vault_bytes = Some(value);
            } else if clear_maximum_vault_bytes {
                policy.maximum_vault_bytes = None;
            }
            let config = update_recovery_retention(vault_root.as_deref(), policy)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                print_policy(&config);
            }
        }
    }
    Ok(())
}

fn print_policy(config: &hig_core::RecoveryVaultConfig) {
    let policy = &config.retention;
    println!(
        "recovery: policy minimum_points={} minimum_days={} maximum_points={} maximum_bytes={} at_rest={}",
        policy.minimum_points_per_repository,
        policy.minimum_retention_days,
        policy
            .maximum_points_per_repository
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".into()),
        policy
            .maximum_vault_bytes
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".into()),
        config.at_rest_policy.as_str()
    );
}
