use clap::{Parser, Subcommand};
use hig_core::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, EncryptionMode, KdfProfile,
    ManifestFormat, PackOptions, PackReport, SessionMaterial, SolidMode, SpeedMode, UnpackOptions,
    bench, clear_session, default_session_ttl, derive_key, derive_session_binding, pack,
    random_bytes, run_session_server, session_socket_path, session_status, unpack,
};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Subcommand)]
enum Command {
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
        #[arg(long, default_value = "auto", help = "Solid grouping: auto or off")]
        solid: SolidMode,
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
        #[arg(long, default_value = "auto", help = "Solid grouping: auto or off")]
        solid: SolidMode,
        #[arg(
            long,
            help = "Compare Hig against zip, tar+gzip, and tar+zstd, writing artifacts/hig-v1.6.0-benchmark.md"
        )]
        compare: bool,
        #[arg(
            long,
            help = "Directory used for benchmark temporary files and copy baseline qualification"
        )]
        bench_dir: Option<PathBuf>,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
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
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        socket: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
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
            solid,
        } => {
            validate_pack_encryption_args(encryption, password.as_deref(), use_session)?;
            let kdf_profile = effective_kdf_profile(speed, kdf_profile);
            let solid = effective_solid(speed, solid);
            let report = pack(PackOptions {
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
            })?;
            print_report("pack", &report);
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
            solid,
            compare,
            bench_dir,
        } => {
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
                    manifest_format,
                    use_session,
                    solid,
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
            })?;
            print_report("bench:first", &report.first);
            print_report("bench:second", &report.second);
            if report.second.duration.as_secs_f64() > 0.0 {
                println!(
                    "bench:speedup {:.2}x",
                    report.first.duration.as_secs_f64() / report.second.duration.as_secs_f64()
                );
            }
        }
        Command::Session { command } => handle_session(command)?,
    }
    Ok(())
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
            unlock_session_for_cache(&cache_dir, &password, kdf_profile, ttl_secs)?;
            let (age, ttl) = session_status(&cache_dir)?.unwrap_or((0, ttl_secs));
            println!(
                "session: unlocked cache_dir={} ttl_secs={} age_secs={} kdf_ms={}",
                cache_dir.display(),
                ttl,
                age,
                started.elapsed().as_millis()
            );
            Ok(())
        }
        SessionCommand::Status { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            if let Some((age, ttl)) = session_status(&cache_dir)? {
                println!(
                    "session: active cache_dir={} age_secs={} ttl_secs={}",
                    cache_dir.display(),
                    age,
                    ttl
                );
            } else {
                println!("session: inactive cache_dir={}", cache_dir.display());
            }
            Ok(())
        }
        SessionCommand::Clear { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let cleared = clear_session(&cache_dir)?;
            println!(
                "session: {} cache_dir={}",
                if cleared { "cleared" } else { "inactive" },
                cache_dir.display()
            );
            Ok(())
        }
        SessionCommand::Serve { socket } => {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            let material: SessionMaterial = serde_json::from_str(line.trim())?;
            run_session_server(&socket, material)
        }
    }
}

fn unlock_session_for_cache(
    cache_dir: &Path,
    password: &str,
    kdf_profile: KdfProfile,
    ttl_secs: u64,
) -> anyhow::Result<()> {
    fs::create_dir_all(cache_dir)?;
    let kdf = kdf_profile.params();
    let salt = random_bytes::<16>();
    let key = derive_key(password, &salt, &kdf)?;
    let material = SessionMaterial {
        binding: derive_session_binding(cache_dir, kdf_profile, &kdf, EncryptionMode::Password),
        key,
        salt,
        created_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        ttl_secs,
    };
    let socket = session_socket_path(cache_dir);
    let _ = clear_session(cache_dir);
    let mut child = ProcessCommand::new(std::env::current_exe()?)
        .arg("session")
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("failed to open session server stdin"))?;
        writeln!(stdin, "{}", serde_json::to_string(&material)?)?;
    }
    for _ in 0..100 {
        if session_status(cache_dir)?.is_some() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    anyhow::bail!("session server did not become ready")
}

fn default_cache_dir() -> PathBuf {
    PathBuf::from(".hig-cache")
}

fn print_report(label: &str, report: &PackReport) {
    let seconds = report.duration.as_secs_f64().max(0.000_001);
    let mib = report.input_bytes as f64 / 1024.0 / 1024.0;
    println!(
        "{label}: files={} input_bytes={} archive_bytes={} duration_ms={} throughput_mib_s={:.2} encryption={:?} speed={:?} kdf_profile={:?} session_used={} session_lookup_ms={} session_key_age_secs={} kdf_skipped_by_session={} workers={} writer_strategy={:?} preallocated_bytes={} preallocation_enabled={} cached_payload_opens={} cached_range_opens={} cached_payload_read_bytes={} prefetched_bytes={} direct_writes={} buffered_writes={} peak_pipeline_memory_bytes={} scan_ms={} plan_ms={} kdf_ms={} kdf_overlapped_ms={} read_ms={} compression_ms={} crypto_ms={} pack_blocks_ms={} manifest_ms={} write_ms={} payload_read_ms={} payload_write_ms={} writer_wait_ms={} output_flush_ms={} output_rename_ms={} cache_hits={} cache_misses={} cache_hit_rate={:.2}% hashed_files={} metadata_hash_reuses={} scan_cache_hits={} scan_cache_misses={} scan_cache_hit_rate={:.2}% chunk_metadata_reuses={} chunk_metadata_misses={} trusted_bytes_skipped={} batch_blocks={} single_blocks={} batched_files={} batch_cache_hits={} batch_cache_misses={} solid_groups={} solid_files={} solid_cache_hits={} solid_cache_misses={} solid_group_bytes={} chunked_files={} chunk_blocks={} chunk_cache_hits={} chunk_cache_misses={} chunk_bytes_reused={} chunk_bytes_compressed={} chunk_plan_cache_hits={} chunk_plan_cache_misses={} sealed_block_hits={} sealed_block_misses={} sealed_bytes_reused={} reencrypted_cache_hits={} payload_source_cache_files={} payload_source_memory_bytes={} cache_pack_hits={} cache_pack_misses={} cache_pack_fallbacks={}",
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
        report.blocks.cache_pack_hits,
        report.blocks.cache_pack_misses,
        report.blocks.cache_pack_fallbacks
    );
    println!(
        "{label}:critical setup_ms={} cache_open_ms={} scan_kdf_wall_ms={} plan_ms={} block_prepare_ms={} cache_commit_ms={} manifest_build_ms={} output_write_ms={} cleanup_ms={} unattributed_ms={} cache_index_format={} cache_index_open_ms={} cache_index_commit_ms={} cache_shards_read={} cache_shards_written={} cache_shard_dirty_count={} l1_index_hits={} l1_metadata_hits={} l1_scratch_reuses={} rayon_pool_reused={} header_bytes={} manifest_plain_bytes={} manifest_compressed_bytes={} manifest_protected_bytes={} payload_bytes={} compression_level_counts={:?} legacy_cache_hits={} parameterized_cache_hits={} cache_policy_misses={}",
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
        report.l1.l1_index_hits,
        report.l1.l1_metadata_hits,
        report.l1.l1_scratch_reuses,
        report.l1.rayon_pool_reused,
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
    manifest_format: ManifestFormat,
    use_session: bool,
    solid: SolidMode,
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

fn run_compare(options: CompareOptions) -> anyhow::Result<()> {
    let input_dir = options.input_dir.canonicalize()?;
    let input_bytes = dir_size(&input_dir)?;
    let probe = select_benchmark_volume(options.bench_dir.as_deref(), options.output.parent())?;
    let work_dir = benchmark_work_dir(Some(&probe.path))?;
    let cache_dir = options
        .cache_dir
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
    })?;
    print_report("bench:higv2:batch:first", &first);
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
    })?;
    print_report("bench:higv2:batch:second", &second);
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
    let session_pack = pack(PackOptions {
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
    })?;
    print_report("bench:higv2:balanced:session", &session_pack);
    rows.push(row_from_report(
        "higv2 balanced secure session",
        &session_pack,
        &format!("secure session pack; unlock cost {session_unlock_ms} ms reported separately"),
    ));
    if compare_invoked_with_session {
        println!("benchmark: --use-session was set; compare still reports unlock cost separately");
    }

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
    })?;
    print_report("bench:higv2:fastest:secure:warm", &trusted);
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
    })?;
    print_report("bench:higv2:fastest:secure", &fastest_secure);
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
    })?;
    print_report(
        "bench:higv2:fastest:interactive:warm",
        &fastest_interactive_warm,
    );
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
    })?;
    print_report("bench:higv2:fastest:interactive", &fastest_interactive);
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
    })?;
    print_report("bench:higv2:fastest:fast-bench", &fastest_bench);
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
    })?;
    print_report("bench:higv2:no-batch", &no_batch);
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
    })?;
    print_report("bench:higv2:no-chunk", &no_chunk);
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
    })?;
    print_report("bench:higv2:no-chunk:second", &no_chunk_second);
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
    })?;
    print_report("bench:higv2:none", &no_encryption);
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
    })?;
    print_report("bench:higv1:legacy", &legacy);
    rows.push(row_from_report(
        "higv1 legacy",
        &legacy,
        "legacy one-file-per-block format",
    ));

    rows.push(run_zip_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_tar_gzip_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_tar_zstd_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_7z_benchmark(&input_dir, input_bytes));

    let markdown = render_markdown(&input_dir, &rows, &probe);
    fs::create_dir_all("artifacts")?;
    fs::write("artifacts/hig-v1.6.0-benchmark.md", markdown)?;
    println!("benchmark: wrote artifacts/hig-v1.6.0-benchmark.md");
    Ok(())
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
        notes: notes.to_string(),
    }
}

fn render_markdown(input_dir: &Path, rows: &[BenchmarkRow], probe: &CopyProbe) -> String {
    let mut output = String::new();
    output.push_str("# Hig v1.6.0 Benchmark\n\n");
    output.push_str(&format!("Input: `{}`\n\n", input_dir.display()));
    output.push_str("## Environment Qualification\n\n");
    output.push_str(&format!(
        "Status: `{}`\n\n",
        if probe.qualified {
            "QUALIFIED"
        } else {
            "ENVIRONMENT_NOT_QUALIFIED"
        }
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

fn select_benchmark_volume(
    requested: Option<&Path>,
    output_parent: Option<&Path>,
) -> anyhow::Result<CopyProbe> {
    let mut candidates = Vec::new();
    if let Some(path) = requested {
        candidates.push(path.to_path_buf());
    }
    if let Some(path) = output_parent {
        candidates.push(path.to_path_buf());
    }
    candidates.push(std::env::temp_dir());
    candidates.push(PathBuf::from("/tmp"));
    candidates.push(PathBuf::from("/private/tmp"));
    candidates.sort();
    candidates.dedup();

    let mut probes = Vec::new();
    for candidate in candidates {
        if fs::create_dir_all(&candidate).is_err() {
            continue;
        }
        if let Ok(probe) = copy_probe(&candidate) {
            if probe.qualified {
                return Ok(probe);
            }
            probes.push(probe);
        }
    }
    probes
        .into_iter()
        .max_by(|left, right| {
            left.copy_256_mib_median
                .partial_cmp(&right.copy_256_mib_median)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or_else(|| anyhow::anyhow!("no usable benchmark directory found"))
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
}
