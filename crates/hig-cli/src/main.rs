#![recursion_limit = "256"]

mod benchmark;
mod commands;
mod output;
mod runtime;

use output::print_report;
use runtime::{
    effective_kdf_profile, effective_solid, ensure_daemon, handle_cache, handle_daemon,
    handle_project, handle_project_init, handle_project_watch, handle_session, handle_task,
    pack_with_daemon_policy, unlock_session_for_cache, validate_pack_encryption_args,
};

use clap::{Parser, Subcommand, ValueEnum};
use hig_core::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, DaemonMode, EncryptionMode, KdfProfile,
    ManifestFormat, PackOptions, PayloadMemoryMode, PipelineOptions, ProjectMode, SolidMode,
    SpeedMode, UnpackOptions, bench, inspect_archive, unpack,
};
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
struct ReportFlags {
    verbose: bool,
    json: bool,
    quiet: bool,
}

impl ReportFlags {
    fn mode(self) -> anyhow::Result<ReportMode> {
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
    Pack {
        input_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        password: Option<String>,
        #[arg(
            long,
            default_value = "password",
            help = "Encryption mode: password uses Argon2id + ChaCha20-Poly1305; none provides no confidentiality or authentication"
        )]
        encryption: EncryptionMode,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long, default_value = "zstd")]
        compression: Compression,
        #[arg(
            long,
            help = "Explicit zstd level; balanced otherwise selects level 5 or probes large chunks"
        )]
        level: Option<i32>,
        #[arg(long)]
        no_cache: bool,
        #[arg(long, default_value = "higv2")]
        format: ArchiveFormat,
        #[arg(long, default_value = "compact")]
        manifest_format: ManifestFormat,
        #[arg(long)]
        no_batch: bool,
        #[arg(long, default_value_t = 65_536)]
        small_file_threshold: u64,
        #[arg(long, default_value_t = 4_194_304)]
        max_batch_raw_bytes: u64,
        #[arg(long)]
        no_chunk: bool,
        #[arg(long, default_value_t = 8_388_608)]
        chunk_file_threshold: u64,
        #[arg(long, default_value_t = 1_048_576)]
        chunk_size: u64,
        #[arg(
            long,
            default_value = "balanced",
            help = "balanced keeps the safest defaults; fastest trusts metadata and reuses locally sealed ciphertext, revealing block equality"
        )]
        speed: SpeedMode,
        #[arg(
            long,
            help = "KDF profile: secure, interactive, or benchmark-only fast-bench. fastest defaults to interactive unless explicitly set"
        )]
        kdf_profile: Option<KdfProfile>,
        #[arg(
            long,
            help = "Trust path/size/mtime/permissions metadata to reuse cached hashes without rereading files. Fast, but unsafe if file contents changed while metadata was preserved. --speed fastest enables this automatically."
        )]
        trust_metadata: bool,
        #[arg(
            long,
            help = "Use a previously unlocked in-memory Hig session key and skip KDF"
        )]
        use_session: bool,
        #[arg(
            long,
            default_value = "auto",
            help = "Daemon mode: auto, off, or required"
        )]
        daemon: DaemonMode,
        #[arg(
            long,
            default_value = "auto",
            help = "Project snapshot mode: auto, off, or required"
        )]
        project: ProjectMode,
        #[arg(long, default_value = "auto", help = "Solid grouping: auto or off")]
        solid: SolidMode,
        #[arg(
            long,
            default_value = "adaptive",
            help = "Payload memory mode: adaptive or low (64 MiB with disk spooling)"
        )]
        memory_mode: PayloadMemoryMode,
        #[arg(long, help = "Print full human telemetry")]
        verbose: bool,
        #[arg(long, help = "Print complete PackReport as JSON")]
        json: bool,
        #[arg(long, help = "Suppress success output")]
        quiet: bool,
    },
    Unpack {
        archive_file: PathBuf,
        #[arg(short = 'd', long)]
        output_dir: PathBuf,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    Inspect {
        archive_file: PathBuf,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Bench {
        input_dir: PathBuf,
        #[arg(short, long, default_value = "bench.hig")]
        output: PathBuf,
        #[arg(long)]
        password: Option<String>,
        #[arg(
            long,
            default_value = "password",
            help = "Encryption mode: password uses Argon2id + ChaCha20-Poly1305; none provides no confidentiality or authentication"
        )]
        encryption: EncryptionMode,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long, default_value = "zstd")]
        compression: Compression,
        #[arg(
            long,
            help = "Explicit zstd level; balanced otherwise selects level 5 or probes large chunks"
        )]
        level: Option<i32>,
        #[arg(long)]
        no_cache: bool,
        #[arg(long, default_value = "higv2")]
        format: ArchiveFormat,
        #[arg(long, default_value = "compact")]
        manifest_format: ManifestFormat,
        #[arg(long)]
        no_batch: bool,
        #[arg(long, default_value_t = 65_536)]
        small_file_threshold: u64,
        #[arg(long, default_value_t = 4_194_304)]
        max_batch_raw_bytes: u64,
        #[arg(long)]
        no_chunk: bool,
        #[arg(long, default_value_t = 8_388_608)]
        chunk_file_threshold: u64,
        #[arg(long, default_value_t = 1_048_576)]
        chunk_size: u64,
        #[arg(long, default_value = "balanced")]
        speed: SpeedMode,
        #[arg(
            long,
            help = "KDF profile: secure, interactive, or fast-bench. fast-bench is for benchmarking only and is not recommended for production archives."
        )]
        kdf_profile: Option<KdfProfile>,
        #[arg(
            long,
            help = "Trust path/size/mtime/permissions metadata to reuse cached hashes without rereading files. Fast, but unsafe if file contents changed while metadata was preserved. --speed fastest enables this automatically."
        )]
        trust_metadata: bool,
        #[arg(
            long,
            help = "Use a previously unlocked in-memory Hig session key and skip KDF"
        )]
        use_session: bool,
        #[arg(
            long,
            default_value = "auto",
            help = "Daemon mode: auto, off, or required"
        )]
        daemon: DaemonMode,
        #[arg(long, default_value = "auto", help = "Solid grouping: auto or off")]
        solid: SolidMode,
        #[arg(long, help = "Print full human telemetry")]
        verbose: bool,
        #[arg(long, help = "Print complete PackReport as JSON")]
        json: bool,
        #[arg(long, help = "Suppress success output")]
        quiet: bool,
        #[arg(
            long,
            help = "Compare Hig against zip, tar+gzip, and tar+zstd, writing v1.9.7 benchmark artifacts"
        )]
        compare: bool,
        #[arg(
            long,
            help = "Directory used for benchmark temporary files and copy baseline qualification"
        )]
        bench_dir: Option<PathBuf>,
        #[arg(
            long,
            default_value = "all",
            help = "Benchmark corpus suite: source, lobehub, small500, textmix, repeat4m, random8m, binarymix, or all"
        )]
        bench_suite: BenchSuite,
    },
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
enum SessionCommand {
    Unlock {
        #[arg(long)]
        password: String,
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
enum DaemonCommand {
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
enum CacheCommand {
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
enum TaskCommand {
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
enum ProjectCommand {
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
enum ProjectPolicyCommand {
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

fn main() -> anyhow::Result<()> {
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
        Command::Pack {
            input_dir,
            output,
            password,
            encryption,
            cache_dir,
            threads,
            compression,
            level,
            no_cache,
            format,
            manifest_format,
            no_batch,
            small_file_threshold,
            max_batch_raw_bytes,
            no_chunk,
            chunk_file_threshold,
            chunk_size,
            speed,
            kdf_profile,
            trust_metadata,
            use_session,
            daemon,
            project,
            solid,
            memory_mode,
            verbose,
            json,
            quiet,
        } => {
            let report_mode = ReportFlags {
                verbose,
                json,
                quiet,
            }
            .mode()?;
            validate_pack_encryption_args(encryption, password.as_deref(), use_session)?;
            let kdf_profile = effective_kdf_profile(speed, kdf_profile);
            let solid = effective_solid(speed, solid);
            let options = PackOptions {
                input_dir,
                output_file: output,
                password,
                encryption,
                cache_dir,
                threads,
                compression,
                level,
                use_cache: !no_cache,
                trust_metadata,
                format,
                batch: BatchOptions {
                    enabled: !no_batch,
                    small_file_threshold,
                    max_batch_raw_bytes,
                },
                chunk: ChunkOptions {
                    enabled: !no_chunk,
                    chunk_file_threshold,
                    chunk_size,
                },
                speed,
                kdf_profile,
                sealed_cache: speed == SpeedMode::Fastest,
                manifest_format,
                use_session,
                session_required: use_session,
                session_ttl_secs: None,
                solid,
                pipeline: PipelineOptions {
                    daemon_mode: daemon,
                    project_mode: project,
                    payload_memory_mode: memory_mode,
                    ..PipelineOptions::default()
                },
            };
            let report = pack_with_daemon_policy(options, report_mode)?;
            print_report("pack", &report, report_mode)?;
        }
        Command::Unpack {
            archive_file,
            output_dir,
            password,
            overwrite,
        } => {
            unpack(UnpackOptions {
                archive_file,
                output_dir,
                password,
                overwrite,
            })?;
            println!("unpack: ok");
        }
        Command::Inspect {
            archive_file,
            password,
            json,
        } => {
            let inspection = inspect_archive(&archive_file, password.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&inspection)?);
            } else {
                println!(
                    "archive: format={:?} encrypted={} files={} input_bytes={} archive_bytes={}",
                    inspection.format,
                    inspection.encrypted,
                    inspection.files.len(),
                    inspection.input_bytes,
                    inspection.archive_bytes
                );
                for file in inspection.files {
                    println!(
                        "{}\t{}\t{:o}\t{}",
                        file.relative_path,
                        file.size,
                        file.permissions,
                        hex::encode(file.content_hash)
                    );
                }
            }
        }
        Command::Bench {
            input_dir,
            output,
            password,
            encryption,
            cache_dir,
            threads,
            compression,
            level,
            no_cache,
            format,
            manifest_format,
            no_batch,
            small_file_threshold,
            max_batch_raw_bytes,
            no_chunk,
            chunk_file_threshold,
            chunk_size,
            speed,
            kdf_profile,
            trust_metadata,
            use_session,
            daemon,
            solid,
            verbose,
            json,
            quiet,
            compare,
            bench_dir,
            bench_suite,
        } => {
            let password = password.or_else(|| std::env::var("HIG_BENCH_PASSWORD").ok());
            let report_mode = ReportFlags {
                verbose,
                json,
                quiet,
            }
            .mode()?;
            validate_pack_encryption_args(encryption, password.as_deref(), use_session)?;
            let kdf_profile = effective_kdf_profile(speed, kdf_profile);
            let solid = effective_solid(speed, solid);
            if compare {
                if password.is_none() {
                    anyhow::bail!("--compare requires --password for secure benchmark rows");
                }
                benchmark::run_compare(benchmark::CompareOptions {
                    input_dir,
                    output,
                    password,
                    cache_dir,
                    threads,
                    compression,
                    level,
                    use_cache: !no_cache,
                    batch: BatchOptions {
                        enabled: !no_batch,
                        small_file_threshold,
                        max_batch_raw_bytes,
                    },
                    chunk: ChunkOptions {
                        enabled: !no_chunk,
                        chunk_file_threshold,
                        chunk_size,
                    },
                    bench_dir,
                    bench_suite,
                    manifest_format,
                    use_session,
                    daemon,
                    solid,
                    report_mode,
                })?;
                return Ok(());
            }
            let report = bench(PackOptions {
                input_dir,
                output_file: output,
                password,
                encryption,
                cache_dir,
                threads,
                compression,
                level,
                use_cache: !no_cache,
                trust_metadata,
                format,
                batch: BatchOptions {
                    enabled: !no_batch,
                    small_file_threshold,
                    max_batch_raw_bytes,
                },
                chunk: ChunkOptions {
                    enabled: !no_chunk,
                    chunk_file_threshold,
                    chunk_size,
                },
                speed,
                kdf_profile,
                sealed_cache: speed == SpeedMode::Fastest,
                manifest_format,
                use_session,
                session_required: use_session,
                session_ttl_secs: None,
                solid,
                pipeline: PipelineOptions {
                    daemon_mode: daemon,
                    ..PipelineOptions::default()
                },
            })?;
            print_report("bench:first", &report.first, report_mode)?;
            print_report("bench:second", &report.second, report_mode)?;
            if report_mode != ReportMode::Quiet && report.second.duration.as_secs_f64() > 0.0 {
                println!(
                    "bench:speedup {:.2}x",
                    report.first.duration.as_secs_f64() / report.second.duration.as_secs_f64()
                );
            }
        }
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
    use crate::runtime::response_mode_for_report;
    use hig_core::PackResponseMode;
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
            Command::Inspect {
                archive_file,
                password: None,
                json: true,
            } if archive_file == Path::new("archive.hig")
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
            Command::Pack {
                memory_mode: PayloadMemoryMode::Low,
                ..
            }
        ));
    }
}
