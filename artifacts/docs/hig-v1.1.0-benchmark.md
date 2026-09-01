# Hig v1.1.0 Benchmark

Generated with `target/debug/hig bench --compare` after v1.1.0 implementation. HIGV2 batch is the default format; HIGV2 no-batch and HIGV1 legacy are included for comparison.

## srcdata

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 156269 | 68653 | 56.07% | 259 | 0.58 | 0.00% | 0.00% | 1 | 0 | 21 | default HIGV2 batch format |
| higv2 batch second pack | 156269 | 68653 | 56.07% | 248 | 0.60 | 0.00% | 0.00% | 1 | 0 | 21 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 156269 | 68653 | 56.07% | 266 | 0.56 | 0.00% | 100.00% | 1 | 0 | 21 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 156269 | 68137 | 56.40% | 277 | 0.54 | 0.00% | 0.00% | 0 | 21 | 0 | HIGV2 with batching disabled |
| higv1 legacy | 156269 | 70703 | 54.76% | 264 | 0.56 | 0.00% | 0.00% | 0 | 21 | 0 | legacy one-file-per-block format |
| zip | 156269 | 67478 | 56.82% | 10 | 14.90 | - | - | - | - | - | zip -qr |
| tar.zst | 156269 | 66251 | 57.60% | 25 | 5.96 | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 156269 | - | - | - | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:higv2:batch:first: files=21 input_bytes=156269 archive_bytes=68653 duration_ms=259 throughput_mib_s=0.58 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=21 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=21 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=21 batch_cache_hits=0 batch_cache_misses=1
bench:higv2:batch:second: files=21 input_bytes=156269 archive_bytes=68653 duration_ms=248 throughput_mib_s=0.60 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=21 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=21 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=21 batch_cache_hits=1 batch_cache_misses=0
bench:higv2:batch:trusted-metadata: files=21 input_bytes=156269 archive_bytes=68653 duration_ms=266 throughput_mib_s=0.56 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=0 metadata_hash_reuses=21 scan_cache_hits=21 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=1 single_blocks=0 batched_files=21 batch_cache_hits=1 batch_cache_misses=0
bench:higv2:no-batch: files=21 input_bytes=156269 archive_bytes=68137 duration_ms=277 throughput_mib_s=0.54 cache_hits=0 cache_misses=21 cache_hit_rate=0.00% hashed_files=21 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=21 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=21 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv1:legacy: files=21 input_bytes=156269 archive_bytes=70703 duration_ms=264 throughput_mib_s=0.56 cache_hits=0 cache_misses=21 cache_hit_rate=0.00% hashed_files=21 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=21 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=21 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
benchmark: wrote artifacts/hig-v1.1.0-benchmark.md
```

## repetitive

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 9600000 | 1245 | 99.99% | 407 | 22.49 | 0.00% | 0.00% | 0 | 1 | 0 | default HIGV2 batch format |
| higv2 batch second pack | 9600000 | 1245 | 99.99% | 393 | 23.30 | 100.00% | 0.00% | 0 | 1 | 0 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 9600000 | 1245 | 99.99% | 243 | 37.68 | 100.00% | 100.00% | 0 | 1 | 0 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 9600000 | 1245 | 99.99% | 356 | 25.72 | 0.00% | 0.00% | 0 | 1 | 0 | HIGV2 with batching disabled |
| higv1 legacy | 9600000 | 1316 | 99.99% | 344 | 26.61 | 0.00% | 0.00% | 0 | 1 | 0 | legacy one-file-per-block format |
| zip | 9600000 | 28151 | 99.71% | 31 | 295.33 | - | - | - | - | - | zip -qr |
| tar.zst | 9600000 | 1351 | 99.99% | 38 | 240.93 | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 9600000 | - | - | - | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:higv2:batch:first: files=1 input_bytes=9600000 archive_bytes=1245 duration_ms=407 throughput_mib_s=22.46 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv2:batch:second: files=1 input_bytes=9600000 archive_bytes=1245 duration_ms=393 throughput_mib_s=23.24 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv2:batch:trusted-metadata: files=1 input_bytes=9600000 archive_bytes=1245 duration_ms=243 throughput_mib_s=37.59 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv2:no-batch: files=1 input_bytes=9600000 archive_bytes=1245 duration_ms=356 throughput_mib_s=25.65 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv1:legacy: files=1 input_bytes=9600000 archive_bytes=1316 duration_ms=344 throughput_mib_s=26.54 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
benchmark: wrote artifacts/hig-v1.1.0-benchmark.md
```

## random

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 8388608 | 8389108 | -0.01% | 1240 | 6.45 | 0.00% | 0.00% | 0 | 1 | 0 | default HIGV2 batch format |
| higv2 batch second pack | 8388608 | 8389108 | -0.01% | 1132 | 7.07 | 100.00% | 0.00% | 0 | 1 | 0 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 8388608 | 8389108 | -0.01% | 1159 | 6.90 | 100.00% | 100.00% | 0 | 1 | 0 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 8388608 | 8389108 | -0.01% | 1425 | 5.61 | 0.00% | 0.00% | 0 | 1 | 0 | HIGV2 with batching disabled |
| higv1 legacy | 8388608 | 8389180 | -0.01% | 1461 | 5.48 | 0.00% | 0.00% | 0 | 1 | 0 | legacy one-file-per-block format |
| zip | 8388608 | 8390058 | -0.02% | 195 | 41.03 | - | - | - | - | - | zip -qr |
| tar.zst | 8388608 | 8389288 | -0.01% | 145 | 55.17 | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 8388608 | - | - | - | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:higv2:batch:first: files=1 input_bytes=8388608 archive_bytes=8389108 duration_ms=1240 throughput_mib_s=6.45 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv2:batch:second: files=1 input_bytes=8388608 archive_bytes=8389108 duration_ms=1132 throughput_mib_s=7.06 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv2:batch:trusted-metadata: files=1 input_bytes=8388608 archive_bytes=8389108 duration_ms=1159 throughput_mib_s=6.90 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv2:no-batch: files=1 input_bytes=8388608 archive_bytes=8389108 duration_ms=1425 throughput_mib_s=5.61 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv1:legacy: files=1 input_bytes=8388608 archive_bytes=8389180 duration_ms=1461 throughput_mib_s=5.48 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
benchmark: wrote artifacts/hig-v1.1.0-benchmark.md
```

## small

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 18000 | 21689 | -20.49% | 259 | 0.07 | 0.00% | 0.00% | 1 | 0 | 500 | default HIGV2 batch format |
| higv2 batch second pack | 18000 | 21689 | -20.49% | 277 | 0.06 | 0.00% | 0.00% | 1 | 0 | 500 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 18000 | 21689 | -20.49% | 278 | 0.06 | 0.00% | 100.00% | 1 | 0 | 500 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 18000 | 79848 | -343.60% | 316 | 0.05 | 0.00% | 0.00% | 0 | 500 | 0 | HIGV2 with batching disabled |
| higv1 legacy | 18000 | 142524 | -691.80% | 327 | 0.05 | 0.00% | 0.00% | 0 | 500 | 0 | legacy one-file-per-block format |
| zip | 18000 | 93806 | -421.14% | 14 | 1.23 | - | - | - | - | - | zip -qr |
| tar.zst | 18000 | 14594 | 18.92% | 156 | 0.11 | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 18000 | - | - | - | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:higv2:batch:first: files=500 input_bytes=18000 archive_bytes=21689 duration_ms=259 throughput_mib_s=0.07 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=0 batch_cache_misses=1
bench:higv2:batch:second: files=500 input_bytes=18000 archive_bytes=21689 duration_ms=277 throughput_mib_s=0.06 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=1 batch_cache_misses=0
bench:higv2:batch:trusted-metadata: files=500 input_bytes=18000 archive_bytes=21689 duration_ms=278 throughput_mib_s=0.06 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=0 metadata_hash_reuses=500 scan_cache_hits=500 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=1 batch_cache_misses=0
bench:higv2:no-batch: files=500 input_bytes=18000 archive_bytes=79848 duration_ms=316 throughput_mib_s=0.05 cache_hits=0 cache_misses=500 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=500 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
bench:higv1:legacy: files=500 input_bytes=18000 archive_bytes=142524 duration_ms=327 throughput_mib_s=0.05 cache_hits=0 cache_misses=500 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=500 batched_files=0 batch_cache_hits=0 batch_cache_misses=0
benchmark: wrote artifacts/hig-v1.1.0-benchmark.md
```

## modified-single-file

This scenario packs two files with HIGV2, modifies one file, then packs again with `--trust-metadata`. Expected behavior: one metadata hash reuse and a batch cache miss for the changed batch.

```text
pack: files=2 input_bytes=27 archive_bytes=384 duration_ms=237 throughput_mib_s=0.00 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=2 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=2 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=2 batch_cache_hits=0 batch_cache_misses=1
pack: files=2 input_bytes=26 archive_bytes=385 duration_ms=259 throughput_mib_s=0.00 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=1 scan_cache_hit_rate=50.00% batch_blocks=1 single_blocks=0 batched_files=2 batch_cache_hits=0 batch_cache_misses=1
```
