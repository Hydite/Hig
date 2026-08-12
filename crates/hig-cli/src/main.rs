#![recursion_limit = "256"]

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use hig_core::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, DaemonMode, DaemonRequest,
    DaemonResponse, EncryptionMode, JobKeyMaterial, KdfProfile, ManifestFormat, PackAuthMode,
    PackJobRequest, PackOptions, PackReport, PackResponseMode, PayloadMemoryMode, PipelineOptions,
    ProjectMode, ProjectRegistration, RepositoryWatcher, SerializablePackOptions, SolidMode,
    SpeedMode, UnpackOptions, bench, cache_writer_available, daemon_socket_path, daemon_status,
    default_session_ttl, derive_key, derive_session_binding, discover_project, gc_repository,
    init_project, init_repository, inspect_archive, pack, random_bytes, repository_diff,
    repository_log, repository_path_history, repository_storage_tree, repository_symbol_history,
    repository_symbols, request_daemon, resolve_project_cache_dir, restore_repository,
    restore_repository_range, restore_repository_symbol, run_daemon_server, snapshot_repository,
    stop_daemon, unpack, verify_repository,
};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BenchSuite {
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
enum ReportMode {
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
}

#[derive(Debug, Subcommand)]
enum RepositoryCommand {
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
        Command::Repo { command } => handle_repository(command)?,
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
                run_compare(CompareOptions {
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

fn handle_project_init(
    dir: &Path,
    cache_dir: Option<PathBuf>,
    excludes: Vec<String>,
) -> anyhow::Result<()> {
    let config = init_project(dir, cache_dir, excludes)?;
    let root = dir.canonicalize()?;
    let cache = resolve_project_cache_dir(&root, &config);
    ensure_daemon(&cache, default_session_ttl(None))?;
    let response = request_daemon(
        &cache,
        DaemonRequest::ProjectRegister(ProjectRegistration {
            root: root.clone(),
            config: config.clone(),
        }),
    )?;
    match response {
        Some(DaemonResponse::ProjectRegistered(status)) => {
            println!(
                "project: initialized root={} cache_dir={} project_id={} files={} generation={} state={:?}",
                root.display(),
                cache.display(),
                hex::encode(config.project_id),
                status.files,
                status.generation,
                status.snapshot_validity
            );
            Ok(())
        }
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not register project"),
    }
}

fn project_registration(
    dir: &Path,
    cache_override: Option<&Path>,
) -> anyhow::Result<(PathBuf, PathBuf, hig_core::ProjectConfig)> {
    let (root, config) = discover_project(dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "project is not initialized; run `hig init {}`",
            dir.display()
        )
    })?;
    let cache = resolve_project_cache_dir(&root, &config);
    if let Some(override_path) = cache_override {
        let expected = cache.canonicalize().unwrap_or(cache.clone());
        let requested = override_path
            .canonicalize()
            .unwrap_or_else(|_| override_path.to_path_buf());
        anyhow::ensure!(
            expected == requested,
            "--cache-dir does not match the initialized project cache"
        );
    }
    Ok((root, cache, config))
}

fn register_project(
    root: &Path,
    cache: &Path,
    config: &hig_core::ProjectConfig,
) -> anyhow::Result<hig_core::ProjectStatusReport> {
    ensure_daemon(cache, default_session_ttl(None))?;
    match request_daemon(
        cache,
        DaemonRequest::ProjectRegister(ProjectRegistration {
            root: root.to_path_buf(),
            config: config.clone(),
        }),
    )? {
        Some(DaemonResponse::ProjectRegistered(status)) => Ok(status),
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not register project"),
    }
}

fn handle_project_watch(
    dir: &Path,
    cache_override: Option<PathBuf>,
    verbose: bool,
) -> anyhow::Result<()> {
    let (root, cache, config) = project_registration(dir, cache_override.as_deref())?;
    let mut status = register_project(&root, &cache, &config)?;
    println!(
        "watch: active root={} backend={} generation={} files={}",
        root.display(),
        status.watcher_backend,
        status.generation,
        status.files
    );
    loop {
        std::thread::sleep(Duration::from_millis(250));
        match request_daemon(
            &cache,
            DaemonRequest::ProjectStatus {
                project_id: config.project_id,
            },
        )? {
            Some(DaemonResponse::ProjectStatus(next)) => {
                if verbose && next.event_sequence != status.event_sequence {
                    println!(
                        "watch: generation={} events={} pending={} dirty_files={} state={:?}",
                        next.generation,
                        next.event_sequence,
                        next.pending_events,
                        next.dirty_files,
                        next.snapshot_validity
                    );
                }
                status = next;
            }
            Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
            _ => anyhow::bail!("watch daemon became unavailable"),
        }
    }
}

fn handle_project(command: ProjectCommand) -> anyhow::Result<()> {
    match command {
        ProjectCommand::Status { dir, json } => {
            let (root, cache, config) = project_registration(&dir, None)?;
            register_project(&root, &cache, &config)?;
            match request_daemon(
                &cache,
                DaemonRequest::ProjectStatus {
                    project_id: config.project_id,
                },
            )? {
                Some(DaemonResponse::ProjectStatus(status)) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&status)?);
                    } else {
                        println!(
                            "project: root={} state={:?} generation={} files={} pending={} dirty_files={} backend={} prepared_bytes={}",
                            status.root,
                            status.snapshot_validity,
                            status.generation,
                            status.files,
                            status.pending_events,
                            status.dirty_files,
                            status.watcher_backend,
                            status.prepared_bytes
                        );
                    }
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return project status"),
            }
        }
        ProjectCommand::Rebuild { dir, wait: _ } => {
            let (root, cache, config) = project_registration(&dir, None)?;
            register_project(&root, &cache, &config)?;
            match request_daemon(
                &cache,
                DaemonRequest::ProjectRebuild {
                    project_id: config.project_id,
                },
            )? {
                Some(DaemonResponse::ProjectStatus(status)) => {
                    println!(
                        "project: rebuilt generation={} files={} state={:?}",
                        status.generation, status.files, status.snapshot_validity
                    );
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not rebuild project"),
            }
        }
    }
}

fn handle_repository(command: RepositoryCommand) -> anyhow::Result<()> {
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
                    short_repository_id(report.commit_id),
                    report
                        .parent_id
                        .map(short_repository_id)
                        .unwrap_or_else(|| "none".to_string()),
                    report.files,
                    report.input_bytes,
                    report.objects_written,
                    report.chunks_written,
                    report.chunks_reused
                );
            }
        }
        RepositoryCommand::Log { dir, limit, json } => {
            anyhow::ensure!(limit > 0, "--limit must be greater than zero");
            let commits = repository_log(&dir, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&commits)?);
            } else {
                for commit in commits {
                    println!(
                        "commit {} parent={} time_ns={} files={} bytes={} message={}",
                        short_repository_id(commit.commit_id),
                        commit
                            .parent_id
                            .map(short_repository_id)
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
                        .map(short_repository_id)
                        .unwrap_or_else(|| "empty".to_string()),
                    short_repository_id(report.to),
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
                    short_repository_id(report.commit_id),
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
                    short_repository_id(report.commit_id),
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
                    short_repository_id(report.head),
                    report.query_path,
                    report.entries.len()
                );
                for entry in report.entries {
                    println!(
                        "{:?}\t{}\t{}\tranges={}",
                        entry.kind,
                        short_repository_id(entry.commit_id),
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
                    short_repository_id(report.commit_id),
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
                    short_repository_id(report.commit_id),
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
                    short_repository_id(report.head),
                    &report.resolved_symbol_id[..12],
                    report.entries.len()
                );
                for entry in report.entries {
                    println!(
                        "{:?}\t{}\t{}\t{}",
                        entry.kind,
                        short_repository_id(entry.commit_id),
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
                    short_repository_id(report.commit_id),
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
                            short_repository_id(report.commit_id),
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

fn short_repository_id(id: hig_core::RepositoryObjectId) -> String {
    id.to_hex()[..12].to_string()
}

fn handle_cache(command: CacheCommand) -> anyhow::Result<()> {
    let (cache_dir, request) = match command {
        CacheCommand::Status { cache_dir } => (
            cache_dir.unwrap_or_else(default_cache_dir),
            DaemonRequest::CacheStatus,
        ),
        CacheCommand::Gc { cache_dir, dry_run } => (
            cache_dir.unwrap_or_else(default_cache_dir),
            DaemonRequest::CacheGc { dry_run },
        ),
        CacheCommand::Compact { cache_dir, dry_run } => (
            cache_dir.unwrap_or_else(default_cache_dir),
            DaemonRequest::CacheCompact { dry_run },
        ),
    };
    ensure_daemon(&cache_dir, default_session_ttl(None))?;
    match request_daemon(&cache_dir, request)? {
        Some(DaemonResponse::CacheMaintenance(report)) => {
            println!(
                "cache: total_bytes={} budget_bytes={} files={} removable_bytes={} removed_bytes={} compacted_bytes={} generation={} journal_bytes={} journal_entries={} journal_replayed_entries={} journal_compacted_entries={} journal_dirty_record_estimate={} journal_compact_recommended={} journal_estimated_reclaimed_bytes={} last_compact_unix_ns={} dry_run={}",
                report.total_bytes,
                report.budget_bytes,
                report.files,
                report.removable_bytes,
                report.removed_bytes,
                report.compacted_bytes,
                report.generation,
                report.journal_bytes,
                report.journal_entries,
                report.journal_replayed_entries,
                report.journal_compacted_entries,
                report.journal_dirty_record_estimate,
                report.journal_compact_recommended,
                report.journal_estimated_reclaimed_bytes,
                report.last_compact_unix_ns,
                report.dry_run
            );
            Ok(())
        }
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not return cache maintenance status"),
    }
}

fn handle_task(command: TaskCommand) -> anyhow::Result<()> {
    match command {
        TaskCommand::List {
            cache_dir,
            include_completed,
        } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            match request_daemon(&cache_dir, DaemonRequest::TaskList { include_completed })? {
                Some(DaemonResponse::TaskList(tasks)) => {
                    println!("{}", serde_json::to_string_pretty(&tasks)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return task list"),
            }
        }
        TaskCommand::Status { task_id, cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let task_id = parse_task_id(&task_id)?;
            match request_daemon(&cache_dir, DaemonRequest::TaskStatus { task_id })? {
                Some(DaemonResponse::TaskStatus(status)) => {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return task status"),
            }
        }
        TaskCommand::Cancel { task_id, cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let task_id = parse_task_id(&task_id)?;
            match request_daemon(&cache_dir, DaemonRequest::TaskCancel { task_id })? {
                Some(DaemonResponse::TaskStatus(status)) => {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not cancel task"),
            }
        }
        TaskCommand::Result { task_id, cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let task_id = parse_task_id(&task_id)?;
            match request_daemon(&cache_dir, DaemonRequest::TaskResult { task_id })? {
                Some(DaemonResponse::TaskResult(result)) => {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return task result"),
            }
        }
    }
}

fn parse_task_id(value: &str) -> anyhow::Result<[u8; 16]> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("task id must be a 16-byte hex string"))
}

fn validate_pack_encryption_args(
    encryption: EncryptionMode,
    password: Option<&str>,
    use_session: bool,
) -> anyhow::Result<()> {
    match (encryption, password, use_session) {
        (EncryptionMode::Password, Some(value), _) if !value.is_empty() => Ok(()),
        (EncryptionMode::Password, None, true) => Ok(()),
        (EncryptionMode::Password, _, _) => {
            anyhow::bail!("--encryption password requires --password")
        }
        (EncryptionMode::None, None, false) => Ok(()),
        (EncryptionMode::None, _, _) => {
            anyhow::bail!("--password and --use-session cannot be used with --encryption none")
        }
    }
}

fn pack_with_daemon_policy(
    mut options: PackOptions,
    report_mode: ReportMode,
) -> anyhow::Result<PackReport> {
    if options.pipeline.project_mode != ProjectMode::Off
        && options.cache_dir.is_none()
        && let Some((root, config)) = discover_project(&options.input_dir)?
    {
        options.cache_dir = Some(resolve_project_cache_dir(&root, &config));
    }
    if options.pipeline.project_mode == ProjectMode::Required
        && (options.format == ArchiveFormat::HigV1
            || options.pipeline.daemon_mode == DaemonMode::Off
            || !options.use_cache)
    {
        anyhow::bail!("--project required needs HIGV2, cache, and an active daemon");
    }
    if options.format == ArchiveFormat::HigV1
        || options.pipeline.daemon_mode == DaemonMode::Off
        || !options.use_cache
    {
        return pack(options);
    }
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| options.input_dir.join(".hig-cache"));
    fs::create_dir_all(&cache_dir)?;
    let connect_started = Instant::now();
    let daemon_ready = ensure_daemon(&cache_dir, default_session_ttl(None));
    if let Err(error) = daemon_ready {
        if options.pipeline.daemon_mode == DaemonMode::Required || options.use_session {
            return Err(error);
        }
        return pack(options);
    }
    let daemon_connect_us = connect_started.elapsed().as_micros() as u64;
    let kdf = options.kdf_profile.params();
    let binding = derive_session_binding(&cache_dir, options.kdf_profile, &kdf, options.encryption);
    let mut ephemeral_key = None;
    let auth_mode = if options.encryption == EncryptionMode::None {
        PackAuthMode::None
    } else if options.use_session {
        PackAuthMode::UseSession
    } else if options.password.is_some() {
        let salt = random_bytes::<16>();
        let password = options
            .password
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("password encryption requires a password"))?;
        let key = derive_key(password, &salt, &kdf)?;
        ephemeral_key = Some(JobKeyMaterial { key, salt });
        PackAuthMode::PreferSessionOrJobKey
    } else {
        PackAuthMode::UseSession
    };
    let request = DaemonRequest::SubmitTask(hig_core::TaskSubmitRequest {
        request: hig_core::TaskRequest::Pack(PackJobRequest {
            options: SerializablePackOptions::from_pack(&options),
            binding_fingerprint: Some(binding.fingerprint),
            ephemeral_key,
            auth_mode,
            response_mode: response_mode_for_report(report_mode),
        }),
    });
    let socket_started = Instant::now();
    let response = request_daemon(&cache_dir, request);
    let pack_roundtrip_us = socket_started.elapsed().as_micros() as u64;
    match response {
        Ok(Some(DaemonResponse::TaskAccepted(status))) => {
            let task_id = status.task_id;
            let result = wait_for_task_result(&cache_dir, task_id)?;
            let hig_core::TaskResult::Pack { report } = result else {
                anyhow::bail!("daemon task did not return a pack result");
            };
            let mut report = *report;
            report.timings_us.daemon_connect_us = daemon_connect_us;
            report.timings_us.socket_request_us = pack_roundtrip_us;
            report.timings_us.socket_pack_roundtrip_us = pack_roundtrip_us;
            Ok(report)
        }
        Err(error) if options.pipeline.daemon_mode == DaemonMode::Auto && !options.use_session => {
            if cache_writer_available(&cache_dir)? {
                eprintln!("hig: daemon exited, falling back to standalone: {error}");
                pack(options)
            } else {
                Err(error)
            }
        }
        Ok(Some(DaemonResponse::Error { code, message })) => {
            anyhow::bail!("daemon {:?}: {}", code, message)
        }
        Ok(_) => anyhow::bail!("daemon returned an unexpected pack response"),
        Err(error) => Err(error),
    }
}

fn wait_for_task_result(
    cache_dir: &Path,
    task_id: [u8; 16],
) -> anyhow::Result<hig_core::TaskResult> {
    loop {
        match request_daemon(cache_dir, DaemonRequest::TaskResult { task_id }) {
            Ok(Some(DaemonResponse::TaskResult(result))) => return Ok(result),
            Ok(Some(DaemonResponse::Error { message, .. })) if message.contains("not ready") => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(Some(DaemonResponse::Error { message, .. })) => anyhow::bail!(message),
            Ok(_) => anyhow::bail!("daemon returned an unexpected task result response"),
            Err(error) => return Err(error),
        }
    }
}

fn response_mode_for_report(report_mode: ReportMode) -> PackResponseMode {
    match report_mode {
        ReportMode::Short | ReportMode::Quiet => PackResponseMode::Summary,
        ReportMode::Verbose | ReportMode::Json => PackResponseMode::Full,
    }
}

fn effective_kdf_profile(speed: SpeedMode, requested: Option<KdfProfile>) -> KdfProfile {
    requested.unwrap_or(match speed {
        SpeedMode::Balanced => KdfProfile::Secure,
        SpeedMode::Fastest => KdfProfile::Interactive,
    })
}

fn effective_solid(speed: SpeedMode, requested: SolidMode) -> SolidMode {
    match speed {
        SpeedMode::Balanced => requested,
        SpeedMode::Fastest => SolidMode::Off,
    }
}

fn handle_session(command: SessionCommand) -> anyhow::Result<()> {
    match command {
        SessionCommand::Unlock {
            password,
            cache_dir,
            ttl_secs,
            kdf_profile,
        } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            fs::create_dir_all(&cache_dir)?;
            let ttl_secs = default_session_ttl(ttl_secs);
            let started = Instant::now();
            ensure_daemon(&cache_dir, ttl_secs)?;
            unlock_session_for_cache(&cache_dir, &password, kdf_profile, ttl_secs)?;
            let status = daemon_status(&cache_dir)?;
            println!(
                "session: unlocked cache_dir={} ttl_secs={} age_secs={} kdf_ms={}",
                cache_dir.display(),
                ttl_secs,
                status.session_age_secs,
                started.elapsed().as_millis()
            );
            Ok(())
        }
        SessionCommand::Status { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let status = daemon_status(&cache_dir)?;
            if status.session_active {
                println!(
                    "session: active cache_dir={} age_secs={} ttl_secs={}",
                    cache_dir.display(),
                    status.session_age_secs,
                    status.ttl_secs
                );
            } else {
                println!("session: inactive cache_dir={}", cache_dir.display());
            }
            Ok(())
        }
        SessionCommand::Clear { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let cleared = matches!(
                request_daemon(&cache_dir, DaemonRequest::ClearSession)?,
                Some(DaemonResponse::SessionCleared)
            );
            println!(
                "session: {} cache_dir={}",
                if cleared { "cleared" } else { "inactive" },
                cache_dir.display()
            );
            Ok(())
        }
    }
}

fn handle_daemon(command: DaemonCommand) -> anyhow::Result<()> {
    match command {
        DaemonCommand::Start {
            cache_dir,
            ttl_secs,
        } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            fs::create_dir_all(&cache_dir)?;
            let ttl_secs = default_session_ttl(ttl_secs);
            ensure_daemon(&cache_dir, ttl_secs)?;
            let status = daemon_status(&cache_dir)?;
            println!(
                "daemon: started cache_dir={} active={} ttl_secs={}",
                cache_dir.display(),
                status.active,
                ttl_secs
            );
            Ok(())
        }
        DaemonCommand::Status { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let status = daemon_status(&cache_dir)?;
            if status.active {
                println!(
                    "daemon: active cache_dir={} age_secs={} uptime_secs={} ttl_secs={} jobs_completed={} active_jobs={} queued_jobs={} cache_open_count={} session_active={} journal_bytes={} watched_projects={} project_ready_count={} project_pending_events={}",
                    cache_dir.display(),
                    status.age_secs,
                    status.uptime_secs,
                    status.ttl_secs,
                    status.jobs_completed,
                    status.active_jobs,
                    status.queued_jobs,
                    status.cache_open_count,
                    status.session_active,
                    status.journal_bytes,
                    status.watched_projects,
                    status.project_ready_count,
                    status.project_pending_events
                );
            } else {
                println!("daemon: inactive cache_dir={}", cache_dir.display());
            }
            Ok(())
        }
        DaemonCommand::Stop { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let stopped = stop_daemon(&cache_dir)?;
            println!(
                "daemon: {} cache_dir={}",
                if stopped { "stopped" } else { "inactive" },
                cache_dir.display()
            );
            Ok(())
        }
        DaemonCommand::Serve {
            cache_dir,
            ttl_secs,
        } => run_daemon_server(&cache_dir, ttl_secs),
    }
}

fn ensure_daemon(cache_dir: &Path, ttl_secs: u64) -> anyhow::Result<()> {
    if daemon_status(cache_dir)?.active {
        return Ok(());
    }
    fs::create_dir_all(cache_dir)?;
    let socket = daemon_socket_path(cache_dir);
    if socket.exists() {
        let _ = fs::remove_file(&socket);
    }
    let mut child = ProcessCommand::new(std::env::current_exe()?)
        .arg("daemon")
        .arg("serve")
        .arg("--cache-dir")
        .arg(cache_dir)
        .arg("--ttl-secs")
        .arg(ttl_secs.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    for _ in 0..100 {
        if socket.exists() && daemon_status(cache_dir)?.active {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            anyhow::bail!("Hig daemon exited during startup");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    anyhow::bail!("Hig daemon did not create its socket")
}

fn unlock_session_for_cache(
    cache_dir: &Path,
    password: &str,
    kdf_profile: KdfProfile,
    ttl_secs: u64,
) -> anyhow::Result<()> {
    fs::create_dir_all(cache_dir)?;
    ensure_daemon(cache_dir, ttl_secs)?;
    let kdf = kdf_profile.params();
    let binding = derive_session_binding(cache_dir, kdf_profile, &kdf, EncryptionMode::Password);
    let salt = match request_daemon(
        cache_dir,
        DaemonRequest::UnlockChallenge {
            binding: binding.clone(),
        },
    )? {
        Some(DaemonResponse::UnlockChallenge { salt }) => salt,
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not provide an unlock challenge"),
    };
    let mut key = derive_key(password, &salt, &kdf)?;
    let response = request_daemon(
        cache_dir,
        DaemonRequest::InstallSessionKey {
            binding,
            key,
            salt,
            ttl_secs,
        },
    );
    use zeroize::Zeroize;
    key.zeroize();
    match response? {
        Some(DaemonResponse::SessionInstalled) => Ok(()),
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not install the session key"),
    }
}

fn default_cache_dir() -> PathBuf {
    PathBuf::from(".hig-cache")
}

fn print_report(label: &str, report: &PackReport, mode: ReportMode) -> anyhow::Result<()> {
    match mode {
        ReportMode::Short => print_short_report(label, report),
        ReportMode::Verbose => print_verbose_report(label, report),
        ReportMode::Json => print_json_report(label, report),
        ReportMode::Quiet => Ok(()),
    }
}

fn print_short_report(label: &str, report: &PackReport) -> anyhow::Result<()> {
    let seconds = report.duration.as_secs_f64().max(0.000_001);
    let mib = report.input_bytes as f64 / 1024.0 / 1024.0;
    let reduction = if report.input_bytes == 0 {
        0.0
    } else {
        (1.0 - report.archive_bytes as f64 / report.input_bytes as f64) * 100.0
    };
    println!(
        "{label}: files={} input_bytes={} archive_bytes={} reduction={:.2}% duration_ms={:.3} throughput_mib_s={:.2} encryption={:?} speed={:?} cache_hit_rate={:.2}% project_mode_used={} project_generation={}",
        report.input_files,
        report.input_bytes,
        report.archive_bytes,
        reduction,
        report.duration.as_secs_f64() * 1000.0,
        mib / seconds,
        report.encryption_mode,
        report.speed,
        report.cache.hit_rate() * 100.0,
        report.project.project_mode_used,
        report.project.project_generation
    );
    Ok(())
}

fn print_json_report(label: &str, report: &PackReport) -> anyhow::Result<()> {
    let value = serde_json::json!({
        "label": label,
        "report": report,
    });
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

fn print_verbose_report(label: &str, report: &PackReport) -> anyhow::Result<()> {
    let seconds = report.duration.as_secs_f64().max(0.000_001);
    let mib = report.input_bytes as f64 / 1024.0 / 1024.0;
    println!(
        "{label}: files={} input_bytes={} archive_bytes={} duration_ms={} throughput_mib_s={:.2} encryption={:?} speed={:?} kdf_profile={:?} session_used={} session_lookup_ms={} session_key_age_secs={} kdf_skipped_by_session={} workers={} writer_strategy={:?} preallocated_bytes={} preallocation_enabled={} cached_payload_opens={} cached_range_opens={} cached_payload_read_bytes={} prefetched_bytes={} direct_writes={} buffered_writes={} peak_pipeline_memory_bytes={} scan_ms={} plan_ms={} kdf_ms={} kdf_overlapped_ms={} read_ms={} compression_ms={} crypto_ms={} pack_blocks_ms={} manifest_ms={} write_ms={} payload_read_ms={} payload_write_ms={} writer_wait_ms={} output_flush_ms={} output_rename_ms={} cache_hits={} cache_misses={} cache_hit_rate={:.2}% hashed_files={} metadata_hash_reuses={} scan_cache_hits={} scan_cache_misses={} scan_cache_hit_rate={:.2}% chunk_metadata_reuses={} chunk_metadata_misses={} trusted_bytes_skipped={} hot_raw_bytes_budget={} hot_raw_bytes={} hot_chunk_raw_bytes={} batch_blocks={} single_blocks={} batched_files={} batch_cache_hits={} batch_cache_misses={} solid_groups={} solid_files={} solid_cache_hits={} solid_cache_misses={} solid_group_bytes={} chunked_files={} chunk_blocks={} chunk_cache_hits={} chunk_cache_misses={} chunk_bytes_reused={} chunk_bytes_compressed={} chunk_plan_cache_hits={} chunk_plan_cache_misses={} sealed_block_hits={} sealed_block_misses={} sealed_bytes_reused={} reencrypted_cache_hits={} payload_source_cache_files={} payload_source_memory_bytes={} payload_source_spool_payloads={} payload_source_spool_bytes={} source_read_bytes={} source_hot_raw_bytes={} payload_memory_mode={:?} payload_memory_budget_bytes={} payload_memory_available_bytes={} cache_pack_hits={} cache_pack_misses={} cache_pack_fallbacks={}",
        report.input_files,
        report.input_bytes,
        report.archive_bytes,
        report.duration.as_millis(),
        mib / seconds,
        report.encryption_mode,
        report.speed,
        report.kdf_profile,
        report.session.session_used,
        report.session.session_lookup_ms,
        report.session.session_key_age_secs,
        report.session.kdf_skipped_by_session,
        report.worker_count,
        report.writer_strategy,
        report.archive_preallocated_bytes,
        report.preallocation_enabled,
        report.cached_payload_open_count,
        report.cached_range_open_count,
        report.cached_payload_read_bytes,
        report.prefetched_bytes,
        report.direct_write_count,
        report.buffered_write_count,
        report.peak_pipeline_memory_bytes,
        report.timings.scan_ms,
        report.timings.plan_ms,
        report.timings.kdf_ms,
        report.timings.kdf_overlapped_ms,
        report.timings.read_ms,
        report.timings.compression_ms,
        report.timings.crypto_ms,
        report.timings.pack_blocks_ms,
        report.timings.manifest_ms,
        report.timings.write_ms,
        report.timings.payload_read_ms,
        report.timings.payload_write_ms,
        report.timings.writer_wait_ms,
        report.timings.output_flush_ms,
        report.timings.output_rename_ms,
        report.cache.hits,
        report.cache.misses,
        report.cache.hit_rate() * 100.0,
        report.scan.hashed_files,
        report.scan.metadata_hash_reuses,
        report.scan.scan_cache_hits,
        report.scan.scan_cache_misses,
        report.scan.scan_cache_hit_rate() * 100.0,
        report.scan.chunk_metadata_reuses,
        report.scan.chunk_metadata_misses,
        report.scan.trusted_bytes_skipped,
        report.scan.hot_raw_bytes_budget,
        report.scan.hot_raw_bytes,
        report.scan.hot_chunk_raw_bytes,
        report.blocks.batch_blocks,
        report.blocks.single_blocks,
        report.blocks.batched_files,
        report.blocks.batch_cache_hits,
        report.blocks.batch_cache_misses,
        report.blocks.solid_groups,
        report.blocks.solid_files,
        report.blocks.solid_cache_hits,
        report.blocks.solid_cache_misses,
        report.blocks.solid_group_bytes,
        report.blocks.chunked_files,
        report.blocks.chunk_blocks,
        report.blocks.chunk_cache_hits,
        report.blocks.chunk_cache_misses,
        report.blocks.chunk_bytes_reused,
        report.blocks.chunk_bytes_compressed,
        report.blocks.chunk_plan_cache_hits,
        report.blocks.chunk_plan_cache_misses,
        report.blocks.sealed_block_hits,
        report.blocks.sealed_block_misses,
        report.blocks.sealed_bytes_reused,
        report.blocks.reencrypted_cache_hits,
        report.blocks.payload_source_cache_files,
        report.blocks.payload_source_memory_bytes,
        report.blocks.payload_source_spool_payloads,
        report.blocks.payload_source_spool_bytes,
        report.blocks.source_read_bytes,
        report.blocks.source_hot_raw_bytes,
        report.blocks.payload_memory_mode,
        report.blocks.payload_memory_budget_bytes,
        report.blocks.payload_memory_available_bytes,
        report.blocks.cache_pack_hits,
        report.blocks.cache_pack_misses,
        report.blocks.cache_pack_fallbacks
    );
    println!(
        "{label}:project mode_used={} generation={} snapshot_valid={} metadata_verified_files={} hash_reuses={} prepared_hits={} prepared_misses={} dirty_files={} dirty_groups={} verify_us={} freeze_us={} retries={}",
        report.project.project_mode_used,
        report.project.project_generation,
        report.project.project_snapshot_valid,
        report.project.project_metadata_verified_files,
        report.project.project_hash_reuses,
        report.project.project_prepared_object_hits,
        report.project.project_prepared_object_misses,
        report.project.project_dirty_files,
        report.project.project_dirty_groups,
        report.project.project_verify_us,
        report.project.project_freeze_us,
        report.project.project_retry_count,
    );
    println!(
        "{label}:us total_us={} daemon_connect_us={} socket_request_us={} socket_connect_us={} socket_pack_roundtrip_us={} daemon_auth_us={} daemon_job_execute_us={} daemon_response_bytes={} client_decode_us={} queue_wait_us={} cache_hot_lookup_us={} walk_us={} metadata_us={} hash_us={} plan_us={} read_us={} compression_us={} crypto_us={} cache_commit_wait_us={} cache_commit_us={} manifest_serialize_us={} manifest_compress_us={} manifest_encrypt_us={} output_create_us={} output_preallocate_us={} output_header_write_us={} output_manifest_write_us={} output_payload_read_us={} output_payload_write_us={} output_write_us={} output_flush_us={} output_fsync_us={} output_rename_us={} response_serialize_us={} unattributed_us={}",
        report.timings_us.total_us,
        report.timings_us.daemon_connect_us,
        report.timings_us.socket_request_us,
        report.timings_us.socket_connect_us,
        report.timings_us.socket_pack_roundtrip_us,
        report.timings_us.daemon_auth_us,
        report.timings_us.daemon_job_execute_us,
        report.timings_us.daemon_response_bytes,
        report.timings_us.client_decode_us,
        report.timings_us.queue_wait_us,
        report.timings_us.cache_hot_lookup_us,
        report.timings_us.walk_us,
        report.timings_us.metadata_us,
        report.timings_us.hash_us,
        report.timings_us.plan_us,
        report.timings_us.read_us,
        report.timings_us.compression_us,
        report.timings_us.crypto_us,
        report.timings_us.cache_commit_wait_us,
        report.timings_us.cache_commit_us,
        report.timings_us.manifest_serialize_us,
        report.timings_us.manifest_compress_us,
        report.timings_us.manifest_encrypt_us,
        report.timings_us.output_create_us,
        report.timings_us.output_preallocate_us,
        report.timings_us.output_header_write_us,
        report.timings_us.output_manifest_write_us,
        report.timings_us.output_payload_read_us,
        report.timings_us.output_payload_write_us,
        report.timings_us.output_write_us,
        report.timings_us.output_flush_us,
        report.timings_us.output_fsync_us,
        report.timings_us.output_rename_us,
        report.timings_us.response_serialize_us,
        report.timings_us.unattributed_us,
    );
    println!(
        "{label}:write_profile temp_create_us={} preallocate_us={} header_write_us={} manifest_write_us={} payload_read_us={} payload_write_us={} payload_memory_write_us={} payload_cached_write_us={} direct_write_us={} buffered_write_us={} writer_wait_us={} flush_us={} fsync_us={} rename_us={} memory_payload_count={} memory_payload_bytes={} cached_file_payload_count={} cached_file_payload_bytes={} cached_range_payload_count={} cached_range_payload_bytes={} direct_writes={} buffered_writes={} coalesced_writes={} coalesced_payloads={} coalesced_bytes={}",
        report.write_profile.temp_create_us,
        report.write_profile.preallocate_us,
        report.write_profile.header_write_us,
        report.write_profile.manifest_write_us,
        report.write_profile.payload_read_us,
        report.write_profile.payload_write_us,
        report.write_profile.payload_memory_write_us,
        report.write_profile.payload_cached_write_us,
        report.write_profile.direct_write_us,
        report.write_profile.buffered_write_us,
        report.write_profile.writer_wait_us,
        report.write_profile.flush_us,
        report.write_profile.fsync_us,
        report.write_profile.rename_us,
        report.write_profile.memory_payload_count,
        report.write_profile.memory_payload_bytes,
        report.write_profile.cached_file_payload_count,
        report.write_profile.cached_file_payload_bytes,
        report.write_profile.cached_range_payload_count,
        report.write_profile.cached_range_payload_bytes,
        report.write_profile.direct_write_count,
        report.write_profile.buffered_write_count,
        report.write_profile.coalesced_write_count,
        report.write_profile.coalesced_payload_count,
        report.write_profile.coalesced_bytes,
    );
    println!(
        "{label}:critical setup_ms={} cache_open_ms={} scan_kdf_wall_ms={} plan_ms={} block_prepare_ms={} cache_commit_ms={} manifest_build_ms={} output_write_ms={} cleanup_ms={} unattributed_ms={} cache_index_format={} cache_index_open_ms={} cache_index_commit_ms={} cache_shards_read={} cache_shards_written={} cache_shard_dirty_count={} cache_commit_mode={} journal_upsert_records={} journal_upsert_paths={} journal_upsert_objects={} journal_upsert_sealed={} l1_index_hits={} l1_metadata_hits={} l1_scratch_reuses={} rayon_pool_reused={} daemon_used={} daemon_lookup_ms={} scheduler_queue_ms={} cpu_worker_wait_ms={} buffer_pool_hits={} buffer_pool_misses={} cache_pack_range_hits={} cache_pack_open_count={} hot_index_reuses={} hot_metadata_reuses={} pipeline_peak_memory_bytes={} header_bytes={} manifest_plain_bytes={} manifest_compressed_bytes={} manifest_protected_bytes={} payload_bytes={} compression_level_counts={:?} legacy_cache_hits={} parameterized_cache_hits={} cache_policy_misses={}",
        report.critical.setup_ms,
        report.critical.cache_open_ms,
        report.critical.scan_kdf_wall_ms,
        report.critical.plan_ms,
        report.critical.block_prepare_ms,
        report.critical.cache_commit_ms,
        report.critical.manifest_build_ms,
        report.critical.output_write_ms,
        report.critical.cleanup_ms,
        report.critical.unattributed_ms,
        report.l2.cache_index_format,
        report.l2.cache_index_open_ms,
        report.l2.cache_index_commit_ms,
        report.l2.cache_shards_read,
        report.l2.cache_shards_written,
        report.l2.cache_shard_dirty_count,
        report.l2.cache_commit_mode,
        report.l2.journal_upsert_records,
        report.l2.journal_upsert_paths,
        report.l2.journal_upsert_objects,
        report.l2.journal_upsert_sealed,
        report.l1.l1_index_hits,
        report.l1.l1_metadata_hits,
        report.l1.l1_scratch_reuses,
        report.l1.rayon_pool_reused,
        report.pipeline.daemon_used,
        report.pipeline.daemon_lookup_ms,
        report.pipeline.scheduler_queue_ms,
        report.pipeline.cpu_worker_wait_ms,
        report.pipeline.buffer_pool_hits,
        report.pipeline.buffer_pool_misses,
        report.pipeline.cache_pack_range_hits,
        report.pipeline.cache_pack_open_count,
        report.pipeline.hot_index_reuses,
        report.pipeline.hot_metadata_reuses,
        report.pipeline.pipeline_peak_memory_bytes,
        report.metadata.header_bytes,
        report.metadata.manifest_plain_bytes,
        report.metadata.manifest_compressed_bytes,
        report.metadata.manifest_protected_bytes,
        report.metadata.payload_bytes,
        report.blocks.compression_level_counts,
        report.blocks.legacy_cache_hits,
        report.blocks.parameterized_cache_hits,
        report.blocks.cache_policy_misses,
    );
    println!(
        "{label}:adaptive_io enabled={} initial_concurrency={} min_concurrency={} max_concurrency={} final_concurrency={} min_observed_concurrency={} max_observed_concurrency={} transitions={} constrained_entries={} recovery_steps={} normal_us={} constrained_us={} total_us={} final_constraint_stage={:?} final_constraint_direction={:?} scan_wall_us={} scan_read_us={} scan_content_hash_us={} stages={} transition_events={}",
        report.adaptive_io.enabled,
        report.adaptive_io.initial_concurrency,
        report.adaptive_io.min_concurrency,
        report.adaptive_io.max_concurrency,
        report.adaptive_io.final_concurrency,
        report.adaptive_io.min_observed_concurrency,
        report.adaptive_io.max_observed_concurrency,
        report.adaptive_io.transitions,
        report.adaptive_io.constrained_entries,
        report.adaptive_io.recovery_steps,
        report.adaptive_io.normal_us,
        report.adaptive_io.constrained_us,
        report.adaptive_io.total_us,
        report.adaptive_io.final_constraint_stage,
        report.adaptive_io.final_constraint_direction,
        report.scan.scan_wall_us,
        report.scan.read_us,
        report.scan.content_hash_us,
        serde_json::to_string(&report.adaptive_io.stages)?,
        serde_json::to_string(&report.adaptive_io.transition_events)?,
    );
    Ok(())
}

#[derive(Debug)]
struct BenchmarkRow {
    tool: String,
    input_bytes: u64,
    archive_bytes: Option<u64>,
    duration_ms: Option<u128>,
    cache_hit_rate: Option<f64>,
    scan_cache_hit_rate: Option<f64>,
    chunk_metadata_reuses: Option<usize>,
    trusted_bytes_skipped: Option<u64>,
    scan_ms: Option<u128>,
    plan_ms: Option<u128>,
    kdf_ms: Option<u128>,
    pack_blocks_ms: Option<u128>,
    speed: Option<String>,
    kdf_profile: Option<String>,
    encryption: Option<String>,
    worker_count: Option<usize>,
    kdf_overlapped_ms: Option<u128>,
    read_ms: Option<u128>,
    compression_ms: Option<u128>,
    crypto_ms: Option<u128>,
    payload_write_ms: Option<u128>,
    writer_strategy: Option<String>,
    archive_preallocated_bytes: Option<u64>,
    preallocation_enabled: Option<bool>,
    cached_payload_open_count: Option<usize>,
    cached_range_open_count: Option<usize>,
    cached_payload_read_bytes: Option<u64>,
    prefetched_bytes: Option<u64>,
    direct_write_count: Option<usize>,
    buffered_write_count: Option<usize>,
    peak_pipeline_memory_bytes: Option<u64>,
    payload_read_ms: Option<u128>,
    writer_wait_ms: Option<u128>,
    output_flush_ms: Option<u128>,
    output_rename_ms: Option<u128>,
    batch_blocks: Option<usize>,
    single_blocks: Option<usize>,
    batched_files: Option<usize>,
    chunked_files: Option<usize>,
    chunk_blocks: Option<usize>,
    chunk_cache_hits: Option<usize>,
    chunk_cache_misses: Option<usize>,
    chunk_plan_cache_hits: Option<usize>,
    chunk_plan_cache_misses: Option<usize>,
    sealed_block_hits: Option<usize>,
    sealed_block_misses: Option<usize>,
    sealed_bytes_reused: Option<u64>,
    reencrypted_cache_hits: Option<usize>,
    payload_source_cache_files: Option<usize>,
    payload_source_memory_bytes: Option<u64>,
    cache_pack_hits: Option<usize>,
    cache_pack_misses: Option<usize>,
    cache_pack_fallbacks: Option<usize>,
    session_used: Option<bool>,
    session_lookup_ms: Option<u128>,
    kdf_skipped_by_session: Option<bool>,
    solid_groups: Option<usize>,
    solid_files: Option<usize>,
    cache_index_format: Option<String>,
    cache_index_open_ms: Option<u128>,
    cache_index_commit_ms: Option<u128>,
    socket_connect_us: Option<u64>,
    socket_pack_roundtrip_us: Option<u64>,
    daemon_auth_us: Option<u64>,
    daemon_job_execute_us: Option<u64>,
    daemon_response_bytes: Option<u64>,
    response_serialize_us: Option<u64>,
    client_decode_us: Option<u64>,
    pipeline: Option<hig_core::PipelineReport>,
    notes: String,
}

#[derive(Debug)]
struct CompareOptions {
    input_dir: PathBuf,
    output: PathBuf,
    password: Option<String>,
    cache_dir: Option<PathBuf>,
    threads: Option<usize>,
    compression: Compression,
    level: Option<i32>,
    use_cache: bool,
    batch: BatchOptions,
    chunk: ChunkOptions,
    bench_dir: Option<PathBuf>,
    bench_suite: BenchSuite,
    manifest_format: ManifestFormat,
    use_session: bool,
    daemon: DaemonMode,
    solid: SolidMode,
    report_mode: ReportMode,
}

#[derive(Debug, Clone)]
struct CopyProbe {
    path: PathBuf,
    free_bytes: u64,
    used_percent: f64,
    copy_32_mib_median: f64,
    copy_32_mib_p95: f64,
    copy_256_mib_median: f64,
    copy_256_mib_p95: f64,
    qualified: bool,
}

#[derive(Debug, Clone)]
struct BenchmarkVolumeSelection {
    selected: CopyProbe,
    probes: Vec<CopyProbe>,
}

#[derive(Debug, Clone)]
struct AcceptanceSamples {
    standalone_second: SampleStats,
    pack_core: SampleStats,
    cli_wall: SampleStats,
    cli_wall_full: SampleStats,
    zip: Option<SampleStats>,
    runs: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SampleStats {
    median_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

impl SampleStats {
    fn from_values(values: &[f64]) -> Self {
        Self {
            median_ms: median(values),
            p95_ms: p95(values),
            p99_ms: percentile(values, 0.99),
            min_ms: values.iter().copied().fold(f64::INFINITY, f64::min),
            max_ms: values.iter().copied().fold(0.0, f64::max),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct WarmProjectSample {
    duration_us: u64,
    project_verify_us: u64,
    plan_us: u64,
    read_us: u64,
    compression_us: u64,
    crypto_us: u64,
    manifest_serialize_us: u64,
    manifest_compress_us: u64,
    manifest_encrypt_us: u64,
    output_create_us: u64,
    output_preallocate_us: u64,
    output_header_write_us: u64,
    output_manifest_write_us: u64,
    output_payload_read_us: u64,
    output_payload_write_us: u64,
    output_write_us: u64,
    output_flush_us: u64,
    output_fsync_us: u64,
    output_rename_us: u64,
    cache_commit_us: u64,
    unattributed_us: u64,
    prepared_object_hits: u64,
    prepared_object_misses: u64,
    cached_payload_open_count: u64,
    cached_payload_read_bytes: u64,
    payload_source_memory_bytes: u64,
}

impl WarmProjectSample {
    fn from_report(report: &PackReport) -> Self {
        Self {
            duration_us: report.duration.as_micros() as u64,
            project_verify_us: report.project.project_verify_us,
            plan_us: report.timings_us.plan_us,
            read_us: report.timings_us.read_us,
            compression_us: report.timings_us.compression_us,
            crypto_us: report.timings_us.crypto_us,
            manifest_serialize_us: report.timings_us.manifest_serialize_us,
            manifest_compress_us: report.timings_us.manifest_compress_us,
            manifest_encrypt_us: report.timings_us.manifest_encrypt_us,
            output_create_us: report.timings_us.output_create_us,
            output_preallocate_us: report.timings_us.output_preallocate_us,
            output_header_write_us: report.timings_us.output_header_write_us,
            output_manifest_write_us: report.timings_us.output_manifest_write_us,
            output_payload_read_us: report.timings_us.output_payload_read_us,
            output_payload_write_us: report.timings_us.output_payload_write_us,
            output_write_us: report.timings_us.output_write_us,
            output_flush_us: report.timings_us.output_flush_us,
            output_fsync_us: report.timings_us.output_fsync_us,
            output_rename_us: report.timings_us.output_rename_us,
            cache_commit_us: report.timings_us.cache_commit_us,
            unattributed_us: report.timings_us.unattributed_us,
            prepared_object_hits: report.project.project_prepared_object_hits,
            prepared_object_misses: report.project.project_prepared_object_misses,
            cached_payload_open_count: report.cached_payload_open_count as u64,
            cached_payload_read_bytes: report.cached_payload_read_bytes,
            payload_source_memory_bytes: report.blocks.payload_source_memory_bytes,
        }
    }

    fn stage_value(&self, stage: &str) -> u64 {
        match stage {
            "project_verify_us" => self.project_verify_us,
            "plan_us" => self.plan_us,
            "read_us" => self.read_us,
            "compression_us" => self.compression_us,
            "crypto_us" => self.crypto_us,
            "manifest_serialize_us" => self.manifest_serialize_us,
            "manifest_compress_us" => self.manifest_compress_us,
            "manifest_encrypt_us" => self.manifest_encrypt_us,
            "output_create_us" => self.output_create_us,
            "output_preallocate_us" => self.output_preallocate_us,
            "output_header_write_us" => self.output_header_write_us,
            "output_manifest_write_us" => self.output_manifest_write_us,
            "output_payload_read_us" => self.output_payload_read_us,
            "output_payload_write_us" => self.output_payload_write_us,
            "output_write_us" => self.output_write_us,
            "output_flush_us" => self.output_flush_us,
            "output_fsync_us" => self.output_fsync_us,
            "output_rename_us" => self.output_rename_us,
            "cache_commit_us" => self.cache_commit_us,
            "unattributed_us" => self.unattributed_us,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct WarmStageStats {
    stage: String,
    median_us: u64,
    p95_us: u64,
}

fn warm_stage_stats(samples: &[WarmProjectSample]) -> Vec<WarmStageStats> {
    let stages = [
        "project_verify_us",
        "plan_us",
        "read_us",
        "compression_us",
        "crypto_us",
        "manifest_serialize_us",
        "manifest_compress_us",
        "manifest_encrypt_us",
        "output_create_us",
        "output_preallocate_us",
        "output_header_write_us",
        "output_manifest_write_us",
        "output_payload_read_us",
        "output_payload_write_us",
        "output_write_us",
        "output_flush_us",
        "output_fsync_us",
        "output_rename_us",
        "cache_commit_us",
        "unattributed_us",
    ];
    stages
        .into_iter()
        .map(|stage| {
            let values = samples
                .iter()
                .map(|sample| sample.stage_value(stage) as f64)
                .collect::<Vec<_>>();
            WarmStageStats {
                stage: stage.to_string(),
                median_us: median(&values) as u64,
                p95_us: p95(&values) as u64,
            }
        })
        .collect()
}

fn top_warm_hotspots(stats: &[WarmStageStats], count: usize) -> Vec<WarmStageStats> {
    let mut stats = stats.to_vec();
    stats.sort_by_key(|stat| std::cmp::Reverse(stat.median_us));
    stats.truncate(count);
    stats
}

#[derive(Debug, serde::Serialize)]
struct BenchmarkSummary {
    version: &'static str,
    input_dir: String,
    corpus_name: String,
    bench_suite: String,
    file_count: u64,
    input_bytes: u64,
    excluded_paths: Vec<String>,
    incremental_modified_files: Vec<String>,
    solid_groups: u64,
    solid_files: u64,
    cache_policy_misses: u64,
    journal_bytes_after: u64,
    journal_entries_after: u64,
    environment_status: String,
    release_gate_status: String,
    fastest_available_volume: String,
    selected_volume_path: String,
    selected_volume_copy_mib_s: f64,
    workspace_volume_copy_mib_s: Option<f64>,
    volume_probes: Vec<VolumeProbeSummary>,
    io_hotspot_summary: String,
    copy_256_mib_median_mib_s: f64,
    copy_256_mib_p95_mib_s: f64,
    pack_core_gate: bool,
    cli_wall_gate: bool,
    size_quality_gate: bool,
    hig_pack_core: SampleStats,
    hig_standalone_second: SampleStats,
    hig_cli_wall: SampleStats,
    hig_cli_wall_full: SampleStats,
    zip_cli_wall: Option<SampleStats>,
    rows: Vec<BenchmarkSummaryRow>,
    incremental: Option<IncrementalSummary>,
}

#[derive(Debug, serde::Serialize)]
struct VolumeProbeSummary {
    path: String,
    free_bytes: u64,
    used_percent: f64,
    copy_32_mib_median_mib_s: f64,
    copy_32_mib_p95_mib_s: f64,
    copy_256_mib_median_mib_s: f64,
    copy_256_mib_p95_mib_s: f64,
    qualified: bool,
}

#[derive(Debug, serde::Serialize)]
struct BenchmarkSummaryRow {
    tool: String,
    input_bytes: u64,
    archive_bytes: Option<u64>,
    duration_ms: Option<u128>,
    socket_connect_us: Option<u64>,
    socket_pack_roundtrip_us: Option<u64>,
    daemon_auth_us: Option<u64>,
    daemon_job_execute_us: Option<u64>,
    daemon_response_bytes: Option<u64>,
    response_serialize_us: Option<u64>,
    client_decode_us: Option<u64>,
    cache_index_commit_ms: Option<u128>,
    notes: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct IncrementalSummary {
    modified_files: Vec<String>,
    hig_pack_core_ms: f64,
    hig_cli_wall_ms: f64,
    zip_cli_wall_ms: Option<f64>,
    cache_hit_rate: f64,
    cache_policy_misses: u64,
    solid_groups: u64,
    solid_files: u64,
    journal_bytes_after: u64,
    journal_entries_after: u64,
}

fn run_compare(options: CompareOptions) -> anyhow::Result<()> {
    let source_input_dir = options.input_dir.canonicalize()?;
    let volume_selection =
        select_benchmark_volume(options.bench_dir.as_deref(), options.output.parent())?;
    let probe = volume_selection.selected.clone();
    let work_dir = benchmark_work_dir(Some(&probe.path))?;
    fs::create_dir_all(work_dir.join("hig-out"))?;
    fs::create_dir_all(work_dir.join("zip-out"))?;
    fs::create_dir_all(work_dir.join("gzip-out"))?;
    fs::create_dir_all(work_dir.join("zstd-out"))?;
    fs::create_dir_all(work_dir.join("datasets"))?;
    if options.bench_suite == BenchSuite::LobehubWatch {
        return run_lobehub_watch_compare(
            &options,
            &source_input_dir,
            &probe,
            &volume_selection.probes,
            &work_dir,
        );
    }
    let input_dir =
        materialize_bench_suite_input(options.bench_suite, &source_input_dir, &work_dir)?;
    let input_bytes = dir_size(&input_dir)?;
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| work_dir.join("hig-cache"));
    let compare_invoked_with_session = options.use_session;
    let mut rows = Vec::new();

    let first = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.clone(),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report("bench:higv2:batch:first", &first, ReportMode::Quiet)?;
    rows.push(row_from_report(
        "higv2 balanced first",
        &first,
        "default HIGV2 batch/chunk format",
    ));

    let second = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("second.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report("bench:higv2:batch:second", &second, ReportMode::Quiet)?;
    rows.push(row_from_report(
        "higv2 balanced second",
        &second,
        "reuses batch/single/chunk cache but recomputes file hashes",
    ));

    let session_unlock_started = Instant::now();
    if let Some(password) = options.password.as_deref() {
        unlock_session_for_cache(&cache_dir, password, KdfProfile::Secure, 1_800)?;
    }
    let session_unlock_ms = session_unlock_started.elapsed().as_millis();
    let session_pack = pack_with_daemon_policy(
        PackOptions {
            input_dir: input_dir.clone(),
            output_file: options.output.with_extension("session.hig"),
            password: None,
            encryption: EncryptionMode::Password,
            cache_dir: Some(cache_dir.clone()),
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: options.use_cache,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: options.batch,
            chunk: options.chunk,
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: options.manifest_format,
            use_session: true,
            session_required: true,
            session_ttl_secs: None,
            solid: options.solid,
            pipeline: PipelineOptions {
                daemon_mode: DaemonMode::Required,
                ..PipelineOptions::default()
            },
        },
        ReportMode::Json,
    )?;
    print_report(
        "bench:higv2:balanced:session",
        &session_pack,
        ReportMode::Quiet,
    )?;
    rows.push(row_from_report(
        "higv2 balanced secure session",
        &session_pack,
        &format!("secure session pack; unlock cost {session_unlock_ms} ms reported separately"),
    ));
    if compare_invoked_with_session {
        println!("benchmark: --use-session was set; compare still reports unlock cost separately");
    }

    let daemon_pack = pack_with_daemon_policy(
        PackOptions {
            input_dir: input_dir.clone(),
            output_file: options.output.with_extension("daemon.hig"),
            password: None,
            encryption: EncryptionMode::Password,
            cache_dir: Some(cache_dir.clone()),
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: options.use_cache,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: options.batch,
            chunk: options.chunk,
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: options.manifest_format,
            use_session: true,
            session_required: true,
            session_ttl_secs: None,
            solid: options.solid,
            pipeline: PipelineOptions {
                daemon_mode: options.daemon,
                ..PipelineOptions::default()
            },
        },
        ReportMode::Json,
    )?;
    print_report(
        "bench:higv2:balanced:daemon",
        &daemon_pack,
        ReportMode::Quiet,
    )?;
    rows.push(row_from_report(
        "higv2 balanced secure daemon",
        &daemon_pack,
        "secure hot daemon/session path; KDF skipped and cache index is warm",
    ));

    let trusted = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("trusted.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: true,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Fastest,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: true,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report(
        "bench:higv2:fastest:secure:warm",
        &trusted,
        ReportMode::Quiet,
    )?;
    let fastest_secure = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("fastest.secure.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: true,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Fastest,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: true,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report(
        "bench:higv2:fastest:secure",
        &fastest_secure,
        ReportMode::Quiet,
    )?;
    rows.push(row_from_report(
        "higv2 fastest secure",
        &fastest_secure,
        "fastest mode with secure KDF and sealed block reuse",
    ));

    let fastest_interactive_warm = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("fastest.interactive.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: true,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Fastest,
        kdf_profile: KdfProfile::Interactive,
        sealed_cache: true,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report(
        "bench:higv2:fastest:interactive:warm",
        &fastest_interactive_warm,
        ReportMode::Quiet,
    )?;
    let fastest_interactive = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options
            .output
            .with_extension("fastest.interactive.second.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: true,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Fastest,
        kdf_profile: KdfProfile::Interactive,
        sealed_cache: true,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report(
        "bench:higv2:fastest:interactive",
        &fastest_interactive,
        ReportMode::Quiet,
    )?;
    rows.push(row_from_report(
        "higv2 fastest interactive",
        &fastest_interactive,
        "explicit fastest mode; metadata trust and sealed encrypted cache enabled",
    ));

    let fastest_bench = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("fastest.fast-bench.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: true,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Fastest,
        kdf_profile: KdfProfile::FastBench,
        sealed_cache: true,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report(
        "bench:higv2:fastest:fast-bench",
        &fastest_bench,
        ReportMode::Quiet,
    )?;
    rows.push(row_from_report(
        "higv2 fastest second --kdf-profile fast-bench",
        &fastest_bench,
        "fastest mode with benchmark-only KDF profile",
    ));

    let no_batch = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv2.no-batch.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(work_dir.join("higv2-no-batch-cache")),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: BatchOptions {
            enabled: false,
            ..options.batch
        },
        chunk: options.chunk,
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report("bench:higv2:no-batch", &no_batch, ReportMode::Quiet)?;
    rows.push(row_from_report(
        "higv2 --no-batch",
        &no_batch,
        "HIGV2 with batching disabled",
    ));

    let no_chunk_cache = work_dir.join("higv2-no-chunk-cache");
    let no_chunk = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv2.no-chunk.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(no_chunk_cache.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: ChunkOptions {
            enabled: false,
            ..options.chunk
        },
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report("bench:higv2:no-chunk", &no_chunk, ReportMode::Quiet)?;
    rows.push(row_from_report(
        "higv2 --no-chunk first",
        &no_chunk,
        "HIGV2 with large-file chunking disabled",
    ));

    let no_chunk_second = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv2.no-chunk.second.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(no_chunk_cache),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: ChunkOptions {
            enabled: false,
            ..options.chunk
        },
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report(
        "bench:higv2:no-chunk:second",
        &no_chunk_second,
        ReportMode::Quiet,
    )?;
    rows.push(row_from_report(
        "higv2 --no-chunk second",
        &no_chunk_second,
        "HIGV2 second pack with large-file chunking disabled",
    ));

    let no_encryption = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("none.hig"),
        password: None,
        encryption: EncryptionMode::None,
        cache_dir: Some(work_dir.join("higv2-none-cache")),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report("bench:higv2:none", &no_encryption, ReportMode::Quiet)?;
    rows.push(row_from_report(
        "higv2 no-encryption",
        &no_encryption,
        "no confidentiality or AEAD; BLAKE3 corruption checks remain",
    ));

    let legacy = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv1.legacy.hig"),
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(work_dir.join("higv1-cache")),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV1,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions::default(),
    })?;
    print_report("bench:higv1:legacy", &legacy, ReportMode::Quiet)?;
    rows.push(row_from_report(
        "higv1 legacy",
        &legacy,
        "legacy one-file-per-block format",
    ));

    rows.push(run_zip_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_tar_gzip_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_tar_zstd_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_7z_benchmark(&input_dir, input_bytes));

    let acceptance = run_acceptance_samples(
        &input_dir,
        &work_dir,
        &cache_dir,
        &options,
        command_exists("zip"),
    )?;
    let incremental = if options.bench_suite == BenchSuite::Lobehub {
        Some(run_lobehub_incremental(
            &input_dir,
            &work_dir,
            &cache_dir,
            &options,
            command_exists("zip"),
        )?)
    } else {
        None
    };
    let cache_status = request_daemon(&cache_dir, DaemonRequest::CacheStatus)?.and_then(
        |response| match response {
            DaemonResponse::CacheMaintenance(report) => Some(report),
            _ => None,
        },
    );
    let summary = build_benchmark_summary(
        &input_dir,
        &rows,
        &volume_selection,
        &acceptance,
        &options,
        incremental.clone(),
        cache_status.as_ref(),
    );
    let markdown = render_markdown(&input_dir, &rows, &probe, &acceptance, &summary);
    fs::create_dir_all("artifacts")?;
    let (benchmark_path, summary_path, profile_path) = artifact_paths(options.bench_suite);
    fs::write(&benchmark_path, markdown)?;
    fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)?;
    if options.bench_suite == BenchSuite::Lobehub {
        fs::write(
            &profile_path,
            render_lobehub_profile(&input_dir, &rows, &probe, &summary),
        )?;
    }
    if options.report_mode == ReportMode::Json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("benchmark: wrote {}", benchmark_path.display());
    }
    let _ = request_daemon(&cache_dir, DaemonRequest::ClearSession);
    Ok(())
}

fn build_benchmark_summary(
    input_dir: &Path,
    rows: &[BenchmarkRow],
    volume_selection: &BenchmarkVolumeSelection,
    acceptance: &AcceptanceSamples,
    options: &CompareOptions,
    incremental: Option<IncrementalSummary>,
    cache_status: Option<&hig_core::CacheMaintenanceReport>,
) -> BenchmarkSummary {
    let probe = &volume_selection.selected;
    let volume_probes = &volume_selection.probes;
    let hig_size = rows
        .iter()
        .find(|row| row.tool == "higv2 balanced secure daemon")
        .and_then(|row| row.archive_bytes);
    let zip_size = rows
        .iter()
        .find(|row| row.tool == "zip")
        .and_then(|row| row.archive_bytes);
    let size_quality_gate = match (hig_size, zip_size) {
        (Some(hig), Some(zip)) => hig <= zip,
        _ => false,
    };
    let pack_core_gate = if options.bench_suite == BenchSuite::Lobehub {
        acceptance.zip.as_ref().is_some_and(|zip| {
            acceptance.pack_core.median_ms <= 600.0
                && acceptance.pack_core.median_ms < zip.median_ms
        })
    } else {
        acceptance.pack_core.median_ms < 3.0
    };
    let cli_wall_gate = acceptance.zip.as_ref().is_some_and(|zip| {
        acceptance.cli_wall.median_ms <= 750.0 && acceptance.cli_wall.median_ms < zip.median_ms
    });
    let release_gate_status =
        release_gate_status(probe, pack_core_gate, cli_wall_gate, size_quality_gate);
    let fastest_available_volume = volume_probes
        .iter()
        .max_by(|left, right| {
            left.copy_256_mib_median
                .partial_cmp(&right.copy_256_mib_median)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|probe| probe.path.display().to_string())
        .unwrap_or_else(|| probe.path.display().to_string());
    let workspace_volume_copy_mib_s = std::env::current_dir().ok().and_then(|cwd| {
        volume_probes
            .iter()
            .find(|probe| paths_on_same_volume(&probe.path, &cwd))
            .map(|probe| probe.copy_256_mib_median)
    });
    BenchmarkSummary {
        version: "1.9.7",
        input_dir: input_dir.display().to_string(),
        corpus_name: match options.bench_suite {
            BenchSuite::Lobehub => "lobehub-source".to_string(),
            other => format!("{other:?}"),
        },
        bench_suite: format!("{:?}", options.bench_suite),
        file_count: count_files(input_dir).unwrap_or_default(),
        input_bytes: dir_size(input_dir).unwrap_or_default(),
        excluded_paths: excluded_paths_for_suite(options.bench_suite),
        incremental_modified_files: incremental
            .as_ref()
            .map(|summary| summary.modified_files.clone())
            .unwrap_or_default(),
        solid_groups: rows
            .iter()
            .find(|row| row.tool == "higv2 balanced secure daemon")
            .and_then(|row| row.solid_groups)
            .unwrap_or_default() as u64,
        solid_files: rows
            .iter()
            .find(|row| row.tool == "higv2 balanced secure daemon")
            .and_then(|row| row.solid_files)
            .unwrap_or_default() as u64,
        cache_policy_misses: 0,
        journal_bytes_after: cache_status
            .map(|report| report.journal_bytes)
            .unwrap_or_default(),
        journal_entries_after: cache_status
            .map(|report| report.journal_entries)
            .unwrap_or_default(),
        environment_status: if probe.qualified {
            "QUALIFIED".to_string()
        } else {
            "ENVIRONMENT_NOT_QUALIFIED".to_string()
        },
        release_gate_status,
        fastest_available_volume,
        selected_volume_path: probe.path.display().to_string(),
        selected_volume_copy_mib_s: probe.copy_256_mib_median,
        workspace_volume_copy_mib_s,
        volume_probes: volume_probes.iter().map(VolumeProbeSummary::from).collect(),
        io_hotspot_summary: io_hotspot_summary(rows, probe),
        copy_256_mib_median_mib_s: probe.copy_256_mib_median,
        copy_256_mib_p95_mib_s: probe.copy_256_mib_p95,
        pack_core_gate,
        cli_wall_gate,
        size_quality_gate,
        hig_pack_core: acceptance.pack_core.clone(),
        hig_standalone_second: acceptance.standalone_second.clone(),
        hig_cli_wall: acceptance.cli_wall.clone(),
        hig_cli_wall_full: acceptance.cli_wall_full.clone(),
        zip_cli_wall: acceptance.zip.clone(),
        rows: rows
            .iter()
            .map(|row| BenchmarkSummaryRow {
                tool: row.tool.clone(),
                input_bytes: row.input_bytes,
                archive_bytes: row.archive_bytes,
                duration_ms: row.duration_ms,
                socket_connect_us: row.socket_connect_us,
                socket_pack_roundtrip_us: row.socket_pack_roundtrip_us,
                daemon_auth_us: row.daemon_auth_us,
                daemon_job_execute_us: row.daemon_job_execute_us,
                daemon_response_bytes: row.daemon_response_bytes,
                response_serialize_us: row.response_serialize_us,
                client_decode_us: row.client_decode_us,
                cache_index_commit_ms: row.cache_index_commit_ms,
                notes: row.notes.clone(),
            })
            .collect(),
        incremental,
    }
}

fn artifact_paths(suite: BenchSuite) -> (PathBuf, PathBuf, PathBuf) {
    if suite == BenchSuite::Lobehub {
        (
            PathBuf::from("artifacts/hig-v1.9.7-lobehub-benchmark.md"),
            PathBuf::from("artifacts/hig-v1.9.7-lobehub-summary.json"),
            PathBuf::from("artifacts/hig-v1.9.7-lobehub-profile.md"),
        )
    } else {
        (
            PathBuf::from("artifacts/hig-v1.9.7-benchmark.md"),
            PathBuf::from("artifacts/hig-v1.9.7-summary.json"),
            PathBuf::from("artifacts/hig-v1.9.7-profile.md"),
        )
    }
}

impl From<&CopyProbe> for VolumeProbeSummary {
    fn from(probe: &CopyProbe) -> Self {
        Self {
            path: probe.path.display().to_string(),
            free_bytes: probe.free_bytes,
            used_percent: probe.used_percent,
            copy_32_mib_median_mib_s: probe.copy_32_mib_median,
            copy_32_mib_p95_mib_s: probe.copy_32_mib_p95,
            copy_256_mib_median_mib_s: probe.copy_256_mib_median,
            copy_256_mib_p95_mib_s: probe.copy_256_mib_p95,
            qualified: probe.qualified,
        }
    }
}

fn release_gate_status(
    probe: &CopyProbe,
    pack_core_gate: bool,
    cli_wall_gate: bool,
    size_quality_gate: bool,
) -> String {
    if pack_core_gate && cli_wall_gate && size_quality_gate {
        "PASS".to_string()
    } else if !probe.qualified && size_quality_gate {
        "NOT_ABSOLUTE_PASS_ENV_UNQUALIFIED".to_string()
    } else {
        "FAILED_QUALIFIED_VOLUME".to_string()
    }
}

fn io_hotspot_summary(rows: &[BenchmarkRow], probe: &CopyProbe) -> String {
    let daemon_row = rows
        .iter()
        .find(|row| row.tool == "higv2 balanced secure daemon")
        .or_else(|| {
            rows.iter()
                .find(|row| row.tool.starts_with("higv2 balanced"))
        });
    match daemon_row {
        Some(_) if !probe.qualified => format!(
            "selected volume is not qualified ({:.2} MiB/s median 256MiB copy); benchmark hot path is expected to be dominated by output write/flush when archive payloads are already prepared",
            probe.copy_256_mib_median
        ),
        Some(row) => format!(
            "selected volume is qualified; daemon duration={:?}ms cache_commit={:?}ms, investigate output write/flush only if project warm gate fails",
            row.duration_ms, row.cache_index_commit_ms
        ),
        None => format!(
            "selected volume median 256MiB copy is {:.2} MiB/s; no daemon row was available for hotspot attribution",
            probe.copy_256_mib_median
        ),
    }
}

fn paths_on_same_volume(left: &Path, right: &Path) -> bool {
    let Ok(left) = fs::metadata(left) else {
        return false;
    };
    let Ok(right) = fs::metadata(right) else {
        return false;
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev()
    }

    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false
    }
}

fn run_lobehub_incremental(
    input_dir: &Path,
    work_dir: &Path,
    cache_dir: &Path,
    options: &CompareOptions,
    zip_available: bool,
) -> anyhow::Result<IncrementalSummary> {
    let incremental_dir = recreate_dataset_dir(work_dir, "lobehub-incremental")?;
    copy_dir_filtered(input_dir, &incremental_dir, &[])?;
    let modified_files = apply_lobehub_incremental_changes(&incremental_dir)?;
    let output = work_dir.join("hig-out").join("lobehub-incremental.hig");
    let report = pack_with_daemon_policy(
        PackOptions {
            input_dir: incremental_dir.clone(),
            output_file: output.clone(),
            password: None,
            encryption: EncryptionMode::Password,
            cache_dir: Some(cache_dir.to_path_buf()),
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: options.use_cache,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: options.batch,
            chunk: options.chunk,
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: options.manifest_format,
            use_session: true,
            session_required: true,
            session_ttl_secs: None,
            solid: options.solid,
            pipeline: PipelineOptions {
                daemon_mode: DaemonMode::Required,
                ..PipelineOptions::default()
            },
        },
        ReportMode::Json,
    )?;
    let cli_started = Instant::now();
    let cli_output = work_dir.join("hig-out").join("lobehub-incremental-cli.hig");
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .arg("pack")
        .arg(&incremental_dir)
        .arg("-o")
        .arg(cli_output)
        .arg("--use-session")
        .arg("--daemon")
        .arg("required")
        .arg("--cache-dir")
        .arg(cache_dir)
        .arg("--manifest-format")
        .arg(match options.manifest_format {
            ManifestFormat::Compact => "compact",
            ManifestFormat::Legacy => "legacy",
        })
        .arg("--solid")
        .arg(match options.solid {
            SolidMode::Auto => "auto",
            SolidMode::Off => "off",
        })
        .arg("--quiet")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(threads) = options.threads {
        command.arg("--threads").arg(threads.to_string());
    }
    let status = command.status()?;
    anyhow::ensure!(status.success(), "lobehub incremental CLI pack failed");
    let hig_cli_wall_ms = cli_started.elapsed().as_secs_f64() * 1000.0;
    let zip_cli_wall_ms = if zip_available {
        let output = work_dir.join("zip-out").join("lobehub-incremental.zip");
        let started = Instant::now();
        let status = ProcessCommand::new("zip")
            .arg("-qr")
            .arg(output)
            .arg(".")
            .arg("-x")
            .arg(".hig-cache/*")
            .current_dir(&incremental_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        anyhow::ensure!(status.success(), "lobehub incremental zip failed");
        Some(started.elapsed().as_secs_f64() * 1000.0)
    } else {
        None
    };
    let cache_status = request_daemon(cache_dir, DaemonRequest::CacheStatus)?.and_then(
        |response| match response {
            DaemonResponse::CacheMaintenance(report) => Some(report),
            _ => None,
        },
    );
    Ok(IncrementalSummary {
        modified_files,
        hig_pack_core_ms: report.duration.as_secs_f64() * 1000.0,
        hig_cli_wall_ms,
        zip_cli_wall_ms,
        cache_hit_rate: report.cache.hit_rate() * 100.0,
        cache_policy_misses: report.blocks.cache_policy_misses as u64,
        solid_groups: report.blocks.solid_groups as u64,
        solid_files: report.blocks.solid_files as u64,
        journal_bytes_after: cache_status
            .as_ref()
            .map(|report| report.journal_bytes)
            .unwrap_or_default(),
        journal_entries_after: cache_status
            .as_ref()
            .map(|report| report.journal_entries)
            .unwrap_or_default(),
    })
}

fn apply_lobehub_incremental_changes(root: &Path) -> anyhow::Result<Vec<String>> {
    let mut modified = Vec::new();
    let mut source_candidates = collect_files_with_extensions(
        root,
        &["ts", "tsx", "js", "jsx", "md", "json"],
        &["src", "packages", "apps"],
    )?;
    source_candidates.sort();
    for path in source_candidates.into_iter().take(3) {
        append_text(&path, "\n// hig deterministic incremental source change\n")?;
        modified.push(relative_string(root, &path));
    }
    if let Some(path) = collect_files_with_extensions(root, &["json", "ts", "md"], &["locales"])?
        .into_iter()
        .min()
    {
        append_text(&path, "\n{\"higIncremental\":\"locale\"}\n")?;
        modified.push(relative_string(root, &path));
    }
    if let Some(path) = collect_files_with_extensions(
        root,
        &["png", "jpg", "jpeg", "webp", "svg", "txt", "json"],
        &["public"],
    )?
    .into_iter()
    .min()
    {
        append_text(&path, "\nHIG_INCREMENTAL_PUBLIC_ASSET\n")?;
        modified.push(relative_string(root, &path));
    }
    anyhow::ensure!(
        !modified.is_empty(),
        "lobehub incremental scenario found no files to modify"
    );
    Ok(modified)
}

fn append_text(path: &Path, text: &str) -> anyhow::Result<()> {
    let mut file = fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn collect_files_with_extensions(
    root: &Path,
    extensions: &[&str],
    top_level_dirs: &[&str],
) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for dir in top_level_dirs {
        let path = root.join(dir);
        if path.exists() {
            collect_files_recursive(&path, extensions, &mut files)?;
        }
    }
    Ok(files)
}

fn collect_files_recursive(
    root: &Path,
    extensions: &[&str],
    files: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_files_recursive(&path, extensions, files)?;
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.iter().any(|value| value == &extension))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn relative_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn excluded_paths_for_suite(suite: BenchSuite) -> Vec<String> {
    if suite == BenchSuite::Lobehub {
        [
            ".git",
            ".hig-cache",
            "node_modules",
            ".next",
            "dist",
            "build",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    } else {
        vec![".git".to_string(), ".hig-cache".to_string()]
    }
}

fn run_acceptance_samples(
    input_dir: &Path,
    work_dir: &Path,
    cache_dir: &Path,
    options: &CompareOptions,
    zip_available: bool,
) -> anyhow::Result<AcceptanceSamples> {
    const RUNS: usize = 20;
    let mut standalone_samples = Vec::with_capacity(RUNS);
    let mut pack_core_samples = Vec::with_capacity(RUNS);
    let mut cli_wall_samples = Vec::with_capacity(RUNS);
    let mut cli_wall_full_samples = Vec::with_capacity(RUNS);
    let mut zip_samples = Vec::with_capacity(RUNS);
    let executable = std::env::current_exe()?;
    let standalone_cache_dir = work_dir.join("acceptance-standalone-cache");
    let run_standalone = |run: usize, warmup: bool| -> anyhow::Result<f64> {
        let output = work_dir.join(if warmup {
            "acceptance-standalone-warmup.hig".to_string()
        } else {
            format!("acceptance-standalone-{run}.hig")
        });
        let report = pack(PackOptions {
            input_dir: input_dir.to_path_buf(),
            output_file: output.clone(),
            password: options.password.clone(),
            encryption: EncryptionMode::Password,
            cache_dir: Some(standalone_cache_dir.clone()),
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: options.use_cache,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: options.batch,
            chunk: options.chunk,
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: options.manifest_format,
            use_session: false,
            session_required: false,
            session_ttl_secs: None,
            solid: options.solid,
            pipeline: PipelineOptions {
                daemon_mode: DaemonMode::Off,
                ..PipelineOptions::default()
            },
        })?;
        let duration_ms = report.duration.as_secs_f64() * 1000.0;
        let _ = fs::remove_file(&output);
        Ok(duration_ms)
    };
    let run_pack_core = |run: usize, warmup: bool| -> anyhow::Result<f64> {
        let output = work_dir.join(if warmup {
            "acceptance-core-warmup.hig".to_string()
        } else {
            format!("acceptance-core-{run}.hig")
        });
        let report = pack_with_daemon_policy(
            PackOptions {
                input_dir: input_dir.to_path_buf(),
                output_file: output.clone(),
                password: None,
                encryption: EncryptionMode::Password,
                cache_dir: Some(cache_dir.to_path_buf()),
                threads: options.threads,
                compression: options.compression,
                level: options.level,
                use_cache: options.use_cache,
                trust_metadata: false,
                format: ArchiveFormat::HigV2,
                batch: options.batch,
                chunk: options.chunk,
                speed: SpeedMode::Balanced,
                kdf_profile: KdfProfile::Secure,
                sealed_cache: false,
                manifest_format: options.manifest_format,
                use_session: true,
                session_required: true,
                session_ttl_secs: None,
                solid: options.solid,
                pipeline: PipelineOptions {
                    daemon_mode: DaemonMode::Required,
                    ..PipelineOptions::default()
                },
            },
            ReportMode::Json,
        )?;
        let duration_ms = report.duration.as_secs_f64() * 1000.0;
        let _ = fs::remove_file(&output);
        Ok(duration_ms)
    };
    let run_cli_wall = |run: usize, warmup: bool, mode: ReportMode| -> anyhow::Result<f64> {
        let output = work_dir.join(if warmup {
            format!("acceptance-cli-{mode:?}-warmup.hig")
        } else {
            format!("acceptance-cli-{mode:?}-{run}.hig")
        });
        let started = Instant::now();
        let mut command = ProcessCommand::new(&executable);
        command
            .arg("pack")
            .arg(input_dir)
            .arg("-o")
            .arg(&output)
            .arg("--use-session")
            .arg("--daemon")
            .arg("required")
            .arg("--cache-dir")
            .arg(cache_dir)
            .arg("--manifest-format")
            .arg(match options.manifest_format {
                ManifestFormat::Compact => "compact",
                ManifestFormat::Legacy => "legacy",
            })
            .arg("--solid")
            .arg(match options.solid {
                SolidMode::Auto => "auto",
                SolidMode::Off => "off",
            })
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.arg(match mode {
            ReportMode::Json => "--json",
            ReportMode::Quiet => "--quiet",
            _ => unreachable!("acceptance CLI-wall supports quiet or JSON only"),
        });
        if let Some(threads) = options.threads {
            command.arg("--threads").arg(threads.to_string());
        }
        let status = command.status()?;
        anyhow::ensure!(status.success(), "independent daemon pack benchmark failed");
        let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        let _ = fs::remove_file(&output);
        Ok(duration_ms)
    };
    let _ = run_standalone(0, true)?;
    for run in 0..RUNS {
        standalone_samples.push(run_standalone(run, false)?);
    }
    let _ = run_pack_core(0, true)?;
    for run in 0..RUNS {
        pack_core_samples.push(run_pack_core(run, false)?);
    }
    let _ = run_cli_wall(0, true, ReportMode::Quiet)?;
    let _ = run_cli_wall(0, true, ReportMode::Json)?;
    for run in 0..RUNS {
        if run % 2 == 0 {
            cli_wall_samples.push(run_cli_wall(run, false, ReportMode::Quiet)?);
            cli_wall_full_samples.push(run_cli_wall(run, false, ReportMode::Json)?);
        } else {
            cli_wall_full_samples.push(run_cli_wall(run, false, ReportMode::Json)?);
            cli_wall_samples.push(run_cli_wall(run, false, ReportMode::Quiet)?);
        }
    }

    if zip_available {
        let warmup = work_dir.join("acceptance-zip-warmup.zip");
        let status = ProcessCommand::new("zip")
            .arg("-qr")
            .arg(&warmup)
            .arg(".")
            .arg("-x")
            .arg(".hig-cache/*")
            .current_dir(input_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        anyhow::ensure!(status.success(), "zip warmup benchmark failed");
        let _ = fs::remove_file(&warmup);
        for run in 0..RUNS {
            let output = work_dir.join(format!("acceptance-{run}.zip"));
            let started = Instant::now();
            let status = ProcessCommand::new("zip")
                .arg("-qr")
                .arg(&output)
                .arg(".")
                .arg("-x")
                .arg(".hig-cache/*")
                .current_dir(input_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            anyhow::ensure!(status.success(), "zip acceptance benchmark failed");
            zip_samples.push(started.elapsed().as_secs_f64() * 1000.0);
            let _ = fs::remove_file(&output);
        }
    }
    let zip = (!zip_samples.is_empty()).then(|| SampleStats::from_values(&zip_samples));
    Ok(AcceptanceSamples {
        standalone_second: SampleStats::from_values(&standalone_samples),
        pack_core: SampleStats::from_values(&pack_core_samples),
        cli_wall: SampleStats::from_values(&cli_wall_samples),
        cli_wall_full: SampleStats::from_values(&cli_wall_full_samples),
        zip,
        runs: RUNS,
    })
}

fn row_from_report(tool: &str, report: &PackReport, notes: &str) -> BenchmarkRow {
    BenchmarkRow {
        tool: tool.to_string(),
        input_bytes: report.input_bytes,
        archive_bytes: Some(report.archive_bytes),
        duration_ms: Some(report.duration.as_millis()),
        cache_hit_rate: Some(report.cache.hit_rate() * 100.0),
        scan_cache_hit_rate: Some(report.scan.scan_cache_hit_rate() * 100.0),
        chunk_metadata_reuses: Some(report.scan.chunk_metadata_reuses),
        trusted_bytes_skipped: Some(report.scan.trusted_bytes_skipped),
        scan_ms: Some(report.timings.scan_ms),
        plan_ms: Some(report.timings.plan_ms),
        kdf_ms: Some(report.timings.kdf_ms),
        pack_blocks_ms: Some(report.timings.pack_blocks_ms),
        speed: Some(format!("{:?}", report.speed)),
        kdf_profile: Some(format!("{:?}", report.kdf_profile)),
        encryption: Some(format!("{:?}", report.encryption_mode)),
        worker_count: Some(report.worker_count),
        kdf_overlapped_ms: Some(report.timings.kdf_overlapped_ms),
        read_ms: Some(report.timings.read_ms),
        compression_ms: Some(report.timings.compression_ms),
        crypto_ms: Some(report.timings.crypto_ms),
        payload_write_ms: Some(report.timings.payload_write_ms),
        writer_strategy: Some(format!("{:?}", report.writer_strategy)),
        archive_preallocated_bytes: Some(report.archive_preallocated_bytes),
        preallocation_enabled: Some(report.preallocation_enabled),
        cached_payload_open_count: Some(report.cached_payload_open_count),
        cached_range_open_count: Some(report.cached_range_open_count),
        cached_payload_read_bytes: Some(report.cached_payload_read_bytes),
        prefetched_bytes: Some(report.prefetched_bytes),
        direct_write_count: Some(report.direct_write_count),
        buffered_write_count: Some(report.buffered_write_count),
        peak_pipeline_memory_bytes: Some(report.peak_pipeline_memory_bytes),
        payload_read_ms: Some(report.timings.payload_read_ms),
        writer_wait_ms: Some(report.timings.writer_wait_ms),
        output_flush_ms: Some(report.timings.output_flush_ms),
        output_rename_ms: Some(report.timings.output_rename_ms),
        batch_blocks: Some(report.blocks.batch_blocks),
        single_blocks: Some(report.blocks.single_blocks),
        batched_files: Some(report.blocks.batched_files),
        chunked_files: Some(report.blocks.chunked_files),
        chunk_blocks: Some(report.blocks.chunk_blocks),
        chunk_cache_hits: Some(report.blocks.chunk_cache_hits),
        chunk_cache_misses: Some(report.blocks.chunk_cache_misses),
        chunk_plan_cache_hits: Some(report.blocks.chunk_plan_cache_hits),
        chunk_plan_cache_misses: Some(report.blocks.chunk_plan_cache_misses),
        sealed_block_hits: Some(report.blocks.sealed_block_hits),
        sealed_block_misses: Some(report.blocks.sealed_block_misses),
        sealed_bytes_reused: Some(report.blocks.sealed_bytes_reused),
        reencrypted_cache_hits: Some(report.blocks.reencrypted_cache_hits),
        payload_source_cache_files: Some(report.blocks.payload_source_cache_files),
        payload_source_memory_bytes: Some(report.blocks.payload_source_memory_bytes),
        cache_pack_hits: Some(report.blocks.cache_pack_hits),
        cache_pack_misses: Some(report.blocks.cache_pack_misses),
        cache_pack_fallbacks: Some(report.blocks.cache_pack_fallbacks),
        session_used: Some(report.session.session_used),
        session_lookup_ms: Some(report.session.session_lookup_ms),
        kdf_skipped_by_session: Some(report.session.kdf_skipped_by_session),
        solid_groups: Some(report.blocks.solid_groups),
        solid_files: Some(report.blocks.solid_files),
        cache_index_format: Some(report.l2.cache_index_format.clone()),
        cache_index_open_ms: Some(report.l2.cache_index_open_ms),
        cache_index_commit_ms: Some(report.l2.cache_index_commit_ms),
        socket_connect_us: Some(report.timings_us.socket_connect_us),
        socket_pack_roundtrip_us: Some(report.timings_us.socket_pack_roundtrip_us),
        daemon_auth_us: Some(report.timings_us.daemon_auth_us),
        daemon_job_execute_us: Some(report.timings_us.daemon_job_execute_us),
        daemon_response_bytes: Some(report.timings_us.daemon_response_bytes),
        response_serialize_us: Some(report.timings_us.response_serialize_us),
        client_decode_us: Some(report.timings_us.client_decode_us),
        pipeline: Some(report.pipeline.clone()),
        notes: notes.to_string(),
    }
}

fn run_zip_benchmark(
    input_dir: &Path,
    work_dir: &Path,
    input_bytes: u64,
) -> anyhow::Result<BenchmarkRow> {
    if !command_exists("zip") {
        return Ok(skipped_row("zip", input_bytes, "skipped (not installed)"));
    }
    let output = work_dir.join("compare.zip");
    let started = Instant::now();
    let status = ProcessCommand::new("zip")
        .arg("-qr")
        .arg(&output)
        .arg(".")
        .arg("-x")
        .arg(".hig-cache/*")
        .current_dir(input_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("zip benchmark failed");
    }
    Ok(BenchmarkRow {
        tool: "zip".to_string(),
        input_bytes,
        archive_bytes: Some(fs::metadata(output)?.len()),
        duration_ms: Some(started.elapsed().as_millis()),
        cache_hit_rate: None,
        scan_cache_hit_rate: None,
        chunk_metadata_reuses: None,
        trusted_bytes_skipped: None,
        scan_ms: None,
        plan_ms: None,
        kdf_ms: None,
        pack_blocks_ms: None,
        speed: None,
        kdf_profile: None,
        encryption: None,
        worker_count: None,
        kdf_overlapped_ms: None,
        read_ms: None,
        compression_ms: None,
        crypto_ms: None,
        payload_write_ms: None,
        writer_strategy: None,
        archive_preallocated_bytes: None,
        preallocation_enabled: None,
        cached_payload_open_count: None,
        cached_range_open_count: None,
        cached_payload_read_bytes: None,
        prefetched_bytes: None,
        direct_write_count: None,
        buffered_write_count: None,
        peak_pipeline_memory_bytes: None,
        payload_read_ms: None,
        writer_wait_ms: None,
        output_flush_ms: None,
        output_rename_ms: None,
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
        chunk_plan_cache_hits: None,
        chunk_plan_cache_misses: None,
        sealed_block_hits: None,
        sealed_block_misses: None,
        sealed_bytes_reused: None,
        reencrypted_cache_hits: None,
        payload_source_cache_files: None,
        payload_source_memory_bytes: None,
        cache_pack_hits: None,
        cache_pack_misses: None,
        cache_pack_fallbacks: None,
        session_used: None,
        session_lookup_ms: None,
        kdf_skipped_by_session: None,
        solid_groups: None,
        solid_files: None,
        cache_index_format: None,
        cache_index_open_ms: None,
        cache_index_commit_ms: None,
        socket_connect_us: None,
        socket_pack_roundtrip_us: None,
        daemon_auth_us: None,
        daemon_job_execute_us: None,
        daemon_response_bytes: None,
        response_serialize_us: None,
        client_decode_us: None,
        pipeline: None,
        notes: "zip -qr".to_string(),
    })
}

fn run_tar_zstd_benchmark(
    input_dir: &Path,
    work_dir: &Path,
    input_bytes: u64,
) -> anyhow::Result<BenchmarkRow> {
    if !command_exists("tar") || !command_exists("zstd") {
        return Ok(skipped_row(
            "tar.zst",
            input_bytes,
            "skipped (tar or zstd not installed)",
        ));
    }
    let tar_output = work_dir.join("compare.tar");
    let zstd_output = work_dir.join("compare.tar.zst");
    let started = Instant::now();
    let tar_status = ProcessCommand::new("tar")
        .arg("--exclude=.hig-cache")
        .arg("-cf")
        .arg(&tar_output)
        .arg("-C")
        .arg(input_dir)
        .arg(".")
        .status()?;
    if !tar_status.success() {
        anyhow::bail!("tar benchmark failed");
    }
    let zstd_status = ProcessCommand::new("zstd")
        .arg("-q")
        .arg("-1")
        .arg("-f")
        .arg(&tar_output)
        .arg("-o")
        .arg(&zstd_output)
        .status()?;
    if !zstd_status.success() {
        anyhow::bail!("zstd benchmark failed");
    }
    let _ = fs::remove_file(tar_output);
    Ok(BenchmarkRow {
        tool: "tar.zst".to_string(),
        input_bytes,
        archive_bytes: Some(fs::metadata(zstd_output)?.len()),
        duration_ms: Some(started.elapsed().as_millis()),
        cache_hit_rate: None,
        scan_cache_hit_rate: None,
        chunk_metadata_reuses: None,
        trusted_bytes_skipped: None,
        scan_ms: None,
        plan_ms: None,
        kdf_ms: None,
        pack_blocks_ms: None,
        speed: None,
        kdf_profile: None,
        encryption: None,
        worker_count: None,
        kdf_overlapped_ms: None,
        read_ms: None,
        compression_ms: None,
        crypto_ms: None,
        payload_write_ms: None,
        writer_strategy: None,
        archive_preallocated_bytes: None,
        preallocation_enabled: None,
        cached_payload_open_count: None,
        cached_range_open_count: None,
        cached_payload_read_bytes: None,
        prefetched_bytes: None,
        direct_write_count: None,
        buffered_write_count: None,
        peak_pipeline_memory_bytes: None,
        payload_read_ms: None,
        writer_wait_ms: None,
        output_flush_ms: None,
        output_rename_ms: None,
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
        chunk_plan_cache_hits: None,
        chunk_plan_cache_misses: None,
        sealed_block_hits: None,
        sealed_block_misses: None,
        sealed_bytes_reused: None,
        reencrypted_cache_hits: None,
        payload_source_cache_files: None,
        payload_source_memory_bytes: None,
        cache_pack_hits: None,
        cache_pack_misses: None,
        cache_pack_fallbacks: None,
        session_used: None,
        session_lookup_ms: None,
        kdf_skipped_by_session: None,
        solid_groups: None,
        solid_files: None,
        cache_index_format: None,
        cache_index_open_ms: None,
        cache_index_commit_ms: None,
        socket_connect_us: None,
        socket_pack_roundtrip_us: None,
        daemon_auth_us: None,
        daemon_job_execute_us: None,
        daemon_response_bytes: None,
        response_serialize_us: None,
        client_decode_us: None,
        pipeline: None,
        notes: "tar -cf + zstd -1".to_string(),
    })
}

fn run_tar_gzip_benchmark(
    input_dir: &Path,
    work_dir: &Path,
    input_bytes: u64,
) -> anyhow::Result<BenchmarkRow> {
    if !command_exists("tar") || !command_exists("gzip") {
        return Ok(skipped_row(
            "tar.gz",
            input_bytes,
            "skipped (tar or gzip not installed)",
        ));
    }
    let tar_output = work_dir.join("compare-gzip.tar");
    let gzip_output = work_dir.join("compare.tar.gz");
    let started = Instant::now();
    let tar_status = ProcessCommand::new("tar")
        .arg("--exclude=.hig-cache")
        .arg("-cf")
        .arg(&tar_output)
        .arg("-C")
        .arg(input_dir)
        .arg(".")
        .status()?;
    if !tar_status.success() {
        anyhow::bail!("tar benchmark failed");
    }
    let gzip_status = ProcessCommand::new("gzip")
        .arg("-6")
        .arg("-c")
        .arg(&tar_output)
        .stdout(fs::File::create(&gzip_output)?)
        .status()?;
    if !gzip_status.success() {
        anyhow::bail!("gzip benchmark failed");
    }
    let _ = fs::remove_file(tar_output);
    Ok(BenchmarkRow {
        tool: "tar.gz".to_string(),
        input_bytes,
        archive_bytes: Some(fs::metadata(gzip_output)?.len()),
        duration_ms: Some(started.elapsed().as_millis()),
        cache_hit_rate: None,
        scan_cache_hit_rate: None,
        chunk_metadata_reuses: None,
        trusted_bytes_skipped: None,
        scan_ms: None,
        plan_ms: None,
        kdf_ms: None,
        pack_blocks_ms: None,
        speed: None,
        kdf_profile: None,
        encryption: None,
        worker_count: None,
        kdf_overlapped_ms: None,
        read_ms: None,
        compression_ms: None,
        crypto_ms: None,
        payload_write_ms: None,
        writer_strategy: None,
        archive_preallocated_bytes: None,
        preallocation_enabled: None,
        cached_payload_open_count: None,
        cached_range_open_count: None,
        cached_payload_read_bytes: None,
        prefetched_bytes: None,
        direct_write_count: None,
        buffered_write_count: None,
        peak_pipeline_memory_bytes: None,
        payload_read_ms: None,
        writer_wait_ms: None,
        output_flush_ms: None,
        output_rename_ms: None,
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
        chunk_plan_cache_hits: None,
        chunk_plan_cache_misses: None,
        sealed_block_hits: None,
        sealed_block_misses: None,
        sealed_bytes_reused: None,
        reencrypted_cache_hits: None,
        payload_source_cache_files: None,
        payload_source_memory_bytes: None,
        cache_pack_hits: None,
        cache_pack_misses: None,
        cache_pack_fallbacks: None,
        session_used: None,
        session_lookup_ms: None,
        kdf_skipped_by_session: None,
        solid_groups: None,
        solid_files: None,
        cache_index_format: None,
        cache_index_open_ms: None,
        cache_index_commit_ms: None,
        socket_connect_us: None,
        socket_pack_roundtrip_us: None,
        daemon_auth_us: None,
        daemon_job_execute_us: None,
        daemon_response_bytes: None,
        response_serialize_us: None,
        client_decode_us: None,
        pipeline: None,
        notes: "tar -cf + gzip -6".to_string(),
    })
}

fn run_7z_benchmark(_input_dir: &Path, input_bytes: u64) -> BenchmarkRow {
    if !command_exists("7z") {
        return skipped_row("7z", input_bytes, "skipped (not installed)");
    }
    skipped_row(
        "7z",
        input_bytes,
        "skipped (7z runner not implemented in v1.0.1)",
    )
}

fn run_lobehub_watch_compare(
    options: &CompareOptions,
    source: &Path,
    probe: &CopyProbe,
    volume_probes: &[CopyProbe],
    work_dir: &Path,
) -> anyhow::Result<()> {
    let password = options
        .password
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("lobehub-watch requires --password"))?;
    let target = recreate_dataset_dir(work_dir, "lobehub-watch-data")?;
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| work_dir.join("hig-watch-cache"));
    fs::create_dir_all(&cache_dir)?;
    let config = init_project(&target, Some(cache_dir.clone()), Vec::new())?;
    ensure_daemon(&cache_dir, default_session_ttl(None))?;
    let registered = match request_daemon(
        &cache_dir,
        DaemonRequest::ProjectRegister(ProjectRegistration {
            root: target.clone(),
            config: config.clone(),
        }),
    )? {
        Some(DaemonResponse::ProjectRegistered(status)) => status,
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not register lobehub-watch project"),
    };
    unlock_session_for_cache(&cache_dir, password, KdfProfile::Secure, 1_800)?;

    let trace = lobehub_watch_trace(source)?;
    let write_started = Instant::now();
    for path in &trace {
        let relative = path.strip_prefix(source)?;
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        write_file_as_event(path, &destination)?;
    }
    let corpus_write_ms = write_started.elapsed().as_secs_f64() * 1000.0;
    let bootstrap_started = Instant::now();
    std::thread::sleep(Duration::from_millis(250));
    let bootstrap_status = wait_for_project_quiescent(
        &cache_dir,
        config.project_id,
        registered.generation.saturating_add(1),
        Duration::from_millis(750),
        Duration::from_secs(120),
    )?;
    let watcher_bootstrap_ms = bootstrap_started.elapsed().as_secs_f64() * 1000.0;

    let bootstrap_pack = project_benchmark_pack(
        options,
        &target,
        &cache_dir,
        work_dir.join("hig-out/project-bootstrap.hig"),
    )?;

    let single_before = bootstrap_status.generation;
    let single_path = deterministic_project_files(&target)?
        .into_iter()
        .find(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("rs" | "ts" | "tsx" | "js" | "jsx")
            )
        })
        .ok_or_else(|| anyhow::anyhow!("lobehub-watch has no source file to edit"))?;
    append_text(&single_path, "\n// hig watch single edit\n")?;
    let single_ready_started = Instant::now();
    let mut single_status = wait_for_project_ready(
        &cache_dir,
        config.project_id,
        single_before.saturating_add(1),
        Duration::from_secs(10),
    )?;
    let single_prepare_ms = single_ready_started.elapsed().as_secs_f64() * 1000.0;
    let mut single_prepare_samples = vec![single_prepare_ms];
    let single_pack = project_benchmark_pack(
        options,
        &target,
        &cache_dir,
        work_dir.join("hig-out/project-single-edit.hig"),
    )?;
    for sample in 1..20 {
        let before = single_status.generation;
        append_text(
            &single_path,
            &format!("// hig watch latency sample {sample}\n"),
        )?;
        let started = Instant::now();
        single_status = wait_for_project_ready(
            &cache_dir,
            config.project_id,
            before.saturating_add(1),
            Duration::from_secs(10),
        )?;
        single_prepare_samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let single_prepare_stats = SampleStats::from_values(&single_prepare_samples);

    let five_before = single_status.generation;
    let modified_five = apply_lobehub_incremental_changes(&target)?;
    let five_ready_started = Instant::now();
    let five_status = wait_for_project_ready(
        &cache_dir,
        config.project_id,
        five_before.saturating_add(1),
        Duration::from_secs(10),
    )?;
    let five_prepare_ms = five_ready_started.elapsed().as_secs_f64() * 1000.0;
    let five_pack = project_benchmark_pack(
        options,
        &target,
        &cache_dir,
        work_dir.join("hig-out/project-five-edits.hig"),
    )?;

    let mixed_before = five_status.generation;
    let mixed_operations = apply_watch_mixed_operations(&target)?;
    let mixed_status = wait_for_project_ready(
        &cache_dir,
        config.project_id,
        mixed_before.saturating_add(1),
        Duration::from_secs(20),
    )?;

    let burst_before = mixed_status.generation;
    let burst_started = Instant::now();
    create_watch_burst(&target, 1_000, 8)?;
    let burst_status = wait_for_project_ready(
        &cache_dir,
        config.project_id,
        burst_before.saturating_add(1),
        Duration::from_secs(30),
    )?;
    let burst_catchup_ms = burst_started.elapsed().as_secs_f64() * 1000.0;
    let burst_archive = work_dir.join("hig-out/project-burst.hig");
    let burst_pack = project_benchmark_pack(options, &target, &cache_dir, burst_archive.clone())?;
    let warm_output = work_dir.join("hig-out/project-warm-sample.hig");
    let mut project_warm_samples = Vec::with_capacity(20);
    for run in 0..20 {
        let _ = run;
        let report = project_benchmark_pack(options, &target, &cache_dir, warm_output.clone())?;
        project_warm_samples.push(WarmProjectSample::from_report(&report));
    }
    let project_warm_durations = project_warm_samples
        .iter()
        .map(|sample| sample.duration_us as f64 / 1000.0)
        .collect::<Vec<_>>();
    let project_warm_stats = SampleStats::from_values(&project_warm_durations);
    let project_warm_stage_stats = warm_stage_stats(&project_warm_samples);
    let project_warm_hotspots = top_warm_hotspots(&project_warm_stage_stats, 3);
    let _ = fs::remove_file(&warm_output);

    let cli_output = work_dir.join("hig-out/project-cli-wall.hig");
    let mut cli_wall_samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let cli_started = Instant::now();
        let status = ProcessCommand::new(std::env::current_exe()?)
            .arg("pack")
            .arg(&target)
            .arg("-o")
            .arg(&cli_output)
            .arg("--use-session")
            .arg("--daemon")
            .arg("required")
            .arg("--project")
            .arg("required")
            .arg("--cache-dir")
            .arg(&cache_dir)
            .arg("--quiet")
            .status()?;
        anyhow::ensure!(status.success(), "lobehub-watch CLI-wall pack failed");
        cli_wall_samples.push(cli_started.elapsed().as_secs_f64() * 1000.0);
    }
    let cli_wall_stats = SampleStats::from_values(&cli_wall_samples);
    let cli_wall_ms = cli_wall_stats.median_ms;

    let normal_cache = work_dir.join("normal-cache");
    let normal_first = pack(normal_watch_pack_options(
        options,
        &target,
        &normal_cache,
        work_dir.join("hig-out/normal-first.hig"),
    ))?;
    let normal_warm = pack(normal_watch_pack_options(
        options,
        &target,
        &normal_cache,
        work_dir.join("hig-out/normal-warm.hig"),
    ))?;

    let input_bytes = dir_size(&target)?;
    let zip = run_zip_benchmark(&target, work_dir, input_bytes)?;
    let gzip = run_tar_gzip_benchmark(&target, work_dir, input_bytes)?;
    let zstd = run_tar_zstd_benchmark(&target, work_dir, input_bytes)?;

    let unpacked = recreate_dataset_dir(work_dir, "lobehub-watch-unpacked")?;
    unpack(UnpackOptions {
        archive_file: burst_archive,
        output_dir: unpacked.clone(),
        password: Some(password.to_string()),
        overwrite: false,
    })?;
    anyhow::ensure!(
        directory_digest(&target)? == directory_digest(&unpacked)?,
        "lobehub-watch archive does not match frozen project contents"
    );

    let project_warm_gate = project_warm_stats.median_ms < 150.0;
    let project_cli_gate = cli_wall_stats.median_ms < 250.0;
    let single_prepare_gate = single_prepare_stats.p95_ms < 50.0;
    let five_edit_gate = five_pack.duration.as_secs_f64() * 1000.0 < 150.0;
    let burst_gate = burst_catchup_ms < 2_000.0;
    let quality_gate = burst_pack.archive_bytes <= 56_729_505_u64 * 101 / 100;
    let release_gate_status = if project_warm_gate
        && project_cli_gate
        && single_prepare_gate
        && five_edit_gate
        && burst_gate
        && quality_gate
    {
        "PASS"
    } else if !probe.qualified && quality_gate {
        "NOT_ABSOLUTE_PASS_ENV_UNQUALIFIED"
    } else {
        "FAILED_QUALIFIED_VOLUME"
    };
    let io_hotspot_summary = project_warm_hotspots
        .iter()
        .map(|hotspot| {
            format!(
                "{} median={}us p95={}us",
                hotspot.stage, hotspot.median_us, hotspot.p95_us
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    let summary = serde_json::json!({
        "version": "1.9.7",
        "suite": "lobehub-watch",
        "seed": 0x4849_4757_4154_4348_u64,
        "source": source,
        "target": target,
        "input_files": burst_status.files,
        "input_bytes": input_bytes,
        "environment_status": if probe.qualified { "QUALIFIED" } else { "ENVIRONMENT_NOT_QUALIFIED" },
        "release_gate_status": release_gate_status,
        "fastest_available_volume": volume_probes
            .iter()
            .max_by(|left, right| left.copy_256_mib_median.partial_cmp(&right.copy_256_mib_median).unwrap_or(std::cmp::Ordering::Equal))
            .map(|probe| probe.path.display().to_string())
            .unwrap_or_else(|| probe.path.display().to_string()),
        "selected_volume_path": probe.path.display().to_string(),
        "selected_volume_copy_mib_s": probe.copy_256_mib_median,
        "workspace_volume_copy_mib_s": std::env::current_dir().ok().and_then(|cwd| volume_probes.iter().find(|candidate| paths_on_same_volume(&candidate.path, &cwd)).map(|candidate| candidate.copy_256_mib_median)),
        "volume_probes": volume_probes.iter().map(VolumeProbeSummary::from).collect::<Vec<_>>(),
        "io_hotspot_summary": io_hotspot_summary,
        "copy_256_mib_median_mib_s": probe.copy_256_mib_median,
        "copy_256_mib_p95_mib_s": probe.copy_256_mib_p95,
        "corpus_write_ms": corpus_write_ms,
        "watcher_bootstrap_ms": watcher_bootstrap_ms,
        "single_prepare_ms": single_prepare_ms,
        "single_prepare_median_ms": single_prepare_stats.median_ms,
        "single_prepare_p95_ms": single_prepare_stats.p95_ms,
        "five_prepare_ms": five_prepare_ms,
        "burst_catchup_ms": burst_catchup_ms,
        "mixed_operations": mixed_operations,
        "modified_five": modified_five,
        "project_generation": burst_status.generation,
        "watcher_backend": burst_status.watcher_backend,
        "watcher_overflow_count": burst_status.watcher_overflow_count,
        "normal_first_ms": normal_first.duration.as_secs_f64() * 1000.0,
        "normal_warm_ms": normal_warm.duration.as_secs_f64() * 1000.0,
        "project_bootstrap_pack_ms": bootstrap_pack.duration.as_secs_f64() * 1000.0,
        "project_single_edit_pack_ms": single_pack.duration.as_secs_f64() * 1000.0,
        "project_five_edit_pack_ms": five_pack.duration.as_secs_f64() * 1000.0,
        "project_burst_pack_ms": burst_pack.duration.as_secs_f64() * 1000.0,
        "project_warm_median_ms": project_warm_stats.median_ms,
        "project_warm_p95_ms": project_warm_stats.p95_ms,
        "project_warm_samples": project_warm_samples,
        "project_warm_stage_stats": project_warm_stage_stats,
        "project_warm_hotspots": project_warm_hotspots,
        "project_cli_wall_ms": cli_wall_ms,
        "project_cli_wall_p95_ms": cli_wall_stats.p95_ms,
        "project_cli_wall_samples_ms": cli_wall_samples,
        "project_archive_bytes": burst_pack.archive_bytes,
        "project_hash_reuses": burst_pack.project.project_hash_reuses,
        "project_prepared_object_hits": burst_pack.project.project_prepared_object_hits,
        "project_verify_us": burst_pack.project.project_verify_us,
        "zip_ms": zip.duration_ms,
        "zip_bytes": zip.archive_bytes,
        "tar_gzip_ms": gzip.duration_ms,
        "tar_gzip_bytes": gzip.archive_bytes,
        "tar_zstd_ms": zstd.duration_ms,
        "tar_zstd_bytes": zstd.archive_bytes,
        "correctness_digest_match": true,
        "project_warm_gate": project_warm_gate,
        "project_cli_gate": project_cli_gate,
        "single_prepare_gate": single_prepare_gate,
        "five_edit_gate": five_edit_gate,
        "burst_gate": burst_gate,
        "quality_gate": quality_gate
    });
    fs::create_dir_all("artifacts")?;
    fs::write(
        "artifacts/hig-v1.9.7-lobehub-watch-summary.json",
        serde_json::to_vec_pretty(&summary)?,
    )?;
    let markdown = render_lobehub_watch_markdown(&summary);
    fs::write("artifacts/hig-v1.9.7-lobehub-watch-benchmark.md", &markdown)?;
    fs::write("artifacts/hig-v1.9.7-lobehub-watch-profile.md", markdown)?;
    if options.report_mode == ReportMode::Json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("benchmark: wrote artifacts/hig-v1.9.7-lobehub-watch-benchmark.md");
    }
    Ok(())
}

fn project_benchmark_pack(
    options: &CompareOptions,
    input: &Path,
    cache: &Path,
    output: PathBuf,
) -> anyhow::Result<PackReport> {
    pack_with_daemon_policy(
        PackOptions {
            input_dir: input.to_path_buf(),
            output_file: output,
            password: None,
            encryption: EncryptionMode::Password,
            cache_dir: Some(cache.to_path_buf()),
            threads: options.threads,
            compression: options.compression,
            level: options.level,
            use_cache: true,
            trust_metadata: false,
            format: ArchiveFormat::HigV2,
            batch: options.batch,
            chunk: options.chunk,
            speed: SpeedMode::Balanced,
            kdf_profile: KdfProfile::Secure,
            sealed_cache: false,
            manifest_format: options.manifest_format,
            use_session: true,
            session_required: true,
            session_ttl_secs: None,
            solid: options.solid,
            pipeline: PipelineOptions {
                daemon_mode: DaemonMode::Required,
                project_mode: ProjectMode::Required,
                ..PipelineOptions::default()
            },
        },
        ReportMode::Json,
    )
}

fn normal_watch_pack_options(
    options: &CompareOptions,
    input: &Path,
    cache: &Path,
    output: PathBuf,
) -> PackOptions {
    PackOptions {
        input_dir: input.to_path_buf(),
        output_file: output,
        password: options.password.clone(),
        encryption: EncryptionMode::Password,
        cache_dir: Some(cache.to_path_buf()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: true,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
        speed: SpeedMode::Balanced,
        kdf_profile: KdfProfile::Secure,
        sealed_cache: false,
        manifest_format: options.manifest_format,
        use_session: false,
        session_required: false,
        session_ttl_secs: None,
        solid: options.solid,
        pipeline: PipelineOptions {
            daemon_mode: DaemonMode::Off,
            project_mode: ProjectMode::Off,
            ..PipelineOptions::default()
        },
    }
}

fn lobehub_watch_trace(source: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let excludes = excluded_paths_for_suite(BenchSuite::Lobehub);
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(source)?;
        if relative.components().any(|component| {
            let value = component.as_os_str().to_string_lossy();
            excludes.iter().any(|excluded| excluded == value.as_ref())
        }) {
            continue;
        }
        files.push(entry.into_path());
    }
    files.sort_by_key(|path| {
        let relative = path.strip_prefix(source).unwrap_or(path);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&0x4849_4757_4154_4348_u64.to_le_bytes());
        hasher.update(relative.to_string_lossy().as_bytes());
        *hasher.finalize().as_bytes()
    });
    Ok(files)
}

fn write_file_as_event(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    Ok(())
}

fn wait_for_project_ready(
    cache: &Path,
    project_id: [u8; 16],
    minimum_generation: u64,
    timeout: Duration,
) -> anyhow::Result<hig_core::ProjectStatusReport> {
    let started = Instant::now();
    loop {
        match request_daemon(cache, DaemonRequest::ProjectStatus { project_id })? {
            Some(DaemonResponse::ProjectStatus(status))
                if status.snapshot_validity == hig_core::SnapshotValidity::Ready
                    && status.pending_events == 0
                    && status.last_event_age_ms >= 15
                    && status.generation >= minimum_generation =>
            {
                return Ok(status);
            }
            Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
            _ => {}
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "timed out waiting for project watcher generation {minimum_generation}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_project_quiescent(
    cache: &Path,
    project_id: [u8; 16],
    minimum_generation: u64,
    quiet_for: Duration,
    timeout: Duration,
) -> anyhow::Result<hig_core::ProjectStatusReport> {
    let started = Instant::now();
    let mut stable_since: Option<Instant> = None;
    let mut stable_generation = 0_u64;
    let mut last_status: Option<hig_core::ProjectStatusReport> = None;
    loop {
        match request_daemon(cache, DaemonRequest::ProjectStatus { project_id })? {
            Some(DaemonResponse::ProjectStatus(status))
                if status.snapshot_validity == hig_core::SnapshotValidity::Ready
                    && status.pending_events == 0
                    && status.last_event_age_ms >= 15
                    && status.generation >= minimum_generation =>
            {
                if status.generation != stable_generation {
                    stable_generation = status.generation;
                    stable_since = Some(Instant::now());
                } else if stable_since.is_none() {
                    stable_since = Some(Instant::now());
                }
                if stable_since.is_some_and(|since| since.elapsed() >= quiet_for) {
                    return Ok(status);
                }
                last_status = Some(status);
            }
            Some(DaemonResponse::ProjectStatus(status)) => {
                stable_since = None;
                last_status = Some(status);
            }
            Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
            _ => {}
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "timed out waiting for quiescent project watcher generation {}; last status: {:?}",
            minimum_generation,
            last_status
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn deterministic_project_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            !entry
                .path()
                .components()
                .any(|component| component.as_os_str() == ".hig")
        })
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn apply_watch_mixed_operations(root: &Path) -> anyhow::Result<u64> {
    let files = deterministic_project_files(root)?;
    anyhow::ensure!(files.len() >= 100, "watch corpus needs at least 100 files");
    for path in files.iter().take(40) {
        let mut output = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("open append target {}", path.display()))?;
        output
            .write_all(b"\nHIG_WATCH_APPEND\n")
            .with_context(|| format!("append watch target {}", path.display()))?;
    }
    for (index, path) in files.iter().skip(40).take(20).enumerate() {
        fs::write(path, format!("hig watch overwrite {index}\n"))
            .with_context(|| format!("overwrite watch target {}", path.display()))?;
    }
    for (index, path) in files.iter().skip(60).take(10).enumerate() {
        let temp = path.with_extension(format!("hig-replace-{index}"));
        fs::write(&temp, format!("hig atomic replacement {index}\n"))
            .with_context(|| format!("write atomic replacement {}", temp.display()))?;
        fs::rename(&temp, path).with_context(|| {
            format!(
                "rename atomic replacement {} -> {}",
                temp.display(),
                path.display()
            )
        })?;
    }
    let generated = root.join("watch-generated");
    fs::create_dir_all(&generated)?;
    for index in 0..10 {
        let created = generated.join(format!("created-{index}.txt"));
        fs::write(&created, b"created\n")
            .with_context(|| format!("create watch file {}", created.display()))?;
    }
    for (index, path) in files.iter().skip(70).take(10).enumerate() {
        let renamed = path.with_file_name(format!("hig-renamed-{index}.txt"));
        fs::rename(path, &renamed).with_context(|| {
            format!(
                "rename watch target {} -> {}",
                path.display(),
                renamed.display()
            )
        })?;
    }
    for path in files.iter().skip(80).take(10) {
        fs::remove_file(path).with_context(|| format!("remove watch target {}", path.display()))?;
    }
    Ok(100)
}

fn create_watch_burst(root: &Path, operations: usize, workers: usize) -> anyhow::Result<()> {
    let burst = root.join("watch-burst");
    fs::create_dir_all(&burst)?;
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for worker in 0..workers {
            let burst = burst.clone();
            handles.push(scope.spawn(move || -> anyhow::Result<()> {
                for index in (worker..operations).step_by(workers) {
                    fs::write(
                        burst.join(format!("burst-{index:04}.txt")),
                        format!("burst event {index}\n"),
                    )?;
                }
                Ok(())
            }));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("burst writer panicked"))??;
        }
        Ok::<_, anyhow::Error>(())
    })
}

fn directory_digest(root: &Path) -> anyhow::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    for path in deterministic_project_files(root)? {
        let relative = path.strip_prefix(root)?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(&fs::read(path)?);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn render_lobehub_watch_markdown(summary: &serde_json::Value) -> String {
    let mut output = format!(
        "# Hig v1.9.7 LobeHub Watch Benchmark\n\n\
- Environment: `{}`\n\
- Release gate status: `{}`\n\
- Selected volume: `{}` (`{:.2} MiB/s` median 256MiB copy)\n\
- Fastest available volume: `{}`\n\
- I/O hotspot summary: `{}`\n\
- Input: `{}` files / `{}` bytes\n\
- Corpus write: `{:.3} ms`\n\
- Watcher bootstrap catch-up: `{:.3} ms`\n\
- Single edit prepare: `{:.3} ms`\n\
- Five edit prepare: `{:.3} ms`\n\
- 1000-event burst catch-up: `{:.3} ms`\n\n\
| scenario | duration ms | archive bytes |\n|---|---:|---:|\n\
| normal first | {:.3} | - |\n\
| normal warm | {:.3} | - |\n\
| project bootstrap pack | {:.3} | - |\n\
| project single edit pack | {:.3} | - |\n\
| project five edit pack | {:.3} | - |\n\
| project burst pack | {:.3} | {} |\n\
| project CLI wall | {:.3} | - |\n\
| zip | {} | {} |\n\
| tar.gz | {} | {} |\n\
| tar.zst | {} | {} |\n\n\
- Project hash reuses: `{}`\n\
- Prepared object hits: `{}`\n\
- Project metadata verify: `{}` us\n\
- Watcher overflow count: `{}`\n\
- Correctness digest match: `{}`\n",
        summary["environment_status"].as_str().unwrap_or("unknown"),
        summary["release_gate_status"].as_str().unwrap_or("unknown"),
        summary["selected_volume_path"]
            .as_str()
            .unwrap_or("unknown"),
        summary["selected_volume_copy_mib_s"]
            .as_f64()
            .unwrap_or_default(),
        summary["fastest_available_volume"]
            .as_str()
            .unwrap_or("unknown"),
        summary["io_hotspot_summary"].as_str().unwrap_or("unknown"),
        summary["input_files"].as_u64().unwrap_or_default(),
        summary["input_bytes"].as_u64().unwrap_or_default(),
        summary["corpus_write_ms"].as_f64().unwrap_or_default(),
        summary["watcher_bootstrap_ms"].as_f64().unwrap_or_default(),
        summary["single_prepare_ms"].as_f64().unwrap_or_default(),
        summary["five_prepare_ms"].as_f64().unwrap_or_default(),
        summary["burst_catchup_ms"].as_f64().unwrap_or_default(),
        summary["normal_first_ms"].as_f64().unwrap_or_default(),
        summary["normal_warm_ms"].as_f64().unwrap_or_default(),
        summary["project_bootstrap_pack_ms"]
            .as_f64()
            .unwrap_or_default(),
        summary["project_single_edit_pack_ms"]
            .as_f64()
            .unwrap_or_default(),
        summary["project_five_edit_pack_ms"]
            .as_f64()
            .unwrap_or_default(),
        summary["project_burst_pack_ms"]
            .as_f64()
            .unwrap_or_default(),
        summary["project_archive_bytes"]
            .as_u64()
            .unwrap_or_default(),
        summary["project_cli_wall_ms"].as_f64().unwrap_or_default(),
        summary["zip_ms"].as_u64().unwrap_or_default(),
        summary["zip_bytes"].as_u64().unwrap_or_default(),
        summary["tar_gzip_ms"].as_u64().unwrap_or_default(),
        summary["tar_gzip_bytes"].as_u64().unwrap_or_default(),
        summary["tar_zstd_ms"].as_u64().unwrap_or_default(),
        summary["tar_zstd_bytes"].as_u64().unwrap_or_default(),
        summary["project_hash_reuses"].as_u64().unwrap_or_default(),
        summary["project_prepared_object_hits"]
            .as_u64()
            .unwrap_or_default(),
        summary["project_verify_us"].as_u64().unwrap_or_default(),
        summary["watcher_overflow_count"]
            .as_u64()
            .unwrap_or_default(),
        summary["correctness_digest_match"]
            .as_bool()
            .unwrap_or(false),
    );
    output.push_str("\n## Release Gates\n\n");
    for (label, key) in [
        ("project warm <150ms", "project_warm_gate"),
        ("project CLI <250ms", "project_cli_gate"),
        ("single prepare <50ms", "single_prepare_gate"),
        ("five edit pack <150ms", "five_edit_gate"),
        ("1000-event burst <2s", "burst_gate"),
        ("archive quality <= v1.8.5 +1%", "quality_gate"),
    ] {
        output.push_str(&format!(
            "- {label}: `{}`\n",
            summary[key].as_bool().unwrap_or(false)
        ));
    }
    if let Some(stats) = summary["project_warm_stage_stats"].as_array() {
        output.push_str("\n## Project Warm Stage Breakdown\n\n");
        output.push_str("| stage | median us | p95 us |\n|---|---:|---:|\n");
        for stat in stats {
            output.push_str(&format!(
                "| {} | {} | {} |\n",
                stat["stage"].as_str().unwrap_or("unknown"),
                stat["median_us"].as_u64().unwrap_or_default(),
                stat["p95_us"].as_u64().unwrap_or_default()
            ));
        }
    }
    if let Some(hotspots) = summary["project_warm_hotspots"].as_array() {
        output.push_str("\n## Project Warm Top Hotspots\n\n");
        for hotspot in hotspots {
            output.push_str(&format!(
                "- `{}`: median `{}us`, p95 `{}us`\n",
                hotspot["stage"].as_str().unwrap_or("unknown"),
                hotspot["median_us"].as_u64().unwrap_or_default(),
                hotspot["p95_us"].as_u64().unwrap_or_default()
            ));
        }
    }
    output
}

fn materialize_bench_suite_input(
    suite: BenchSuite,
    source_input_dir: &Path,
    work_dir: &Path,
) -> anyhow::Result<PathBuf> {
    match suite {
        BenchSuite::Source | BenchSuite::All => Ok(source_input_dir.to_path_buf()),
        BenchSuite::Lobehub | BenchSuite::LobehubWatch => {
            anyhow::ensure!(
                source_input_dir.exists(),
                "lobehub source directory does not exist: {}",
                source_input_dir.display()
            );
            let dir = PathBuf::from("/private/tmp/hig-lobehub-source-data");
            if dir.exists() {
                fs::remove_dir_all(&dir)?;
            }
            fs::create_dir_all(&dir)?;
            copy_dir_filtered(
                source_input_dir,
                &dir,
                &excluded_paths_for_suite(BenchSuite::Lobehub),
            )?;
            Ok(dir)
        }
        BenchSuite::Small500 => {
            let dir = recreate_dataset_dir(work_dir, "small500")?;
            for index in 0..500 {
                fs::write(
                    dir.join(format!("file-{index:04}.txt")),
                    format!("record {index:04} common common common payload payload payload\n"),
                )?;
            }
            Ok(dir)
        }
        BenchSuite::Textmix => {
            let dir = recreate_dataset_dir(work_dir, "textmix")?;
            fs::create_dir_all(dir.join("src"))?;
            for index in 0..64 {
                fs::write(
                    dir.join("src").join(format!("module_{index:02}.rs")),
                    format!(
                        "pub fn value_{index}() -> usize {{ {index} }}\n#[test]\nfn test_{index}() {{ assert_eq!(value_{index}(), {index}); }}\n"
                    ),
                )?;
                fs::write(
                    dir.join(format!("config-{index:02}.toml")),
                    format!("name = \"module-{index}\"\nenabled = true\nlevel = {index}\n"),
                )?;
                fs::write(
                    dir.join(format!("doc-{index:02}.md")),
                    format!(
                        "# Module {index}\n\nRepeated markdown content for compression quality.\n"
                    ),
                )?;
            }
            Ok(dir)
        }
        BenchSuite::Repeat4m => {
            let dir = recreate_dataset_dir(work_dir, "repeat4m")?;
            let line = b"highly repeatable text payload for gzip and hig comparison\n";
            let mut data = Vec::with_capacity(4 * 1024 * 1024);
            while data.len() < 4 * 1024 * 1024 {
                data.extend_from_slice(line);
            }
            data.truncate(4 * 1024 * 1024);
            fs::write(dir.join("repeat.txt"), data)?;
            Ok(dir)
        }
        BenchSuite::Random8m => {
            let dir = recreate_dataset_dir(work_dir, "random8m")?;
            let mut data = vec![0_u8; 8 * 1024 * 1024];
            let mut state = 0x1234_5678_9abc_def0_u64;
            for byte in &mut data {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            fs::write(dir.join("random.bin"), data)?;
            Ok(dir)
        }
        BenchSuite::Binarymix => {
            let dir = recreate_dataset_dir(work_dir, "binarymix")?;
            fs::create_dir_all(dir.join("assets"))?;
            for index in 0..16 {
                let mut data = vec![0_u8; 64 * 1024];
                for (offset, byte) in data.iter_mut().enumerate() {
                    *byte = ((offset * 31 + index * 17) & 0xff) as u8;
                }
                fs::write(
                    dir.join("assets").join(format!("asset-{index:02}.bin")),
                    data,
                )?;
                fs::write(
                    dir.join(format!("manifest-{index:02}.json")),
                    format!(
                        "{{\"asset\":{index},\"kind\":\"binarymix\",\"repeat\":\"metadata\"}}\n"
                    ),
                )?;
            }
            Ok(dir)
        }
    }
}

fn copy_dir_filtered(source: &Path, target: &Path, excluded: &[String]) -> anyhow::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();
        if excluded
            .iter()
            .any(|exclude| exclude == file_name_str.as_ref())
        {
            continue;
        }
        let source_path = entry.path();
        let target_path = target.join(file_name);
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_dir_filtered(&source_path, &target_path, excluded)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn recreate_dataset_dir(work_dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let dir = work_dir.join("datasets").join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn skipped_row(tool: &str, input_bytes: u64, notes: &str) -> BenchmarkRow {
    BenchmarkRow {
        tool: tool.to_string(),
        input_bytes,
        archive_bytes: None,
        duration_ms: None,
        cache_hit_rate: None,
        scan_cache_hit_rate: None,
        chunk_metadata_reuses: None,
        trusted_bytes_skipped: None,
        scan_ms: None,
        plan_ms: None,
        kdf_ms: None,
        pack_blocks_ms: None,
        speed: None,
        kdf_profile: None,
        encryption: None,
        worker_count: None,
        kdf_overlapped_ms: None,
        read_ms: None,
        compression_ms: None,
        crypto_ms: None,
        payload_write_ms: None,
        writer_strategy: None,
        archive_preallocated_bytes: None,
        preallocation_enabled: None,
        cached_payload_open_count: None,
        cached_range_open_count: None,
        cached_payload_read_bytes: None,
        prefetched_bytes: None,
        direct_write_count: None,
        buffered_write_count: None,
        peak_pipeline_memory_bytes: None,
        payload_read_ms: None,
        writer_wait_ms: None,
        output_flush_ms: None,
        output_rename_ms: None,
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
        chunk_plan_cache_hits: None,
        chunk_plan_cache_misses: None,
        sealed_block_hits: None,
        sealed_block_misses: None,
        sealed_bytes_reused: None,
        reencrypted_cache_hits: None,
        payload_source_cache_files: None,
        payload_source_memory_bytes: None,
        cache_pack_hits: None,
        cache_pack_misses: None,
        cache_pack_fallbacks: None,
        session_used: None,
        session_lookup_ms: None,
        kdf_skipped_by_session: None,
        solid_groups: None,
        solid_files: None,
        cache_index_format: None,
        cache_index_open_ms: None,
        cache_index_commit_ms: None,
        socket_connect_us: None,
        socket_pack_roundtrip_us: None,
        daemon_auth_us: None,
        daemon_job_execute_us: None,
        daemon_response_bytes: None,
        response_serialize_us: None,
        client_decode_us: None,
        pipeline: None,
        notes: notes.to_string(),
    }
}

fn render_markdown(
    input_dir: &Path,
    rows: &[BenchmarkRow],
    probe: &CopyProbe,
    acceptance: &AcceptanceSamples,
    summary: &BenchmarkSummary,
) -> String {
    let mut output = String::new();
    output.push_str("# Hig v1.9.7 Benchmark\n\n");
    output.push_str(&format!("Input: `{}`\n\n", input_dir.display()));
    output.push_str("## Gate Summary\n\n");
    output.push_str("| gate | status |\n|---|---|\n");
    output.push_str(&format!(
        "| environment_status | `{}` |\n",
        summary.environment_status
    ));
    output.push_str(&format!(
        "| release_gate_status | `{}` |\n",
        summary.release_gate_status
    ));
    output.push_str(&format!(
        "| pack_core_gate | {} |\n",
        summary.pack_core_gate
    ));
    output.push_str(&format!("| cli_wall_gate | {} |\n", summary.cli_wall_gate));
    output.push_str(&format!(
        "| size_quality_gate | {} |\n\n",
        summary.size_quality_gate
    ));
    output.push_str("## Environment Qualification\n\n");
    output.push_str(&format!(
        "Status: `{}`\n\nSelected volume: `{}` (`{:.2} MiB/s` median 256MiB copy)\n\nFastest available volume: `{}`\n\nI/O hotspot summary: {}\n\n",
        if probe.qualified {
            "QUALIFIED"
        } else {
            "ENVIRONMENT_NOT_QUALIFIED"
        },
        summary.selected_volume_path,
        summary.selected_volume_copy_mib_s,
        summary.fastest_available_volume,
        summary.io_hotspot_summary
    ));
    output.push_str("## Release Gate Samples\n\n");
    output.push_str(&format!(
        "Standalone second, warm daemon pack-core, Summary/quiet CLI-wall, Full/JSON CLI-wall, and zip were each sampled {} times.\n\n",
        acceptance.runs
    ));
    output.push_str("| tool | median ms | p95 ms | p99 ms | min ms | max ms |\n|---|---:|---:|---:|---:|---:|\n");
    output.push_str(&format!(
        "| Hig Balanced secure standalone second | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |\n",
        acceptance.standalone_second.median_ms,
        acceptance.standalone_second.p95_ms,
        acceptance.standalone_second.p99_ms,
        acceptance.standalone_second.min_ms,
        acceptance.standalone_second.max_ms
    ));
    output.push_str(&format!(
        "| Hig Balanced secure daemon pack-core | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |\n",
        acceptance.pack_core.median_ms,
        acceptance.pack_core.p95_ms,
        acceptance.pack_core.p99_ms,
        acceptance.pack_core.min_ms,
        acceptance.pack_core.max_ms
    ));
    output.push_str(&format!(
        "| Hig Balanced secure daemon Summary + quiet cli-wall | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |\n",
        acceptance.cli_wall.median_ms,
        acceptance.cli_wall.p95_ms,
        acceptance.cli_wall.p99_ms,
        acceptance.cli_wall.min_ms,
        acceptance.cli_wall.max_ms
    ));
    output.push_str(&format!(
        "| Hig Balanced secure daemon Full + JSON cli-wall | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |\n",
        acceptance.cli_wall_full.median_ms,
        acceptance.cli_wall_full.p95_ms,
        acceptance.cli_wall_full.p99_ms,
        acceptance.cli_wall_full.min_ms,
        acceptance.cli_wall_full.max_ms
    ));
    output.push_str(&format!(
        "| zip -qr | {} | {} | {} | {} | {} |\n\n",
        acceptance
            .zip
            .as_ref()
            .map(|value| format!("{:.3}", value.median_ms))
            .unwrap_or_else(|| "-".to_string()),
        acceptance
            .zip
            .as_ref()
            .map(|value| format!("{:.3}", value.p95_ms))
            .unwrap_or_else(|| "-".to_string()),
        acceptance
            .zip
            .as_ref()
            .map(|value| format!("{:.3}", value.p99_ms))
            .unwrap_or_else(|| "-".to_string()),
        acceptance
            .zip
            .as_ref()
            .map(|value| format!("{:.3}", value.min_ms))
            .unwrap_or_else(|| "-".to_string()),
        acceptance
            .zip
            .as_ref()
            .map(|value| format!("{:.3}", value.max_ms))
            .unwrap_or_else(|| "-".to_string())
    ));
    output.push_str("| path | free bytes | used % | 32MiB cp median MiB/s | 32MiB cp p95 MiB/s | 256MiB cp median MiB/s | 256MiB cp p95 MiB/s |\n");
    output.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    output.push_str(&format!(
        "| `{}` | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |\n\n",
        probe.path.display(),
        probe.free_bytes,
        probe.used_percent,
        probe.copy_32_mib_median,
        probe.copy_32_mib_p95,
        probe.copy_256_mib_median,
        probe.copy_256_mib_p95
    ));
    let headers = [
        "tool",
        "encryption",
        "speed",
        "kdf profile",
        "session used",
        "session lookup ms",
        "kdf skipped by session",
        "workers",
        "cache index",
        "cache index open ms",
        "cache index commit ms",
        "socket connect us",
        "socket pack roundtrip us",
        "daemon auth us",
        "daemon job execute us",
        "daemon response bytes",
        "response serialize us",
        "client decode us",
        "daemon used",
        "daemon lookup ms",
        "scheduler queue ms",
        "worker wait ms",
        "buffer pool hits",
        "buffer pool misses",
        "cache pack range hits",
        "cache pack opens",
        "hot index reuses",
        "hot metadata reuses",
        "pipeline peak memory",
        "writer",
        "preallocated bytes",
        "preallocation",
        "cached opens",
        "cached range opens",
        "cached read bytes",
        "prefetched bytes",
        "direct writes",
        "buffered writes",
        "peak pipeline bytes",
        "input bytes",
        "archive bytes",
        "reduction",
        "duration ms",
        "throughput MiB/s",
        "scan ms",
        "plan ms",
        "kdf ms",
        "kdf overlap ms",
        "read ms",
        "compression ms",
        "crypto ms",
        "pack blocks ms",
        "payload read ms",
        "payload write ms",
        "writer wait ms",
        "flush ms",
        "rename ms",
        "sealed hits",
        "sealed misses",
        "sealed bytes reused",
        "reencrypted cache hits",
        "payload cache files",
        "payload memory bytes",
        "cache pack hits",
        "cache pack misses",
        "cache pack fallbacks",
        "cache hit rate",
        "scan cache hit rate",
        "chunk metadata reuses",
        "trusted bytes skipped",
        "batch blocks",
        "solid groups",
        "solid files",
        "single blocks",
        "batched files",
        "chunked files",
        "chunk blocks",
        "chunk hits",
        "chunk misses",
        "chunk plan hits",
        "chunk plan misses",
        "notes",
    ];
    output.push_str("| ");
    output.push_str(&headers.join(" | "));
    output.push_str(" |\n|");
    output.push_str(&vec!["---"; headers.len()].join("|"));
    output.push_str("|\n");
    for row in rows {
        let archive_bytes = row
            .archive_bytes
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let reduction = row
            .archive_bytes
            .map(|archive| reduction_pct(row.input_bytes, archive))
            .map(|value| format!("{value:.2}%"))
            .unwrap_or_else(|| "-".to_string());
        let duration = row
            .duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let throughput = row
            .duration_ms
            .filter(|duration| *duration > 0)
            .map(|duration| {
                let seconds = duration as f64 / 1000.0;
                let mib = row.input_bytes as f64 / 1024.0 / 1024.0;
                format!("{:.2}", mib / seconds)
            })
            .unwrap_or_else(|| "-".to_string());
        let cache_hit_rate = row
            .cache_hit_rate
            .map(|value| format!("{value:.2}%"))
            .unwrap_or_else(|| "-".to_string());
        let scan_cache_hit_rate = row
            .scan_cache_hit_rate
            .map(|value| format!("{value:.2}%"))
            .unwrap_or_else(|| "-".to_string());
        let scan_ms = optional_to_string(row.scan_ms);
        let plan_ms = optional_to_string(row.plan_ms);
        let kdf_ms = optional_to_string(row.kdf_ms);
        let pack_blocks_ms = optional_to_string(row.pack_blocks_ms);
        let speed = optional_to_string(row.speed.as_ref());
        let kdf_profile = optional_to_string(row.kdf_profile.as_ref());
        let encryption = optional_to_string(row.encryption.as_ref());
        let session_used = optional_to_string(row.session_used);
        let session_lookup_ms = optional_to_string(row.session_lookup_ms);
        let kdf_skipped_by_session = optional_to_string(row.kdf_skipped_by_session);
        let workers = optional_to_string(row.worker_count);
        let cache_index = optional_to_string(row.cache_index_format.as_ref());
        let cache_index_open_ms = optional_to_string(row.cache_index_open_ms);
        let cache_index_commit_ms = optional_to_string(row.cache_index_commit_ms);
        let socket_connect_us = optional_to_string(row.socket_connect_us);
        let socket_pack_roundtrip_us = optional_to_string(row.socket_pack_roundtrip_us);
        let daemon_auth_us = optional_to_string(row.daemon_auth_us);
        let daemon_job_execute_us = optional_to_string(row.daemon_job_execute_us);
        let daemon_response_bytes = optional_to_string(row.daemon_response_bytes);
        let response_serialize_us = optional_to_string(row.response_serialize_us);
        let client_decode_us = optional_to_string(row.client_decode_us);
        let daemon_used = optional_to_string(row.pipeline.as_ref().map(|value| value.daemon_used));
        let daemon_lookup_ms =
            optional_to_string(row.pipeline.as_ref().map(|value| value.daemon_lookup_ms));
        let scheduler_queue_ms =
            optional_to_string(row.pipeline.as_ref().map(|value| value.scheduler_queue_ms));
        let cpu_worker_wait_ms =
            optional_to_string(row.pipeline.as_ref().map(|value| value.cpu_worker_wait_ms));
        let buffer_pool_hits =
            optional_to_string(row.pipeline.as_ref().map(|value| value.buffer_pool_hits));
        let buffer_pool_misses =
            optional_to_string(row.pipeline.as_ref().map(|value| value.buffer_pool_misses));
        let cache_pack_range_hits = optional_to_string(
            row.pipeline
                .as_ref()
                .map(|value| value.cache_pack_range_hits),
        );
        let cache_pack_open_count = optional_to_string(
            row.pipeline
                .as_ref()
                .map(|value| value.cache_pack_open_count),
        );
        let hot_index_reuses =
            optional_to_string(row.pipeline.as_ref().map(|value| value.hot_index_reuses));
        let hot_metadata_reuses =
            optional_to_string(row.pipeline.as_ref().map(|value| value.hot_metadata_reuses));
        let pipeline_peak_memory = optional_to_string(
            row.pipeline
                .as_ref()
                .map(|value| value.pipeline_peak_memory_bytes),
        );
        let kdf_overlap = optional_to_string(row.kdf_overlapped_ms);
        let read_ms = optional_to_string(row.read_ms);
        let compression_ms = optional_to_string(row.compression_ms);
        let crypto_ms = optional_to_string(row.crypto_ms);
        let payload_write_ms = optional_to_string(row.payload_write_ms);
        let writer_strategy = optional_to_string(row.writer_strategy.as_ref());
        let preallocated = optional_to_string(row.archive_preallocated_bytes);
        let preallocation = optional_to_string(row.preallocation_enabled);
        let cached_opens = optional_to_string(row.cached_payload_open_count);
        let cached_range_opens = optional_to_string(row.cached_range_open_count);
        let cached_read_bytes = optional_to_string(row.cached_payload_read_bytes);
        let prefetched_bytes = optional_to_string(row.prefetched_bytes);
        let direct_writes = optional_to_string(row.direct_write_count);
        let buffered_writes = optional_to_string(row.buffered_write_count);
        let peak_pipeline_bytes = optional_to_string(row.peak_pipeline_memory_bytes);
        let payload_read_ms = optional_to_string(row.payload_read_ms);
        let writer_wait_ms = optional_to_string(row.writer_wait_ms);
        let flush_ms = optional_to_string(row.output_flush_ms);
        let rename_ms = optional_to_string(row.output_rename_ms);
        let sealed_hits = optional_to_string(row.sealed_block_hits);
        let sealed_misses = optional_to_string(row.sealed_block_misses);
        let sealed_bytes = optional_to_string(row.sealed_bytes_reused);
        let reencrypted = optional_to_string(row.reencrypted_cache_hits);
        let payload_files = optional_to_string(row.payload_source_cache_files);
        let payload_memory = optional_to_string(row.payload_source_memory_bytes);
        let cache_pack_hits = optional_to_string(row.cache_pack_hits);
        let cache_pack_misses = optional_to_string(row.cache_pack_misses);
        let cache_pack_fallbacks = optional_to_string(row.cache_pack_fallbacks);
        let chunk_metadata_reuses = optional_to_string(row.chunk_metadata_reuses);
        let trusted_bytes_skipped = optional_to_string(row.trusted_bytes_skipped);
        let batch_blocks = row
            .batch_blocks
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let solid_groups = optional_to_string(row.solid_groups);
        let solid_files = optional_to_string(row.solid_files);
        let single_blocks = row
            .single_blocks
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let batched_files = row
            .batched_files
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let chunked_files = row
            .chunked_files
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let chunk_blocks = row
            .chunk_blocks
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let chunk_hits = row
            .chunk_cache_hits
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let chunk_misses = row
            .chunk_cache_misses
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let chunk_plan_hits = optional_to_string(row.chunk_plan_cache_hits);
        let chunk_plan_misses = optional_to_string(row.chunk_plan_cache_misses);
        let columns = vec![
            row.tool.clone(),
            encryption,
            speed,
            kdf_profile,
            session_used,
            session_lookup_ms,
            kdf_skipped_by_session,
            workers,
            cache_index,
            cache_index_open_ms,
            cache_index_commit_ms,
            socket_connect_us,
            socket_pack_roundtrip_us,
            daemon_auth_us,
            daemon_job_execute_us,
            daemon_response_bytes,
            response_serialize_us,
            client_decode_us,
            daemon_used,
            daemon_lookup_ms,
            scheduler_queue_ms,
            cpu_worker_wait_ms,
            buffer_pool_hits,
            buffer_pool_misses,
            cache_pack_range_hits,
            cache_pack_open_count,
            hot_index_reuses,
            hot_metadata_reuses,
            pipeline_peak_memory,
            writer_strategy,
            preallocated,
            preallocation,
            cached_opens,
            cached_range_opens,
            cached_read_bytes,
            prefetched_bytes,
            direct_writes,
            buffered_writes,
            peak_pipeline_bytes,
            row.input_bytes.to_string(),
            archive_bytes,
            reduction,
            duration,
            throughput,
            scan_ms,
            plan_ms,
            kdf_ms,
            kdf_overlap,
            read_ms,
            compression_ms,
            crypto_ms,
            pack_blocks_ms,
            payload_read_ms,
            payload_write_ms,
            writer_wait_ms,
            flush_ms,
            rename_ms,
            sealed_hits,
            sealed_misses,
            sealed_bytes,
            reencrypted,
            payload_files,
            payload_memory,
            cache_pack_hits,
            cache_pack_misses,
            cache_pack_fallbacks,
            cache_hit_rate,
            scan_cache_hit_rate,
            chunk_metadata_reuses,
            trusted_bytes_skipped,
            batch_blocks,
            solid_groups,
            solid_files,
            single_blocks,
            batched_files,
            chunked_files,
            chunk_blocks,
            chunk_hits,
            chunk_misses,
            chunk_plan_hits,
            chunk_plan_misses,
            row.notes.clone(),
        ];
        output.push_str("| ");
        output.push_str(&columns.join(" | "));
        output.push_str(" |\n");
    }
    output
}

fn render_lobehub_profile(
    input_dir: &Path,
    rows: &[BenchmarkRow],
    probe: &CopyProbe,
    summary: &BenchmarkSummary,
) -> String {
    let mut output = String::new();
    output.push_str("# Hig v1.9.7 Lobehub Profile\n\n");
    output.push_str("## Dataset\n\n");
    output.push_str(&format!("- Input: `{}`\n", input_dir.display()));
    output.push_str(&format!("- Files: `{}`\n", summary.file_count));
    output.push_str(&format!("- Bytes: `{}`\n", summary.input_bytes));
    output.push_str(&format!(
        "- Excluded: `{}`\n\n",
        summary.excluded_paths.join(", ")
    ));
    output.push_str("## Top-Level Sizes\n\n");
    output.push_str("| path | bytes |\n|---|---:|\n");
    for (name, bytes) in top_level_sizes(input_dir).unwrap_or_default() {
        output.push_str(&format!("| `{name}` | {bytes} |\n"));
    }
    output.push_str("\n## Comparison\n\n");
    output.push_str("| tool | duration ms | archive bytes | notes |\n|---|---:|---:|---|\n");
    for tool in [
        "higv2 balanced first",
        "higv2 balanced secure daemon",
        "higv2 fastest secure",
        "higv2 no-encryption",
        "zip",
        "tar.gz",
        "tar.zst",
    ] {
        if let Some(row) = rows.iter().find(|row| row.tool == tool) {
            output.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                row.tool,
                row.duration_ms
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                row.archive_bytes
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                row.notes
            ));
        }
    }
    output.push_str("\n## Release Gates\n\n");
    output.push_str(&format!(
        "- Environment: `{}` (256MiB copy median {:.2} MiB/s)\n",
        summary.environment_status, probe.copy_256_mib_median
    ));
    output.push_str(&format!(
        "- Release gate status: `{}`\n",
        summary.release_gate_status
    ));
    output.push_str(&format!(
        "- Selected/Fastest volume: `{}` / `{}`\n",
        summary.selected_volume_path, summary.fastest_available_volume
    ));
    output.push_str(&format!(
        "- I/O hotspot summary: `{}`\n",
        summary.io_hotspot_summary
    ));
    output.push_str(&format!(
        "- Pack-core gate: `{}` median {:.3} ms\n",
        summary.pack_core_gate, summary.hig_pack_core.median_ms
    ));
    output.push_str(&format!(
        "- Standalone second median: `{:.3} ms`\n",
        summary.hig_standalone_second.median_ms
    ));
    output.push_str(&format!(
        "- Summary + quiet CLI-wall gate: `{}` median {:.3} ms\n",
        summary.cli_wall_gate, summary.hig_cli_wall.median_ms
    ));
    output.push_str(&format!(
        "- Full + JSON CLI-wall median: `{:.3} ms`\n",
        summary.hig_cli_wall_full.median_ms
    ));
    output.push_str(&format!(
        "- Size-quality gate: `{}`\n\n",
        summary.size_quality_gate
    ));
    if let Some(incremental) = &summary.incremental {
        output.push_str("## Incremental Scenario\n\n");
        output.push_str(&format!(
            "- Modified files: `{}`\n",
            incremental.modified_files.join(", ")
        ));
        output.push_str(&format!(
            "- Hig pack-core: `{:.3} ms`\n",
            incremental.hig_pack_core_ms
        ));
        output.push_str(&format!(
            "- Hig CLI-wall: `{:.3} ms`\n",
            incremental.hig_cli_wall_ms
        ));
        output.push_str(&format!(
            "- zip CLI-wall: `{}`\n",
            incremental
                .zip_cli_wall_ms
                .map(|value| format!("{value:.3} ms"))
                .unwrap_or_else(|| "not available".to_string())
        ));
        output.push_str(&format!(
            "- Cache hit rate: `{:.2}%`\n",
            incremental.cache_hit_rate
        ));
        output.push_str(&format!(
            "- Solid groups/files: `{}/{}`\n",
            incremental.solid_groups, incremental.solid_files
        ));
        output.push_str(&format!(
            "- Journal bytes/entries after: `{}/{}`\n\n",
            incremental.journal_bytes_after, incremental.journal_entries_after
        ));
    }
    output.push_str("## Daemon Hot Path\n\n");
    let daemon_gap_ms = summary.hig_pack_core.median_ms - summary.hig_standalone_second.median_ms;
    output.push_str(&format!(
        "- Daemon pack-core minus standalone second: `{daemon_gap_ms:.3} ms`\n"
    ));
    output.push_str(&format!(
        "- Summary response savings vs Full JSON CLI-wall: `{:.3} ms`\n",
        summary.hig_cli_wall_full.median_ms - summary.hig_cli_wall.median_ms
    ));
    if let Some(row) = rows
        .iter()
        .find(|row| row.tool == "higv2 balanced secure daemon")
    {
        output.push_str(&format!(
            "- Socket connect / roundtrip: `{}/{}` us\n",
            row.socket_connect_us.unwrap_or_default(),
            row.socket_pack_roundtrip_us.unwrap_or_default()
        ));
        output.push_str(&format!(
            "- Daemon auth / job execute: `{}/{}` us\n",
            row.daemon_auth_us.unwrap_or_default(),
            row.daemon_job_execute_us.unwrap_or_default()
        ));
        output.push_str(&format!(
            "- Response serialize / client decode / bytes: `{}/{}/{}`\n",
            row.response_serialize_us.unwrap_or_default(),
            row.client_decode_us.unwrap_or_default(),
            row.daemon_response_bytes.unwrap_or_default()
        ));
        output.push_str(&format!(
            "- Cache commit: `{} ms`\n",
            row.cache_index_commit_ms.unwrap_or_default()
        ));
    }
    output.push_str("\n## Bottleneck Readout\n\n");
    if daemon_gap_ms > 100.0 {
        output.push_str(
            "1. Daemon job execution and scheduler/cache ownership are the primary gap until internal execution is separated further.\n",
        );
        output.push_str(
            "2. Socket roundtrip and response serialization are the second measured source.\n",
        );
        output.push_str(
            "3. Client response decoding/report mode is the third measured source; compare Summary and Full rows.\n",
        );
    } else {
        output.push_str(
            "- Daemon-to-standalone gap is within the 100ms release tolerance; preserve this path as a regression gate.\n",
        );
    }
    output
}

fn top_level_sizes(root: &Path) -> anyhow::Result<Vec<(String, u64)>> {
    let mut rows = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let metadata = entry.metadata()?;
        let bytes = if metadata.is_dir() {
            dir_size(&path)?
        } else if metadata.is_file() {
            metadata.len()
        } else {
            0
        };
        rows.push((name, bytes));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.1));
    rows.truncate(20);
    Ok(rows)
}

fn optional_to_string(value: Option<impl ToString>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn reduction_pct(input_bytes: u64, archive_bytes: u64) -> f64 {
    if input_bytes == 0 {
        0.0
    } else {
        (1.0 - archive_bytes as f64 / input_bytes as f64) * 100.0
    }
}

fn dir_size(path: &Path) -> anyhow::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name == OsStr::new(".hig-cache") {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += dir_size(&entry.path())?;
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn count_files(path: &Path) -> anyhow::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name == OsStr::new(".hig-cache") {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += count_files(&entry.path())?;
        } else if metadata.is_file() {
            total += 1;
        }
    }
    Ok(total)
}

fn select_benchmark_volume(
    requested: Option<&Path>,
    output_parent: Option<&Path>,
) -> anyhow::Result<BenchmarkVolumeSelection> {
    let mut candidates = Vec::new();
    if let Some(path) = requested {
        candidates.push(path.to_path_buf());
    }
    candidates.push(std::env::current_dir()?);
    if let Some(path) = output_parent {
        candidates.push(path.to_path_buf());
    }
    candidates.push(std::env::temp_dir());
    candidates.push(PathBuf::from("/tmp"));
    candidates.push(PathBuf::from("/private/tmp"));
    let mut unique_candidates = Vec::new();
    for candidate in candidates {
        if !unique_candidates.contains(&candidate) {
            unique_candidates.push(candidate);
        }
    }

    let mut probes = Vec::new();
    for candidate in unique_candidates {
        if fs::create_dir_all(&candidate).is_err() {
            continue;
        }
        if let Ok(probe) = copy_probe(&candidate) {
            probes.push(probe);
        }
    }
    let selected = if let Some(requested) = requested {
        let requested = requested
            .canonicalize()
            .unwrap_or_else(|_| requested.to_path_buf());
        probes
            .iter()
            .find(|probe| {
                probe
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| probe.path.clone())
                    == requested
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "requested benchmark directory is not usable: {}",
                    requested.display()
                )
            })?
    } else {
        probes
            .iter()
            .find(|probe| probe.qualified)
            .cloned()
            .or_else(|| {
                probes
                    .iter()
                    .max_by(|left, right| {
                        left.copy_256_mib_median
                            .partial_cmp(&right.copy_256_mib_median)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .cloned()
            })
            .ok_or_else(|| anyhow::anyhow!("no usable benchmark directory found"))?
    };
    Ok(BenchmarkVolumeSelection { selected, probes })
}

fn copy_probe(path: &Path) -> anyhow::Result<CopyProbe> {
    let probe_dir = benchmark_work_dir(Some(path))?;
    let copy_32 = copy_speed_samples(&probe_dir, 32 * 1024 * 1024, 3)?;
    let copy_256 = copy_speed_samples(&probe_dir, 256 * 1024 * 1024, 3)?;
    let (free_bytes, used_percent) = volume_stats(path)?;
    let probe = CopyProbe {
        path: path.to_path_buf(),
        free_bytes,
        used_percent,
        copy_32_mib_median: median(&copy_32),
        copy_32_mib_p95: p95(&copy_32),
        copy_256_mib_median: median(&copy_256),
        copy_256_mib_p95: p95(&copy_256),
        qualified: median(&copy_256) >= 650.0
            && p95(&copy_256) >= 500.0
            && free_bytes >= 20 * 1024 * 1024 * 1024
            && used_percent < 90.0,
    };
    let _ = fs::remove_dir_all(probe_dir);
    Ok(probe)
}

fn copy_speed_samples(dir: &Path, bytes: usize, runs: usize) -> anyhow::Result<Vec<f64>> {
    let source = dir.join(format!("copy-source-{bytes}.bin"));
    let mut file = fs::File::create(&source)?;
    let chunk = vec![0xA5_u8; 1024 * 1024];
    let mut remaining = bytes;
    while remaining > 0 {
        let len = remaining.min(chunk.len());
        file.write_all(&chunk[..len])?;
        remaining -= len;
    }
    drop(file);

    let mut speeds = Vec::with_capacity(runs);
    for run in 0..runs {
        let target = dir.join(format!("copy-target-{bytes}-{run}.bin"));
        let started = Instant::now();
        copy_file_buffered(&source, &target)?;
        let seconds = started.elapsed().as_secs_f64().max(0.000_001);
        speeds.push(bytes as f64 / 1024.0 / 1024.0 / seconds);
        let _ = fs::remove_file(target);
    }
    let _ = fs::remove_file(source);
    Ok(speeds)
}

fn copy_file_buffered(source: &Path, target: &Path) -> anyhow::Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(target)?;
    let mut buffer = vec![0_u8; 8 * 1024 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    output.flush()?;
    Ok(())
}

fn volume_stats(path: &Path) -> anyhow::Result<(u64, f64)> {
    let output = ProcessCommand::new("df").arg("-k").arg(path).output()?;
    if !output.status.success() {
        anyhow::bail!("df failed for {}", path.display());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("df output missing data line"))?;
    let columns = line.split_whitespace().collect::<Vec<_>>();
    if columns.len() < 5 {
        anyhow::bail!("unexpected df output: {line}");
    }
    let free_kib = columns[3].parse::<u64>()?;
    let used_percent = columns[4].trim_end_matches('%').parse::<f64>()?;
    Ok((free_kib * 1024, used_percent))
}

fn median(values: &[f64]) -> f64 {
    percentile(values, 0.5)
}

fn p95(values: &[f64]) -> f64 {
    percentile(values, 0.95)
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values[index]
}

fn benchmark_work_dir(base: Option<&Path>) -> anyhow::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = base
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let path = root.join(format!("hig-bench-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn command_exists(name: &str) -> bool {
    ProcessCommand::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn warm_project_sample_serializes_key_telemetry() {
        let sample = WarmProjectSample {
            duration_us: 123_000,
            project_verify_us: 10,
            plan_us: 20,
            read_us: 30,
            compression_us: 40,
            crypto_us: 50,
            manifest_serialize_us: 60,
            manifest_compress_us: 70,
            manifest_encrypt_us: 80,
            output_create_us: 90,
            output_preallocate_us: 91,
            output_header_write_us: 92,
            output_manifest_write_us: 93,
            output_payload_read_us: 94,
            output_payload_write_us: 95,
            output_write_us: 100,
            output_flush_us: 110,
            output_fsync_us: 115,
            output_rename_us: 120,
            cache_commit_us: 130,
            unattributed_us: 140,
            prepared_object_hits: 2,
            prepared_object_misses: 1,
            cached_payload_open_count: 0,
            cached_payload_read_bytes: 0,
            payload_source_memory_bytes: 4096,
        };
        let value = serde_json::to_value(&sample).unwrap();
        assert_eq!(value["duration_us"], 123_000);
        assert_eq!(value["project_verify_us"], 10);
        assert_eq!(value["output_payload_write_us"], 95);
        assert_eq!(value["output_fsync_us"], 115);
        assert_eq!(value["payload_source_memory_bytes"], 4096);
    }

    #[test]
    fn warm_stage_stats_and_hotspots_use_median_and_p95() {
        let samples = vec![
            WarmProjectSample {
                duration_us: 100_000,
                output_write_us: 1_000,
                manifest_compress_us: 300,
                crypto_us: 50,
                ..empty_warm_project_sample()
            },
            WarmProjectSample {
                duration_us: 120_000,
                output_write_us: 3_000,
                manifest_compress_us: 700,
                crypto_us: 60,
                ..empty_warm_project_sample()
            },
            WarmProjectSample {
                duration_us: 140_000,
                output_write_us: 2_000,
                manifest_compress_us: 500,
                crypto_us: 70,
                ..empty_warm_project_sample()
            },
        ];
        let stats = warm_stage_stats(&samples);
        let output_write = stats
            .iter()
            .find(|stat| stat.stage == "output_write_us")
            .unwrap();
        assert_eq!(output_write.median_us, 2_000);
        assert_eq!(output_write.p95_us, 3_000);

        let hotspots = top_warm_hotspots(&stats, 2);
        assert_eq!(hotspots[0].stage, "output_write_us");
        assert_eq!(hotspots[1].stage, "manifest_compress_us");
    }

    #[test]
    fn project_warm_gate_uses_median_not_single_sample() {
        let durations = [90.0, 120.0, 149.0, 199.0, 400.0];
        let stats = SampleStats::from_values(&durations);
        assert!(stats.median_ms < 150.0);
        assert_eq!(stats.median_ms, 149.0);
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

    #[test]
    fn release_gate_status_distinguishes_environment_from_product_failure() {
        let mut probe = CopyProbe {
            path: PathBuf::from("/tmp"),
            free_bytes: 32 * 1024 * 1024 * 1024,
            used_percent: 50.0,
            copy_32_mib_median: 700.0,
            copy_32_mib_p95: 650.0,
            copy_256_mib_median: 700.0,
            copy_256_mib_p95: 650.0,
            qualified: true,
        };
        assert_eq!(release_gate_status(&probe, true, true, true), "PASS");
        assert_eq!(
            release_gate_status(&probe, false, true, true),
            "FAILED_QUALIFIED_VOLUME"
        );
        probe.qualified = false;
        assert_eq!(
            release_gate_status(&probe, false, false, true),
            "NOT_ABSOLUTE_PASS_ENV_UNQUALIFIED"
        );
    }

    #[test]
    fn same_volume_detection_uses_filesystem_device() {
        let root = std::env::temp_dir().join(format!(
            "hig-volume-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        assert!(paths_on_same_volume(&root, &nested));
        fs::remove_dir_all(&root).unwrap();
    }

    fn empty_warm_project_sample() -> WarmProjectSample {
        WarmProjectSample {
            duration_us: 0,
            project_verify_us: 0,
            plan_us: 0,
            read_us: 0,
            compression_us: 0,
            crypto_us: 0,
            manifest_serialize_us: 0,
            manifest_compress_us: 0,
            manifest_encrypt_us: 0,
            output_create_us: 0,
            output_preallocate_us: 0,
            output_header_write_us: 0,
            output_manifest_write_us: 0,
            output_payload_read_us: 0,
            output_payload_write_us: 0,
            output_write_us: 0,
            output_flush_us: 0,
            output_fsync_us: 0,
            output_rename_us: 0,
            cache_commit_us: 0,
            unattributed_us: 0,
            prepared_object_hits: 0,
            prepared_object_misses: 0,
            cached_payload_open_count: 0,
            cached_payload_read_bytes: 0,
            payload_source_memory_bytes: 0,
        }
    }
}
