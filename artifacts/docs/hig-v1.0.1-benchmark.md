# Hig v1.0.1 Benchmark

Generated with `target/debug/hig bench --compare` after v1.0.1 implementation. Hig cache directories are kept outside the input during compare runs; `.hig-cache` is excluded from Hig, zip, and tar.zst inputs.

## srcdata

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| hig first pack | 96406 | 43944 | 54.42% | 315 | 0.29 | 0.00% | 0.00% | initial pack computes BLAKE3 and compresses files |
| hig second pack | 96406 | 43944 | 54.42% | 256 | 0.36 | 100.00% | 0.00% | reuses compressed blocks but recomputes file hashes |
| hig second pack --trust-metadata | 96406 | 43944 | 54.42% | 257 | 0.36 | 100.00% | 100.00% | reuses metadata cached hashes and compressed blocks |
| zip | 96406 | 42020 | 56.41% | 20 | 4.60 | - | - | zip -qr |
| tar.zst | 96406 | 39990 | 58.52% | 21 | 4.38 | - | - | tar -cf + zstd -1 |
| 7z | 96406 | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:hig:first: files=18 input_bytes=96406 archive_bytes=43944 duration_ms=315 throughput_mib_s=0.29 cache_hits=0 cache_misses=18 cache_hit_rate=0.00% hashed_files=18 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=18 scan_cache_hit_rate=0.00%
bench:hig:second: files=18 input_bytes=96406 archive_bytes=43944 duration_ms=256 throughput_mib_s=0.36 cache_hits=18 cache_misses=0 cache_hit_rate=100.00% hashed_files=18 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=18 scan_cache_hit_rate=0.00%
bench:hig:trusted-metadata: files=18 input_bytes=96406 archive_bytes=43944 duration_ms=257 throughput_mib_s=0.36 cache_hits=18 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=18 scan_cache_hits=18 scan_cache_misses=0 scan_cache_hit_rate=100.00%
benchmark: wrote artifacts/hig-v1.0.1-benchmark.md
```

## repetitive

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| hig first pack | 9600000 | 1316 | 99.99% | 368 | 24.88 | 0.00% | 0.00% | initial pack computes BLAKE3 and compresses files |
| hig second pack | 9600000 | 1316 | 99.99% | 393 | 23.30 | 100.00% | 0.00% | reuses compressed blocks but recomputes file hashes |
| hig second pack --trust-metadata | 9600000 | 1316 | 99.99% | 273 | 33.54 | 100.00% | 100.00% | reuses metadata cached hashes and compressed blocks |
| zip | 9600000 | 28151 | 99.71% | 31 | 295.33 | - | - | zip -qr |
| tar.zst | 9600000 | 1348 | 99.99% | 32 | 286.10 | - | - | tar -cf + zstd -1 |
| 7z | 9600000 | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:hig:first: files=1 input_bytes=9600000 archive_bytes=1316 duration_ms=368 throughput_mib_s=24.85 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00%
bench:hig:second: files=1 input_bytes=9600000 archive_bytes=1316 duration_ms=393 throughput_mib_s=23.26 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00%
bench:hig:trusted-metadata: files=1 input_bytes=9600000 archive_bytes=1316 duration_ms=273 throughput_mib_s=33.45 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00%
benchmark: wrote artifacts/hig-v1.0.1-benchmark.md
```

## random

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| hig first pack | 8388608 | 8389180 | -0.01% | 1235 | 6.48 | 0.00% | 0.00% | initial pack computes BLAKE3 and compresses files |
| hig second pack | 8388608 | 8389180 | -0.01% | 1191 | 6.72 | 100.00% | 0.00% | reuses compressed blocks but recomputes file hashes |
| hig second pack --trust-metadata | 8388608 | 8389180 | -0.01% | 1224 | 6.54 | 100.00% | 100.00% | reuses metadata cached hashes and compressed blocks |
| zip | 8388608 | 8390058 | -0.02% | 202 | 39.60 | - | - | zip -qr |
| tar.zst | 8388608 | 8389278 | -0.01% | 97 | 82.47 | - | - | tar -cf + zstd -1 |
| 7z | 8388608 | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:hig:first: files=1 input_bytes=8388608 archive_bytes=8389180 duration_ms=1235 throughput_mib_s=6.47 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00%
bench:hig:second: files=1 input_bytes=8388608 archive_bytes=8389180 duration_ms=1191 throughput_mib_s=6.71 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00%
bench:hig:trusted-metadata: files=1 input_bytes=8388608 archive_bytes=8389180 duration_ms=1224 throughput_mib_s=6.54 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00%
benchmark: wrote artifacts/hig-v1.0.1-benchmark.md
```

## small

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| hig first pack | 18000 | 142524 | -691.80% | 6772 | 0.00 | 0.00% | 0.00% | initial pack computes BLAKE3 and compresses files |
| hig second pack | 18000 | 142524 | -691.80% | 6663 | 0.00 | 100.00% | 0.00% | reuses compressed blocks but recomputes file hashes |
| hig second pack --trust-metadata | 18000 | 142524 | -691.80% | 6438 | 0.00 | 100.00% | 100.00% | reuses metadata cached hashes and compressed blocks |
| zip | 18000 | 93806 | -421.14% | 15 | 1.14 | - | - | zip -qr |
| tar.zst | 18000 | 14893 | 17.26% | 171 | 0.10 | - | - | tar -cf + zstd -1 |
| 7z | 18000 | - | - | - | - | - | - | skipped (not installed) |

CLI output:

```text
bench:hig:first: files=500 input_bytes=18000 archive_bytes=142524 duration_ms=6772 throughput_mib_s=0.00 cache_hits=0 cache_misses=500 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00%
bench:hig:second: files=500 input_bytes=18000 archive_bytes=142524 duration_ms=6663 throughput_mib_s=0.00 cache_hits=500 cache_misses=0 cache_hit_rate=100.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00%
bench:hig:trusted-metadata: files=500 input_bytes=18000 archive_bytes=142524 duration_ms=6438 throughput_mib_s=0.00 cache_hits=500 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=500 scan_cache_hits=500 scan_cache_misses=0 scan_cache_hit_rate=100.00%
benchmark: wrote artifacts/hig-v1.0.1-benchmark.md
```

## modified-single-file

This scenario packs two files, modifies one file, then packs again with `--trust-metadata`. Expected behavior: one metadata hash reuse and one rehashed file.

```text
pack: files=2 input_bytes=27 archive_bytes=643 duration_ms=247 throughput_mib_s=0.00 cache_hits=0 cache_misses=2 cache_hit_rate=0.00% hashed_files=2 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=2 scan_cache_hit_rate=0.00%
pack: files=2 input_bytes=26 archive_bytes=642 duration_ms=246 throughput_mib_s=0.00 cache_hits=1 cache_misses=1 cache_hit_rate=50.00% hashed_files=1 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=1 scan_cache_hit_rate=50.00%
```
