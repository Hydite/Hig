use crate::benchmark;
use crate::cli::{BenchSuite, ReportFlags, ReportMode};
use crate::output::print_report;
use crate::runtime::{
    effective_kdf_profile, effective_solid, pack_with_daemon_policy, validate_pack_encryption_args,
};
use clap::Args;
use hig_core::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, DaemonMode, EncryptionMode, KdfProfile,
    ManifestFormat, PackOptions, PayloadMemoryMode, PipelineOptions, ProjectMode, SolidMode,
    SpeedMode, UnpackOptions, bench, inspect_archive, migrate_archive, unpack,
};
use std::io::{self, Read};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub(crate) struct PackArgs {
    pub(crate) input_dir: PathBuf,
    #[arg(short, long)]
    pub(crate) output: PathBuf,
    #[arg(long)]
    pub(crate) password: Option<String>,
    #[arg(long, conflicts_with = "password")]
    pub(crate) password_stdin: bool,
    #[arg(
        long,
        default_value = "password",
        help = "Encryption mode: password uses Argon2id + ChaCha20-Poly1305; none provides no confidentiality or authentication"
    )]
    pub(crate) encryption: EncryptionMode,
    #[arg(long)]
    pub(crate) cache_dir: Option<PathBuf>,
    #[arg(long)]
    pub(crate) threads: Option<usize>,
    #[arg(long, default_value = "zstd")]
    pub(crate) compression: Compression,
    #[arg(
        long,
        help = "Explicit zstd level; balanced otherwise selects level 5 or probes large chunks"
    )]
    pub(crate) level: Option<i32>,
    #[arg(long)]
    pub(crate) no_cache: bool,
    #[arg(long, default_value = "higv2")]
    pub(crate) format: ArchiveFormat,
    #[arg(long, default_value = "compact")]
    pub(crate) manifest_format: ManifestFormat,
    #[arg(long)]
    pub(crate) no_batch: bool,
    #[arg(long, default_value_t = 65_536)]
    pub(crate) small_file_threshold: u64,
    #[arg(long, default_value_t = 4_194_304)]
    pub(crate) max_batch_raw_bytes: u64,
    #[arg(long)]
    pub(crate) no_chunk: bool,
    #[arg(long, default_value_t = 8_388_608)]
    pub(crate) chunk_file_threshold: u64,
    #[arg(long, default_value_t = 1_048_576)]
    pub(crate) chunk_size: u64,
    #[arg(
        long,
        default_value = "balanced",
        help = "balanced keeps the safest defaults; fastest trusts metadata and reuses locally sealed ciphertext, revealing block equality"
    )]
    pub(crate) speed: SpeedMode,
    #[arg(
        long,
        help = "KDF profile: secure, interactive, or benchmark-only fast-bench. fastest defaults to interactive unless explicitly set"
    )]
    pub(crate) kdf_profile: Option<KdfProfile>,
    #[arg(
        long,
        help = "Trust path/size/mtime/permissions metadata to reuse cached hashes without rereading files. Fast, but unsafe if file contents changed while metadata was preserved. --speed fastest enables this automatically."
    )]
    pub(crate) trust_metadata: bool,
    #[arg(
        long,
        help = "Use a previously unlocked in-memory Hig session key and skip KDF"
    )]
    pub(crate) use_session: bool,
    #[arg(
        long,
        default_value = "auto",
        help = "Daemon mode: auto, off, or required"
    )]
    pub(crate) daemon: DaemonMode,
    #[arg(
        long,
        default_value = "auto",
        help = "Project snapshot mode: auto, off, or required"
    )]
    pub(crate) project: ProjectMode,
    #[arg(long, default_value = "auto", help = "Solid grouping: auto or off")]
    pub(crate) solid: SolidMode,
    #[arg(
        long,
        default_value = "adaptive",
        help = "Payload memory mode: adaptive or low (64 MiB with disk spooling)"
    )]
    pub(crate) memory_mode: PayloadMemoryMode,
    #[arg(long, help = "Print full human telemetry")]
    pub(crate) verbose: bool,
    #[arg(long, help = "Print complete PackReport as JSON")]
    pub(crate) json: bool,
    #[arg(long, help = "Suppress success output")]
    pub(crate) quiet: bool,
}

#[derive(Debug, Args)]
pub(crate) struct UnpackArgs {
    pub(crate) archive_file: PathBuf,
    #[arg(short = 'd', long)]
    pub(crate) output_dir: PathBuf,
    #[arg(long)]
    pub(crate) password: Option<String>,
    #[arg(long, conflicts_with = "password")]
    pub(crate) password_stdin: bool,
    #[arg(long)]
    pub(crate) overwrite: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InspectArgs {
    pub(crate) archive_file: PathBuf,
    #[arg(long)]
    pub(crate) password: Option<String>,
    #[arg(long, conflicts_with = "password")]
    pub(crate) password_stdin: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct MigrateArgs {
    pub(crate) source: PathBuf,
    #[arg(short = 'o', long)]
    pub(crate) output: PathBuf,
    #[arg(long)]
    pub(crate) password: Option<String>,
    #[arg(long, conflicts_with = "password")]
    pub(crate) password_stdin: bool,
    #[arg(long)]
    pub(crate) target_password: Option<String>,
    #[arg(long, conflicts_with = "target_password")]
    pub(crate) target_password_stdin: bool,
    #[arg(long, default_value = "password")]
    pub(crate) encryption: EncryptionMode,
    #[arg(long)]
    pub(crate) overwrite: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct BenchArgs {
    pub(crate) input_dir: PathBuf,
    #[arg(short, long, default_value = "bench.hig")]
    pub(crate) output: PathBuf,
    #[arg(long)]
    pub(crate) password: Option<String>,
    #[arg(long, conflicts_with = "password")]
    pub(crate) password_stdin: bool,
    #[arg(
        long,
        default_value = "password",
        help = "Encryption mode: password uses Argon2id + ChaCha20-Poly1305; none provides no confidentiality or authentication"
    )]
    pub(crate) encryption: EncryptionMode,
    #[arg(long)]
    pub(crate) cache_dir: Option<PathBuf>,
    #[arg(long)]
    pub(crate) threads: Option<usize>,
    #[arg(long, default_value = "zstd")]
    pub(crate) compression: Compression,
    #[arg(
        long,
        help = "Explicit zstd level; balanced otherwise selects level 5 or probes large chunks"
    )]
    pub(crate) level: Option<i32>,
    #[arg(long)]
    pub(crate) no_cache: bool,
    #[arg(long, default_value = "higv2")]
    pub(crate) format: ArchiveFormat,
    #[arg(long, default_value = "compact")]
    pub(crate) manifest_format: ManifestFormat,
    #[arg(long)]
    pub(crate) no_batch: bool,
    #[arg(long, default_value_t = 65_536)]
    pub(crate) small_file_threshold: u64,
    #[arg(long, default_value_t = 4_194_304)]
    pub(crate) max_batch_raw_bytes: u64,
    #[arg(long)]
    pub(crate) no_chunk: bool,
    #[arg(long, default_value_t = 8_388_608)]
    pub(crate) chunk_file_threshold: u64,
    #[arg(long, default_value_t = 1_048_576)]
    pub(crate) chunk_size: u64,
    #[arg(long, default_value = "balanced")]
    pub(crate) speed: SpeedMode,
    #[arg(
        long,
        help = "KDF profile: secure, interactive, or fast-bench. fast-bench is for benchmarking only and is not recommended for production archives."
    )]
    pub(crate) kdf_profile: Option<KdfProfile>,
    #[arg(
        long,
        help = "Trust path/size/mtime/permissions metadata to reuse cached hashes without rereading files. Fast, but unsafe if file contents changed while metadata was preserved. --speed fastest enables this automatically."
    )]
    pub(crate) trust_metadata: bool,
    #[arg(
        long,
        help = "Use a previously unlocked in-memory Hig session key and skip KDF"
    )]
    pub(crate) use_session: bool,
    #[arg(
        long,
        default_value = "auto",
        help = "Daemon mode: auto, off, or required"
    )]
    pub(crate) daemon: DaemonMode,
    #[arg(long, default_value = "auto", help = "Solid grouping: auto or off")]
    pub(crate) solid: SolidMode,
    #[arg(long, help = "Print full human telemetry")]
    pub(crate) verbose: bool,
    #[arg(long, help = "Print complete PackReport as JSON")]
    pub(crate) json: bool,
    #[arg(long, help = "Suppress success output")]
    pub(crate) quiet: bool,
    #[arg(
        long,
        help = "Compare Hig against zip, tar+gzip, and tar+zstd, writing v1.10.1 benchmark artifacts"
    )]
    pub(crate) compare: bool,
    #[arg(
        long,
        help = "Directory used for benchmark temporary files and copy baseline qualification"
    )]
    pub(crate) bench_dir: Option<PathBuf>,
    #[arg(
        long,
        default_value = "all",
        help = "Benchmark corpus suite: source, lobehub, small500, textmix, repeat4m, random8m, binarymix, or all"
    )]
    pub(crate) bench_suite: BenchSuite,
}

pub(crate) fn handle_pack(args: PackArgs) -> anyhow::Result<()> {
    let password = resolve_password(args.password, args.password_stdin, "password")?;
    let report_mode = ReportFlags {
        verbose: args.verbose,
        json: args.json,
        quiet: args.quiet,
    }
    .mode()?;
    validate_pack_encryption_args(args.encryption, password.as_deref(), args.use_session)?;
    let options = pack_options(
        args.input_dir,
        args.output,
        password,
        args.encryption,
        args.cache_dir,
        args.threads,
        args.compression,
        args.level,
        args.no_cache,
        args.format,
        args.manifest_format,
        args.no_batch,
        args.small_file_threshold,
        args.max_batch_raw_bytes,
        args.no_chunk,
        args.chunk_file_threshold,
        args.chunk_size,
        args.speed,
        args.kdf_profile,
        args.trust_metadata,
        args.use_session,
        args.daemon,
        Some(args.project),
        args.solid,
        Some(args.memory_mode),
    );
    let report = pack_with_daemon_policy(options, report_mode)?;
    print_report("pack", &report, report_mode)
}

pub(crate) fn handle_unpack(args: UnpackArgs) -> anyhow::Result<()> {
    let password = resolve_password(args.password, args.password_stdin, "password")?;
    unpack(UnpackOptions {
        archive_file: args.archive_file,
        output_dir: args.output_dir,
        password,
        overwrite: args.overwrite,
    })?;
    println!("unpack: ok");
    Ok(())
}

pub(crate) fn handle_inspect(args: InspectArgs) -> anyhow::Result<()> {
    let password = resolve_password(args.password, args.password_stdin, "password")?;
    let inspection = inspect_archive(&args.archive_file, password.as_deref())?;
    if args.json {
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
    Ok(())
}

pub(crate) fn handle_migrate(args: MigrateArgs) -> anyhow::Result<()> {
    let password = resolve_password(args.password, args.password_stdin, "source password")?;
    let target_password = resolve_password(
        args.target_password,
        args.target_password_stdin,
        "target password",
    )?;
    anyhow::ensure!(
        args.encryption != EncryptionMode::None || target_password.is_none(),
        "--target-password cannot be used with --encryption none"
    );
    let report = migrate_archive(
        &args.source,
        &args.output,
        password.as_deref(),
        target_password.as_deref(),
        args.encryption,
        args.overwrite,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "migrate: {} -> {} format={:?}->{:?} files={} bytes={} archive_bytes={} published={}",
            report.source_archive,
            report.target_archive,
            report.source_format,
            report.target_format,
            report.target_files,
            report.target_bytes,
            report.target_archive_bytes,
            report.published
        );
    }
    Ok(())
}

pub(crate) fn handle_bench(args: BenchArgs) -> anyhow::Result<()> {
    let password = resolve_password(args.password, args.password_stdin, "password")?;
    let report_mode = ReportFlags {
        verbose: args.verbose,
        json: args.json,
        quiet: args.quiet,
    }
    .mode()?;
    validate_pack_encryption_args(args.encryption, password.as_deref(), args.use_session)?;
    let kdf_profile = effective_kdf_profile(args.speed, args.kdf_profile);
    let solid = effective_solid(args.speed, args.solid);
    if args.compare {
        if password.is_none() {
            anyhow::bail!("--compare requires --password for secure benchmark rows");
        }
        benchmark::run_compare(benchmark::CompareOptions {
            input_dir: args.input_dir,
            output: args.output,
            password,
            cache_dir: args.cache_dir,
            threads: args.threads,
            compression: args.compression,
            level: args.level,
            use_cache: !args.no_cache,
            batch: BatchOptions {
                enabled: !args.no_batch,
                small_file_threshold: args.small_file_threshold,
                max_batch_raw_bytes: args.max_batch_raw_bytes,
            },
            chunk: ChunkOptions {
                enabled: !args.no_chunk,
                chunk_file_threshold: args.chunk_file_threshold,
                chunk_size: args.chunk_size,
            },
            bench_dir: args.bench_dir,
            bench_suite: args.bench_suite,
            manifest_format: args.manifest_format,
            use_session: args.use_session,
            daemon: args.daemon,
            solid,
            report_mode,
        })?;
        return Ok(());
    }
    let report = bench(pack_options(
        args.input_dir,
        args.output,
        password,
        args.encryption,
        args.cache_dir,
        args.threads,
        args.compression,
        args.level,
        args.no_cache,
        args.format,
        args.manifest_format,
        args.no_batch,
        args.small_file_threshold,
        args.max_batch_raw_bytes,
        args.no_chunk,
        args.chunk_file_threshold,
        args.chunk_size,
        args.speed,
        Some(kdf_profile),
        args.trust_metadata,
        args.use_session,
        args.daemon,
        None,
        solid,
        None,
    ))?;
    print_report("bench:first", &report.first, report_mode)?;
    print_report("bench:second", &report.second, report_mode)?;
    if report_mode != ReportMode::Quiet && report.second.duration.as_secs_f64() > 0.0 {
        println!(
            "bench:speedup {:.2}x",
            report.first.duration.as_secs_f64() / report.second.duration.as_secs_f64()
        );
    }
    Ok(())
}

fn resolve_password(
    direct: Option<String>,
    from_stdin: bool,
    label: &str,
) -> anyhow::Result<Option<String>> {
    if !from_stdin {
        return Ok(direct);
    }
    let mut value = String::new();
    io::stdin().take(64 * 1024 + 1).read_to_string(&mut value)?;
    anyhow::ensure!(value.len() <= 64 * 1024, "{label} exceeds stdin limit");
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    anyhow::ensure!(!value.is_empty(), "{label} from stdin is empty");
    Ok(Some(value))
}

#[allow(clippy::too_many_arguments)]
fn pack_options(
    input_dir: PathBuf,
    output: PathBuf,
    password: Option<String>,
    encryption: EncryptionMode,
    cache_dir: Option<PathBuf>,
    threads: Option<usize>,
    compression: Compression,
    level: Option<i32>,
    no_cache: bool,
    format: ArchiveFormat,
    manifest_format: ManifestFormat,
    no_batch: bool,
    small_file_threshold: u64,
    max_batch_raw_bytes: u64,
    no_chunk: bool,
    chunk_file_threshold: u64,
    chunk_size: u64,
    speed: SpeedMode,
    requested_kdf: Option<KdfProfile>,
    trust_metadata: bool,
    use_session: bool,
    daemon: DaemonMode,
    project: Option<ProjectMode>,
    solid: SolidMode,
    memory_mode: Option<PayloadMemoryMode>,
) -> PackOptions {
    PackOptions {
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
        kdf_profile: effective_kdf_profile(speed, requested_kdf),
        sealed_cache: speed == SpeedMode::Fastest,
        manifest_format,
        use_session,
        session_required: use_session,
        session_ttl_secs: None,
        solid: effective_solid(speed, solid),
        pipeline: PipelineOptions {
            daemon_mode: daemon,
            project_mode: project.unwrap_or(ProjectMode::Auto),
            payload_memory_mode: memory_mode.unwrap_or(PayloadMemoryMode::Adaptive),
            ..PipelineOptions::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(Debug, clap::Subcommand)]
    enum TestCommand {
        Pack(PackArgs),
        Migrate(MigrateArgs),
    }

    #[test]
    fn pack_args_keep_low_memory_mode() {
        let cli = TestCli::try_parse_from([
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
            TestCommand::Pack(args) if args.memory_mode == PayloadMemoryMode::Low
        ));
    }

    #[test]
    fn migrate_args_default_to_password_encryption() {
        let cli =
            TestCli::try_parse_from(["hig", "migrate", "source.hig", "--output", "target.hig"])
                .unwrap();
        assert!(matches!(
            cli.command,
            TestCommand::Migrate(args) if args.encryption == EncryptionMode::Password
        ));
    }
}
