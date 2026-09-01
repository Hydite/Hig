use crate::cli::{RecoveryCommand, RecoveryPolicyCommand, RecoveryTombstoneKindArg};
use hig_core::{
    RecoveryTombstoneKind, capture_recovery_point, gc_recovery_vault, init_recovery_vault,
    list_recovery_vault, record_recovery_tombstone, recovery_audit_log, recovery_vault_config,
    register_recovery_repository, repair_recovery_point, restore_recovery_point,
    scrub_recovery_vault, set_recovery_point_pin, update_recovery_retention, verify_recovery_point,
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
