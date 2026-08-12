# HIG v1.9.7 Cold Pipeline Fusion Benchmark

Date: 2026-07-22

## Scope

This benchmark compares the pre-fusion v1.9.7 adaptive-I/O binary with the
v1.9.7 cold-pipeline-fusion candidate. Both binaries use the same archive
format, compression policy, adaptive I/O controller, and writer.

Corpus and options:

- 17,583 files;
- 505,906,599 input bytes;
- approximately 248 MB archive;
- system-disk corpus at `/private/tmp/Hig-Test/corpus-system`;
- fresh HIG cache and output path for every run;
- `--daemon off --project off --speed fastest --encryption none`;
- archive and cache deleted after every measured run;
- `sync` and a 10-second settling interval between final ABBA samples.

## Final ABBA Results

| Order | Binary | Total | Scan | Block prepare | Output write | Peak pipeline memory | Cache-pack read |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | baseline | 1.482 s | 851 ms | 427 ms | 177 ms | 860.8 MiB | 108.1 MiB |
| 2 | fusion | 1.194 s | 570 ms | 288 ms | 313 ms | 871.7 MiB | 0 MiB |
| 3 | fusion | 1.069 s | 557 ms | 309 ms | 181 ms | 867.7 MiB | 0 MiB |
| 4 | baseline | 1.188 s | 549 ms | 405 ms | 208 ms | 878.0 MiB | 108.1 MiB |

Median comparison:

| Metric | Baseline | Fusion | Change |
|---|---:|---:|---:|
| Total | 1.335 s | 1.132 s | -15.2% |
| Block prepare | 416.0 ms | 298.5 ms | -28.2% |
| Peak pipeline memory | 869.4 MiB | 869.7 MiB | +0.03% |
| Cache-pack read | 108.1 MiB | 0 MiB | -100% |

The memory ranges overlap and the median difference is below 0.1%; this is
treated as no material increase. Buffer-pool misses were 29/31 for baseline
and 31/29 for fusion, confirming equivalent allocation behavior.

## Invalidated Samples

An earlier run retained every archive and fresh cache. The system data volume
fell to 2.5 GiB free and 99% reported capacity. Later samples slowed
monotonically and one report could not be created because the volume was full.
Those samples are retained as environmental diagnostics but are excluded from
the release comparison. Generated archives and cache directories were removed;
their JSON and time reports remain.

## Correctness

The final candidate archive was unpacked and compared with the source corpus:

- source files: 17,583;
- unpacked files: 17,583;
- source bytes: 505,906,599;
- unpacked bytes: 505,906,599;
- sorted SHA-256 manifests: byte-for-byte identical.

## Verification Gates

- `cargo fmt --all --check`
- `cargo test -p hig-core -p hig-cli`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --release -p hig-cli`
- full 17,583-file unpack and SHA-256 comparison

Raw final reports are retained under:

```text
/private/tmp/Hig-Test/runs-20260722-cold-fusion-final-abba
/private/tmp/Hig-Test/runs-20260722-cold-fusion-join
```
