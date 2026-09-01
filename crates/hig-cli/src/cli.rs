use crate::commands;
use crate::runtime::{
    handle_cache, handle_daemon, handle_project, handle_project_init, handle_project_watch,
    handle_session, handle_task,
};

use clap::{Parser, Subcommand, ValueEnum};
use hig_core::KdfProfile;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum BenchSuite {
    Source,
    Lobehub,
    LobehubWatch,
    Small500,
    Textmix,
    Repeat4m,
    Random8m,
    Binarymix,
    All,
}

#[derive(Debug, Parser)]
#[command(
    name = "hig",
    version,
    about = "High-speed cached encrypted archive prototype"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportMode {
    Short,
    Verbose,
    Json,
    Quiet,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReportFlags {
    pub(crate) verbose: bool,
    pub(crate) json: bool,
    pub(crate) quiet: bool,
}

impl ReportFlags {
    pub(crate) fn mode(self) -> anyhow::Result<ReportMode> {
        let count = usize::from(self.verbose) + usize::from(self.json) + usize::from(self.quiet);
        anyhow::ensure!(
            count <= 1,
            "--verbose, --json, and --quiet are mutually exclusive"
        );
        Ok(if self.json {
            ReportMode::Json
        } else if self.verbose {
            ReportMode::Verbose
        } else if self.quiet {
            ReportMode::Quiet
        } else {
            ReportMode::Short
        })
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long = "exclude")]
        excludes: Vec<String>,
    },
    Watch {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
    },
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Repo {
        #[command(subcommand)]
        command: RepositoryCommand,
    },
    Recovery {
        #[command(subcommand)]
        command: RecoveryCommand,
    },
    Pack(commands::archive::PackArgs),
    Unpack(commands::archive::UnpackArgs),
    Inspect(commands::archive::InspectArgs),
    Migrate(commands::archive::MigrateArgs),
    Bench(commands::archive::BenchArgs),
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    Unlock {
        #[arg(long)]
        password: Option<String>,
        #[arg(long, conflicts_with = "password")]
        password_stdin: bool,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        ttl_secs: Option<u64>,
        #[arg(long, default_value = "secure")]
        kdf_profile: KdfProfile,
    },
    Status {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Clear {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DaemonCommand {
    Start {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        ttl_secs: Option<u64>,
    },
    Status {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Stop {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        cache_dir: PathBuf,
        #[arg(long)]
        ttl_secs: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CacheCommand {
    Status {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Gc {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Compact {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TaskCommand {
    List {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        include_completed: bool,
    },
    Status {
        task_id: String,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Cancel {
        task_id: String,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Result {
        task_id: String,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectCommand {
    Status {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Rebuild {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        wait: bool,
    },
    Policy {
        #[command(subcommand)]
        command: ProjectPolicyCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectPolicyCommand {
    Show {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Set {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        quiescence_ms: Option<u64>,
        #[arg(long)]
        periodic_interval_ms: Option<u64>,
        #[arg(long)]
        max_pending_events: Option<u64>,
        #[arg(long)]
        max_pending_files: Option<u64>,
        #[arg(long)]
        resource_enabled: Option<bool>,
        #[arg(long)]
        min_available_memory_bytes: Option<u64>,
        #[arg(long)]
        resume_available_memory_bytes: Option<u64>,
        #[arg(long)]
        resource_poll_interval_ms: Option<u64>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RepositoryCommand {
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    Snapshot {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(short, long, default_value = "snapshot")]
        message: String,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Refs {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Migrate {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Branch {
        #[command(subcommand)]
        command: RepositoryBranchCommand,
    },
    Tag {
        #[command(subcommand)]
        command: RepositoryTagCommand,
    },
    Log {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    Diff {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Restore {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[arg(short = 'd', long)]
        output_dir: PathBuf,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
    RestoreRange {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        start: u64,
        #[arg(long)]
        len: Option<u64>,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
    History {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        path: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    StorageTree {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[arg(long)]
        json: bool,
    },
    Symbols {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        json: bool,
    },
    SymbolHistory {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        symbol: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    RestoreSymbol {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[arg(long)]
        symbol: String,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
    Watch {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value_t = 750)]
        debounce_ms: u64,
        #[arg(short, long, default_value = "automatic snapshot")]
        message: String,
        #[arg(long)]
        author: Option<String>,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        catch_up: bool,
        #[arg(long, hide = true)]
        lifecycle_stdin: bool,
        #[arg(long)]
        recovery_vault: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Verify {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Gc {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, help = "Delete unreachable objects; default is report-only")]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RecoveryCommand {
    Init {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long = "mirror")]
        mirrors: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Register {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Capture {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Promote {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long = "mirror")]
        mirrors: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Audit {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Pin {
        repository_id: String,
        recovery_point_id: String,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Unpin {
        repository_id: String,
        recovery_point_id: String,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Tombstone {
        repository_id: String,
        #[arg(long)]
        kind: RecoveryTombstoneKindArg,
        #[arg(long)]
        source_path: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Policy {
        #[command(subcommand)]
        command: RecoveryPolicyCommand,
    },
    Gc {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(
            long,
            help = "Apply eligible recovery-point and object deletion; default is report-only"
        )]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    Scrub {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Repair {
        repository_id: String,
        recovery_point_id: String,
        #[arg(long)]
        mirror: Option<PathBuf>,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Verify {
        repository_id: String,
        recovery_point_id: String,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Restore {
        repository_id: String,
        recovery_point_id: String,
        #[arg(short = 'd', long)]
        output_dir: PathBuf,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum RecoveryTombstoneKindArg {
    File,
    Workspace,
    Registration,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RecoveryPolicyCommand {
    Show {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Set {
        #[arg(long)]
        vault_root: Option<PathBuf>,
        #[arg(long)]
        minimum_points: Option<u32>,
        #[arg(long)]
        minimum_retention_days: Option<u32>,
        #[arg(long)]
        maximum_points: Option<u32>,
        #[arg(long)]
        maximum_vault_bytes: Option<u64>,
        #[arg(long)]
        clear_maximum_points: bool,
        #[arg(long)]
        clear_maximum_vault_bytes: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RepositoryBranchCommand {
    List {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Create {
        name: String,
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long = "from")]
        from_revision: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Switch {
        name: String,
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Delete {
        name: String,
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RepositoryTagCommand {
    List {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Create {
        name: String,
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long = "from")]
        from_revision: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Delete {
        name: String,
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init {
            dir,
            cache_dir,
            excludes,
        } => handle_project_init(&dir, cache_dir, excludes)?,
        Command::Watch {
            dir,
            cache_dir,
            verbose,
        } => handle_project_watch(&dir, cache_dir, verbose)?,
        Command::Project { command } => handle_project(command)?,
        Command::Repo { command } => commands::repository::handle(command)?,
        Command::Recovery { command } => commands::recovery::handle(command)?,
        Command::Pack(args) => commands::archive::handle_pack(args)?,
        Command::Unpack(args) => commands::archive::handle_unpack(args)?,
        Command::Inspect(args) => commands::archive::handle_inspect(args)?,
        Command::Migrate(args) => commands::archive::handle_migrate(args)?,
        Command::Bench(args) => commands::archive::handle_bench(args)?,
        Command::Session { command } => handle_session(command)?,
        Command::Daemon { command } => handle_daemon(command)?,
        Command::Cache { command } => handle_cache(command)?,
        Command::Task { command } => handle_task(command)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        effective_kdf_profile, response_mode_for_report, validate_pack_encryption_args,
    };
    use hig_core::{EncryptionMode, PackResponseMode, PayloadMemoryMode, SpeedMode};
    use std::path::Path;

    #[test]
    fn encryption_arguments_are_strict() {
        assert!(validate_pack_encryption_args(EncryptionMode::Password, Some("pw"), false).is_ok());
        assert!(validate_pack_encryption_args(EncryptionMode::Password, None, false).is_err());
        assert!(validate_pack_encryption_args(EncryptionMode::Password, None, true).is_ok());
        assert!(validate_pack_encryption_args(EncryptionMode::None, None, false).is_ok());
        assert!(validate_pack_encryption_args(EncryptionMode::None, Some("pw"), false).is_err());
        assert!(validate_pack_encryption_args(EncryptionMode::None, None, true).is_err());
    }

    #[test]
    fn fastest_defaults_to_interactive_but_respects_secure() {
        assert_eq!(
            effective_kdf_profile(SpeedMode::Balanced, None),
            KdfProfile::Secure
        );
        assert_eq!(
            effective_kdf_profile(SpeedMode::Fastest, None),
            KdfProfile::Interactive
        );
        assert_eq!(
            effective_kdf_profile(SpeedMode::Fastest, Some(KdfProfile::Secure)),
            KdfProfile::Secure
        );
    }

    #[test]
    fn report_flags_are_mutually_exclusive() {
        assert_eq!(
            ReportFlags {
                verbose: false,
                json: false,
                quiet: false,
            }
            .mode()
            .unwrap(),
            ReportMode::Short
        );
        assert_eq!(
            ReportFlags {
                verbose: true,
                json: false,
                quiet: false,
            }
            .mode()
            .unwrap(),
            ReportMode::Verbose
        );
        assert!(
            ReportFlags {
                verbose: true,
                json: true,
                quiet: false,
            }
            .mode()
            .is_err()
        );
    }

    #[test]
    fn report_mode_selects_daemon_response_shape() {
        assert_eq!(
            response_mode_for_report(ReportMode::Short),
            PackResponseMode::Summary
        );
        assert_eq!(
            response_mode_for_report(ReportMode::Quiet),
            PackResponseMode::Summary
        );
        assert_eq!(
            response_mode_for_report(ReportMode::Verbose),
            PackResponseMode::Full
        );
        assert_eq!(
            response_mode_for_report(ReportMode::Json),
            PackResponseMode::Full
        );
    }

    #[test]
    fn inspect_command_parses_json_mode() {
        let cli = Cli::try_parse_from(["hig", "inspect", "archive.hig", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Inspect(args)
                if args.archive_file == Path::new("archive.hig")
                    && args.password.is_none()
                    && args.json
        ));
    }

    #[test]
    fn repository_snapshot_command_parses_history_metadata() {
        let cli = Cli::try_parse_from([
            "hig",
            "repo",
            "snapshot",
            "project",
            "--message",
            "micro change",
            "--author",
            "tester",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Repo {
                command: RepositoryCommand::Snapshot {
                    dir,
                    message,
                    author: Some(author),
                    json: true,
                }
            } if dir == Path::new("project") && message == "micro change" && author == "tester"
        ));
    }

    #[test]
    fn recovery_restore_requires_explicit_identity_and_destination() {
        let cli = Cli::try_parse_from([
            "hig",
            "recovery",
            "restore",
            "00112233445566778899aabbccddeeff",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "--output-dir",
            "restored",
            "--vault-root",
            "vault",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Recovery {
                command: RecoveryCommand::Restore {
                    repository_id,
                    recovery_point_id,
                    output_dir,
                    vault_root: Some(vault_root),
                    overwrite: false,
                    json: true,
                    ..
                }
            } if repository_id == "00112233445566778899aabbccddeeff"
                && recovery_point_id.len() == 64
                && output_dir == Path::new("restored")
                && vault_root == Path::new("vault")
        ));
    }

    #[test]
    fn recovery_scrub_and_repair_parse_in_the_recovery_namespace() {
        let status = Cli::try_parse_from([
            "hig",
            "recovery",
            "status",
            "--vault-root",
            "vault",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            status.command,
            Command::Recovery {
                command: RecoveryCommand::Status {
                    vault_root: Some(root),
                    json: true,
                }
            } if root == Path::new("vault")
        ));

        let promote = Cli::try_parse_from([
            "hig",
            "recovery",
            "promote",
            "--vault-root",
            "survivor",
            "--mirror",
            "replacement",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            promote.command,
            Command::Recovery {
                command: RecoveryCommand::Promote {
                    vault_root: Some(root),
                    mirrors,
                    json: true,
                }
            } if root == Path::new("survivor")
                && mirrors == vec![PathBuf::from("replacement")]
        ));

        let audit = Cli::try_parse_from([
            "hig",
            "recovery",
            "audit",
            "--vault-root",
            "vault",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            audit.command,
            Command::Recovery {
                command: RecoveryCommand::Audit {
                    vault_root: Some(root),
                    json: true,
                }
            } if root == Path::new("vault")
        ));

        let scrub = Cli::try_parse_from([
            "hig",
            "recovery",
            "scrub",
            "--vault-root",
            "vault",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            scrub.command,
            Command::Recovery {
                command: RecoveryCommand::Scrub {
                    vault_root: Some(root),
                    json: true,
                }
            } if root == Path::new("vault")
        ));

        let repair = Cli::try_parse_from([
            "hig",
            "recovery",
            "repair",
            "00112233445566778899aabbccddeeff",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "--mirror",
            "mirror",
            "--vault-root",
            "vault",
        ])
        .unwrap();
        assert!(matches!(
            repair.command,
            Command::Recovery {
                command: RecoveryCommand::Repair {
                    repository_id,
                    recovery_point_id,
                    mirror: Some(mirror),
                    vault_root: Some(root),
                    json: false,
                }
            } if repository_id.len() == 32
                && recovery_point_id.len() == 64
                && mirror == Path::new("mirror")
                && root == Path::new("vault")
        ));
    }

    #[test]
    fn repository_gc_is_report_only_without_apply() {
        let cli = Cli::try_parse_from(["hig", "repo", "gc", "project"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Repo {
                command: RepositoryCommand::Gc {
                    dir,
                    apply: false,
                    json: false,
                }
            } if dir == Path::new("project")
        ));
    }

    #[test]
    fn repository_range_restore_parses_exact_byte_selection() {
        let cli = Cli::try_parse_from([
            "hig",
            "repo",
            "restore-range",
            "project",
            "--path",
            "src/lib.rs",
            "--start",
            "17",
            "--len",
            "1",
            "--output",
            "letter.bin",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Repo {
                command: RepositoryCommand::RestoreRange {
                    dir,
                    path,
                    start: 17,
                    len: Some(1),
                    output,
                    json: true,
                    ..
                }
            } if dir == Path::new("project")
                && path == "src/lib.rs"
                && output == Path::new("letter.bin")
        ));
    }

    #[test]
    fn repository_watch_defaults_to_debounced_capture() {
        let cli = Cli::try_parse_from(["hig", "repo", "watch", "project"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Repo {
                command: RepositoryCommand::Watch {
                    dir,
                    debounce_ms: 750,
                    catch_up: true,
                    lifecycle_stdin: false,
                    json: false,
                    ..
                }
            } if dir == Path::new("project")
        ));
    }

    #[test]
    fn repository_symbol_restore_parses_semantic_selection() {
        let cli = Cli::try_parse_from([
            "hig",
            "repo",
            "restore-symbol",
            "project",
            "--revision",
            "01234567",
            "--symbol",
            "Thing::method",
            "--output",
            "method.rs",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Repo {
                command: RepositoryCommand::RestoreSymbol {
                    dir,
                    revision,
                    symbol,
                    output,
                    json: true,
                    ..
                }
            } if dir == Path::new("project")
                && revision == "01234567"
                && symbol == "Thing::method"
                && output == Path::new("method.rs")
        ));
    }

    #[test]
    fn pack_command_parses_low_memory_mode() {
        let cli = Cli::try_parse_from([
            "hig",
            "pack",
            "input",
            "--output",
            "archive.hig",
            "--memory-mode",
            "low",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Pack(args) if args.memory_mode == PayloadMemoryMode::Low
        ));
    }
}
