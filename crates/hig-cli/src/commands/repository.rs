use crate::cli::{RepositoryBranchCommand, RepositoryCommand, RepositoryTagCommand};
use hig_core::{
    RepositoryObjectId, RepositoryWatcher, create_repository_branch, create_repository_tag,
    delete_repository_branch, delete_repository_tag, gc_repository, init_repository,
    migrate_repository, repository_branch_names, repository_diff, repository_log,
    repository_path_history, repository_refs, repository_storage_tree, repository_symbol_history,
    repository_symbols, restore_repository, restore_repository_range, restore_repository_symbol,
    snapshot_repository, switch_repository_branch, verify_repository,
};
use std::time::Duration;

pub(crate) fn handle(command: RepositoryCommand) -> anyhow::Result<()> {
    match command {
        RepositoryCommand::Init {
            dir,
            excludes,
            json,
        } => {
            let report = init_repository(&dir, excludes)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: {} root={} repository_id={}",
                    if report.created {
                        "initialized"
                    } else {
                        "existing"
                    },
                    report.root,
                    hex::encode(report.repository_id)
                );
            }
        }
        RepositoryCommand::Snapshot {
            dir,
            message,
            author,
            json,
        } => {
            let report = snapshot_repository(&dir, message, author)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: {} commit={} parent={} files={} bytes={} objects_written={} chunks_written={} chunks_reused={}",
                    if report.created {
                        "snapshot"
                    } else {
                        "unchanged"
                    },
                    short_id(report.commit_id),
                    report
                        .parent_id
                        .map(short_id)
                        .unwrap_or_else(|| "none".to_string()),
                    report.files,
                    report.input_bytes,
                    report.objects_written,
                    report.chunks_written,
                    report.chunks_reused
                );
            }
        }
        RepositoryCommand::Refs { dir, json } => {
            let report = repository_refs(&dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: refs head={} active_branch={}",
                    report
                        .head
                        .map(short_id)
                        .unwrap_or_else(|| "none".to_string()),
                    report.active_branch.as_deref().unwrap_or("none")
                );
                for reference in report.refs {
                    println!(
                        "{:?}\t{}\t{}\tactive={}",
                        reference.kind,
                        reference.name,
                        short_id(reference.commit_id),
                        reference.active
                    );
                }
            }
        }
        RepositoryCommand::Migrate { dir, json } => {
            let report = migrate_repository(&dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: migrated root={} active_branch={} commit={} changed={} objects_rewritten={}",
                    report.root,
                    report.active_branch,
                    report
                        .commit_id
                        .map(short_id)
                        .unwrap_or_else(|| "none".to_string()),
                    report.changed,
                    report.objects_rewritten
                );
            }
        }
        RepositoryCommand::Branch { command } => handle_branch(command)?,
        RepositoryCommand::Tag { command } => handle_tag(command)?,
        RepositoryCommand::Log { dir, limit, json } => {
            anyhow::ensure!(limit > 0, "--limit must be greater than zero");
            let commits = repository_log(&dir, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&commits)?);
            } else {
                for commit in commits {
                    println!(
                        "commit {} parent={} time_ns={} files={} bytes={} message={}",
                        short_id(commit.commit_id),
                        commit
                            .parent_id
                            .map(short_id)
                            .unwrap_or_else(|| "none".to_string()),
                        commit.created_unix_ns,
                        commit.files,
                        commit.input_bytes,
                        commit.message
                    );
                }
            }
        }
        RepositoryCommand::Diff {
            dir,
            from,
            to,
            json,
        } => {
            let report = repository_diff(&dir, from.as_deref(), to.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: diff from={} to={} added={} deleted={} modified={} metadata={} renamed={}",
                    report
                        .from
                        .map(short_id)
                        .unwrap_or_else(|| "empty".to_string()),
                    short_id(report.to),
                    report.added,
                    report.deleted,
                    report.modified,
                    report.metadata,
                    report.renamed
                );
                for change in report.changes {
                    println!(
                        "{:?}\t{}\tranges={}",
                        change.kind,
                        change.path,
                        change.byte_ranges.len()
                    );
                }
            }
        }
        RepositoryCommand::Restore {
            dir,
            revision,
            output_dir,
            path,
            overwrite,
            json,
        } => {
            let report =
                restore_repository(&dir, &revision, &output_dir, path.as_deref(), overwrite)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: restored commit={} files={} bytes={} output={} path={}",
                    short_id(report.commit_id),
                    report.files,
                    report.bytes,
                    report.output_dir,
                    report.selected_path.as_deref().unwrap_or("all")
                );
            }
        }
        RepositoryCommand::RestoreRange {
            dir,
            revision,
            path,
            start,
            len,
            output,
            overwrite,
            json,
        } => {
            let report =
                restore_repository_range(&dir, &revision, &path, start, len, &output, overwrite)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: restored-range commit={} path={} start={} len={} output={}",
                    short_id(report.commit_id),
                    report.path,
                    report.start,
                    report.len,
                    report.output_file
                );
            }
        }
        RepositoryCommand::History {
            dir,
            path,
            limit,
            json,
        } => {
            let report = repository_path_history(&dir, &path, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: history head={} path={} entries={}",
                    short_id(report.head),
                    report.query_path,
                    report.entries.len()
                );
                for entry in report.entries {
                    println!(
                        "{:?}\t{}\t{}\tranges={}",
                        entry.kind,
                        short_id(entry.commit_id),
                        entry.path,
                        entry.byte_ranges.len()
                    );
                }
            }
        }
        RepositoryCommand::StorageTree {
            dir,
            revision,
            json,
        } => {
            let report = repository_storage_tree(&dir, &revision)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: storage-tree commit={} files={} raw_bytes={} chunks={} unique_chunks={} stored_object_bytes={}",
                    short_id(report.commit_id),
                    report.files,
                    report.raw_bytes,
                    report.chunks,
                    report.unique_chunks,
                    report.stored_object_bytes
                );
                for path in report.paths {
                    println!(
                        "{}\traw={}\tchunks={}\tunique={}\tstored={}",
                        path.path,
                        path.raw_bytes,
                        path.chunks,
                        path.unique_chunks,
                        path.stored_object_bytes
                    );
                }
            }
        }
        RepositoryCommand::Symbols {
            dir,
            revision,
            path,
            json,
        } => {
            let report = repository_symbols(&dir, &revision, path.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: symbols commit={} symbols={} parser_failures={}",
                    short_id(report.commit_id),
                    report.symbols.len(),
                    report.parser_failures.len()
                );
                for symbol in report.symbols {
                    println!(
                        "{}\t{}\t{}\t{}:{}-{}",
                        &symbol.symbol_id[..12],
                        symbol.kind,
                        symbol.qualified_name,
                        symbol.path,
                        symbol.start_byte,
                        symbol.end_byte
                    );
                }
            }
        }
        RepositoryCommand::SymbolHistory {
            dir,
            symbol,
            limit,
            json,
        } => {
            let report = repository_symbol_history(&dir, &symbol, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: symbol-history head={} symbol={} entries={}",
                    short_id(report.head),
                    &report.resolved_symbol_id[..12],
                    report.entries.len()
                );
                for entry in report.entries {
                    println!(
                        "{:?}\t{}\t{}\t{}",
                        entry.kind,
                        short_id(entry.commit_id),
                        entry.qualified_name,
                        entry.path
                    );
                }
            }
        }
        RepositoryCommand::RestoreSymbol {
            dir,
            revision,
            symbol,
            output,
            overwrite,
            json,
        } => {
            let report = restore_repository_symbol(&dir, &revision, &symbol, &output, overwrite)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: restored-symbol commit={} symbol={} name={} path={} bytes={} output={}",
                    short_id(report.commit_id),
                    &report.symbol_id[..12],
                    report.qualified_name,
                    report.path,
                    report.end_byte - report.start_byte,
                    report.output_file
                );
            }
        }
        RepositoryCommand::Watch {
            dir,
            debounce_ms,
            message,
            author,
            json,
        } => {
            anyhow::ensure!(debounce_ms > 0, "--debounce-ms must be greater than zero");
            let mut watcher = RepositoryWatcher::start(&dir, Duration::from_millis(debounce_ms))?;
            if !json {
                println!(
                    "repo: watch active root={} debounce_ms={}",
                    watcher.root().display(),
                    debounce_ms
                );
            }
            loop {
                if let Some(report) = watcher.poll(&message, author.as_deref())?
                    && report.created
                {
                    if json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        println!(
                            "repo: automatic-snapshot commit={} files={} bytes={} chunks_written={} chunks_reused={}",
                            short_id(report.commit_id),
                            report.files,
                            report.input_bytes,
                            report.chunks_written,
                            report.chunks_reused
                        );
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        RepositoryCommand::Verify { dir, json } => {
            let report = verify_repository(&dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: verified refs={} commits={} trees={} files={} chunks={} change_indexes={} semantic_indexes={} compression_tree_indexes={} objects={} raw_bytes={}",
                    report.refs,
                    report.commits,
                    report.trees,
                    report.files,
                    report.chunks,
                    report.change_indexes,
                    report.semantic_indexes,
                    report.compression_tree_indexes,
                    report.checked_objects,
                    report.checked_raw_bytes
                );
            }
        }
        RepositoryCommand::Gc { dir, apply, json } => {
            let report = gc_repository(&dir, !apply)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: gc dry_run={} total={} reachable={} unreachable={} unreachable_bytes={} removed={} removed_bytes={} temporary_files={} temporary_bytes={} removed_temporary_files={} removed_temporary_bytes={}",
                    report.dry_run,
                    report.total_objects,
                    report.reachable_objects,
                    report.unreachable_objects,
                    report.unreachable_bytes,
                    report.removed_objects,
                    report.removed_bytes,
                    report.temporary_files,
                    report.temporary_bytes,
                    report.removed_temporary_files,
                    report.removed_temporary_bytes
                );
            }
        }
    }
    Ok(())
}

fn short_id(id: RepositoryObjectId) -> String {
    id.to_hex()[..12].to_string()
}

fn handle_branch(command: RepositoryBranchCommand) -> anyhow::Result<()> {
    match command {
        RepositoryBranchCommand::List { dir, json } => {
            let report = repository_branch_names(&dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                for reference in report {
                    println!(
                        "{}\t{}\tactive={}",
                        reference.name,
                        short_id(reference.commit_id),
                        reference.active
                    );
                }
            }
        }
        RepositoryBranchCommand::Create {
            name,
            dir,
            from_revision,
            json,
        } => {
            let report = create_repository_branch(&dir, &name, from_revision.as_deref())?;
            print_ref_report(&report, json)?;
        }
        RepositoryBranchCommand::Switch { name, dir, json } => {
            let report = switch_repository_branch(&dir, &name)?;
            print_ref_report(&report, json)?;
        }
        RepositoryBranchCommand::Delete { name, dir, json } => {
            let report = delete_repository_branch(&dir, &name)?;
            print_delete_report(&report, json)?;
        }
    }
    Ok(())
}

fn handle_tag(command: RepositoryTagCommand) -> anyhow::Result<()> {
    match command {
        RepositoryTagCommand::List { dir, json } => {
            let report = hig_core::repository_tag_names(&dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                for reference in report {
                    println!("{}\t{}", reference.name, short_id(reference.commit_id));
                }
            }
        }
        RepositoryTagCommand::Create {
            name,
            dir,
            from_revision,
            json,
        } => {
            let report = create_repository_tag(&dir, &name, from_revision.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "repo: tag name={} commit={} created={}",
                    report.name,
                    short_id(report.commit_id),
                    report.created
                );
            }
        }
        RepositoryTagCommand::Delete { name, dir, json } => {
            let report = delete_repository_tag(&dir, &name)?;
            print_delete_report(&report, json)?;
        }
    }
    Ok(())
}

fn print_ref_report(report: &hig_core::RepositoryBranchReport, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!(
            "repo: branch name={} commit={} active={} created={}",
            report.name,
            short_id(report.commit_id),
            report.active,
            report.created
        );
    }
    Ok(())
}

fn print_delete_report(
    report: &hig_core::RepositoryRefDeleteReport,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!(
            "repo: deleted {:?} name={} deleted={}",
            report.kind, report.name, report.deleted
        );
    }
    Ok(())
}
