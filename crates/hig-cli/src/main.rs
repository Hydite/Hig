use clap::{Parser, Subcommand};
use hig_core::{Compression, PackOptions, PackReport, UnpackOptions, bench, pack, unpack};
use std::path::PathBuf;

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
        } => {
            let report = bench(PackOptions {
                input_dir,
                output_file: output,
                password,
                cache_dir,
                threads,
                compression,
                level,
                use_cache: !no_cache,
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
        "{label}: files={} input_bytes={} archive_bytes={} duration_ms={} throughput_mib_s={:.2} cache_hits={} cache_misses={} cache_hit_rate={:.2}%",
        report.input_files,
        report.input_bytes,
        report.archive_bytes,
        report.duration.as_millis(),
        mib / seconds,
        report.cache.hits,
        report.cache.misses,
        report.cache.hit_rate() * 100.0
    );
}
