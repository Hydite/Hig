use clap::{Parser, Subcommand};
use hig_core::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, PackOptions, PackReport, UnpackOptions,
    bench, pack, unpack,
};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
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
        password: String,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long, default_value = "zstd")]
        compression: Compression,
        #[arg(long, default_value_t = 1)]
        level: i32,
        #[arg(long)]
        no_cache: bool,
        #[arg(long, default_value = "higv2")]
        format: ArchiveFormat,
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
            help = "Trust path/size/mtime/permissions metadata to reuse cached hashes without rereading files. Fast, but unsafe if file contents changed while metadata was preserved."
        )]
        trust_metadata: bool,
    },
    Unpack {
        archive_file: PathBuf,
        #[arg(short = 'd', long)]
        output_dir: PathBuf,
        #[arg(long)]
        password: String,
        #[arg(long)]
        overwrite: bool,
    },
    Bench {
        input_dir: PathBuf,
        #[arg(short, long, default_value = "bench.hig")]
        output: PathBuf,
        #[arg(long)]
        password: String,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long, default_value = "zstd")]
        compression: Compression,
        #[arg(long, default_value_t = 1)]
        level: i32,
        #[arg(long)]
        no_cache: bool,
        #[arg(long, default_value = "higv2")]
        format: ArchiveFormat,
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
            help = "Trust path/size/mtime/permissions metadata to reuse cached hashes without rereading files. Fast, but unsafe if file contents changed while metadata was preserved."
        )]
        trust_metadata: bool,
        #[arg(
            long,
            help = "Compare Hig against zip and tar+zstd, writing artifacts/hig-v1.2.0-benchmark.md"
        )]
        compare: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Pack {
            input_dir,
            output,
            password,
            cache_dir,
            threads,
            compression,
            level,
            no_cache,
            format,
            no_batch,
            small_file_threshold,
            max_batch_raw_bytes,
            no_chunk,
            chunk_file_threshold,
            chunk_size,
            trust_metadata,
        } => {
            let report = pack(PackOptions {
                input_dir,
                output_file: output,
                password,
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
            cache_dir,
            threads,
            compression,
            level,
            no_cache,
            format,
            no_batch,
            small_file_threshold,
            max_batch_raw_bytes,
            no_chunk,
            chunk_file_threshold,
            chunk_size,
            trust_metadata,
            compare,
        } => {
            if compare {
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
                })?;
                return Ok(());
            }
            let report = bench(PackOptions {
                input_dir,
                output_file: output,
                password,
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
    }
    Ok(())
}

fn print_report(label: &str, report: &PackReport) {
    let seconds = report.duration.as_secs_f64().max(0.000_001);
    let mib = report.input_bytes as f64 / 1024.0 / 1024.0;
    println!(
        "{label}: files={} input_bytes={} archive_bytes={} duration_ms={} throughput_mib_s={:.2} cache_hits={} cache_misses={} cache_hit_rate={:.2}% hashed_files={} metadata_hash_reuses={} scan_cache_hits={} scan_cache_misses={} scan_cache_hit_rate={:.2}% batch_blocks={} single_blocks={} batched_files={} batch_cache_hits={} batch_cache_misses={} chunked_files={} chunk_blocks={} chunk_cache_hits={} chunk_cache_misses={} chunk_bytes_reused={} chunk_bytes_compressed={}",
        report.input_files,
        report.input_bytes,
        report.archive_bytes,
        report.duration.as_millis(),
        mib / seconds,
        report.cache.hits,
        report.cache.misses,
        report.cache.hit_rate() * 100.0,
        report.scan.hashed_files,
        report.scan.metadata_hash_reuses,
        report.scan.scan_cache_hits,
        report.scan.scan_cache_misses,
        report.scan.scan_cache_hit_rate() * 100.0,
        report.blocks.batch_blocks,
        report.blocks.single_blocks,
        report.blocks.batched_files,
        report.blocks.batch_cache_hits,
        report.blocks.batch_cache_misses,
        report.blocks.chunked_files,
        report.blocks.chunk_blocks,
        report.blocks.chunk_cache_hits,
        report.blocks.chunk_cache_misses,
        report.blocks.chunk_bytes_reused,
        report.blocks.chunk_bytes_compressed
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
    batch_blocks: Option<usize>,
    single_blocks: Option<usize>,
    batched_files: Option<usize>,
    chunked_files: Option<usize>,
    chunk_blocks: Option<usize>,
    chunk_cache_hits: Option<usize>,
    chunk_cache_misses: Option<usize>,
    notes: String,
}

#[derive(Debug)]
struct CompareOptions {
    input_dir: PathBuf,
    output: PathBuf,
    password: String,
    cache_dir: Option<PathBuf>,
    threads: Option<usize>,
    compression: Compression,
    level: i32,
    use_cache: bool,
    batch: BatchOptions,
    chunk: ChunkOptions,
}

fn run_compare(options: CompareOptions) -> anyhow::Result<()> {
    let input_dir = options.input_dir.canonicalize()?;
    let input_bytes = dir_size(&input_dir)?;
    let work_dir = benchmark_work_dir()?;
    let cache_dir = options
        .cache_dir
        .unwrap_or_else(|| work_dir.join("hig-cache"));
    let mut rows = Vec::new();

    let first = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.clone(),
        password: options.password.clone(),
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
    })?;
    print_report("bench:higv2:batch:first", &first);
    rows.push(row_from_report(
        "higv2 batch first pack",
        &first,
        "default HIGV2 batch format",
    ));

    let second = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("second.hig"),
        password: options.password.clone(),
        cache_dir: Some(cache_dir.clone()),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
    })?;
    print_report("bench:higv2:batch:second", &second);
    rows.push(row_from_report(
        "higv2 batch second pack",
        &second,
        "reuses batch/single cache but recomputes file hashes",
    ));

    let trusted = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("trusted.hig"),
        password: options.password.clone(),
        cache_dir: Some(cache_dir),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: true,
        format: ArchiveFormat::HigV2,
        batch: options.batch,
        chunk: options.chunk,
    })?;
    print_report("bench:higv2:batch:trusted-metadata", &trusted);
    rows.push(row_from_report(
        "higv2 batch second pack --trust-metadata",
        &trusted,
        "reuses metadata cached hashes and batch/single cache",
    ));

    let no_batch = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv2.no-batch.hig"),
        password: options.password.clone(),
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
    })?;
    print_report("bench:higv2:no-batch", &no_batch);
    rows.push(row_from_report(
        "higv2 --no-batch",
        &no_batch,
        "HIGV2 with batching disabled",
    ));

    let no_chunk = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv2.no-chunk.hig"),
        password: options.password.clone(),
        cache_dir: Some(work_dir.join("higv2-no-chunk-cache")),
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
    })?;
    print_report("bench:higv2:no-chunk", &no_chunk);
    rows.push(row_from_report(
        "higv2 --no-chunk",
        &no_chunk,
        "HIGV2 with large-file chunking disabled",
    ));

    let legacy = pack(PackOptions {
        input_dir: input_dir.clone(),
        output_file: options.output.with_extension("higv1.legacy.hig"),
        password: options.password,
        cache_dir: Some(work_dir.join("higv1-cache")),
        threads: options.threads,
        compression: options.compression,
        level: options.level,
        use_cache: options.use_cache,
        trust_metadata: false,
        format: ArchiveFormat::HigV1,
        batch: options.batch,
        chunk: options.chunk,
    })?;
    print_report("bench:higv1:legacy", &legacy);
    rows.push(row_from_report(
        "higv1 legacy",
        &legacy,
        "legacy one-file-per-block format",
    ));

    rows.push(run_zip_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_tar_zstd_benchmark(&input_dir, &work_dir, input_bytes)?);
    rows.push(run_7z_benchmark(&input_dir, input_bytes));

    let markdown = render_markdown(&input_dir, &rows);
    fs::create_dir_all("artifacts")?;
    fs::write("artifacts/hig-v1.2.0-benchmark.md", markdown)?;
    println!("benchmark: wrote artifacts/hig-v1.2.0-benchmark.md");
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
        batch_blocks: Some(report.blocks.batch_blocks),
        single_blocks: Some(report.blocks.single_blocks),
        batched_files: Some(report.blocks.batched_files),
        chunked_files: Some(report.blocks.chunked_files),
        chunk_blocks: Some(report.blocks.chunk_blocks),
        chunk_cache_hits: Some(report.blocks.chunk_cache_hits),
        chunk_cache_misses: Some(report.blocks.chunk_cache_misses),
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
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
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
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
        notes: "tar -cf + zstd -1".to_string(),
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
        batch_blocks: None,
        single_blocks: None,
        batched_files: None,
        chunked_files: None,
        chunk_blocks: None,
        chunk_cache_hits: None,
        chunk_cache_misses: None,
        notes: notes.to_string(),
    }
}

fn render_markdown(input_dir: &Path, rows: &[BenchmarkRow]) -> String {
    let mut output = String::new();
    output.push_str("# Hig v1.2.0 Benchmark\n\n");
    output.push_str(&format!("Input: `{}`\n\n", input_dir.display()));
    output.push_str("| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | notes |\n");
    output.push_str(
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
    );
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
        let batch_blocks = row
            .batch_blocks
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
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
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.tool,
            row.input_bytes,
            archive_bytes,
            reduction,
            duration,
            throughput,
            cache_hit_rate,
            scan_cache_hit_rate,
            batch_blocks,
            single_blocks,
            batched_files,
            chunked_files,
            chunk_blocks,
            chunk_hits,
            chunk_misses,
            row.notes
        ));
    }
    output
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

fn benchmark_work_dir() -> anyhow::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hig-bench-{}-{nanos}", std::process::id()));
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
