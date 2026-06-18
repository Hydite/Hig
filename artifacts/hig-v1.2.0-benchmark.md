# Hig v1.2.0 Benchmark

This report was generated from the local v1.2.0 workspace. Hig rows include HIGV2 batch/chunk default, no-batch, no-chunk, and HIGV1 legacy where applicable.

## srcdata


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.yTR1NX2Pif/srcdata`

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 133195 | 28250 | 78.79% | 261 | 0.49 | 0.00% | 0.00% | 1 | 0 | 15 | 0 | 0 | 0 | 0 | default HIGV2 batch format |
| higv2 batch second pack | 133195 | 28250 | 78.79% | 246 | 0.52 | 0.00% | 0.00% | 1 | 0 | 15 | 0 | 0 | 0 | 0 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 133195 | 28250 | 78.79% | 247 | 0.51 | 0.00% | 100.00% | 1 | 0 | 15 | 0 | 0 | 0 | 0 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 133195 | 31238 | 76.55% | 250 | 0.51 | 0.00% | 0.00% | 0 | 15 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk | 133195 | 28250 | 78.79% | 246 | 0.52 | 0.00% | 0.00% | 1 | 0 | 15 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv1 legacy | 133195 | 32967 | 75.25% | 247 | 0.51 | 0.00% | 0.00% | 0 | 15 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | 133195 | 29656 | 77.73% | 6 | 21.17 | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | 133195 | 29004 | 78.22% | 20 | 6.35 | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 133195 | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

### CLI output

```text
bench:higv2:batch:first: files=15 input_bytes=133195 archive_bytes=28250 duration_ms=261 throughput_mib_s=0.49 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=15 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=15 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=15 batch_cache_hits=0 batch_cache_misses=1 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:batch:second: files=15 input_bytes=133195 archive_bytes=28250 duration_ms=246 throughput_mib_s=0.51 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=15 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=15 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=15 batch_cache_hits=1 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:batch:trusted-metadata: files=15 input_bytes=133195 archive_bytes=28250 duration_ms=247 throughput_mib_s=0.51 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=0 metadata_hash_reuses=15 scan_cache_hits=15 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=1 single_blocks=0 batched_files=15 batch_cache_hits=1 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:no-batch: files=15 input_bytes=133195 archive_bytes=31238 duration_ms=250 throughput_mib_s=0.51 cache_hits=0 cache_misses=15 cache_hit_rate=0.00% hashed_files=15 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=15 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=15 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:no-chunk: files=15 input_bytes=133195 archive_bytes=28250 duration_ms=246 throughput_mib_s=0.52 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=15 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=15 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=15 batch_cache_hits=0 batch_cache_misses=1 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv1:legacy: files=15 input_bytes=133195 archive_bytes=32967 duration_ms=247 throughput_mib_s=0.51 cache_hits=0 cache_misses=15 cache_hit_rate=0.00% hashed_files=15 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=15 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=15 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
benchmark: wrote artifacts/hig-v1.2.0-benchmark.md
```

## repetitive


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.yTR1NX2Pif/repetitive`

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 4300000 | 752 | 99.98% | 291 | 14.09 | 0.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch format |
| higv2 batch second pack | 4300000 | 752 | 99.98% | 294 | 13.95 | 100.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 4300000 | 752 | 99.98% | 267 | 15.36 | 100.00% | 100.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 4300000 | 752 | 99.98% | 299 | 13.72 | 0.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk | 4300000 | 752 | 99.98% | 295 | 13.90 | 0.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv1 legacy | 4300000 | 820 | 99.98% | 307 | 13.36 | 0.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | 4300000 | 12726 | 99.70% | 15 | 273.39 | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | 4300000 | 859 | 99.98% | 45 | 91.13 | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 4300000 | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

### CLI output

```text
bench:higv2:batch:first: files=1 input_bytes=4300000 archive_bytes=752 duration_ms=291 throughput_mib_s=14.07 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:batch:second: files=1 input_bytes=4300000 archive_bytes=752 duration_ms=294 throughput_mib_s=13.91 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:batch:trusted-metadata: files=1 input_bytes=4300000 archive_bytes=752 duration_ms=267 throughput_mib_s=15.31 cache_hits=1 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:no-batch: files=1 input_bytes=4300000 archive_bytes=752 duration_ms=299 throughput_mib_s=13.68 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:no-chunk: files=1 input_bytes=4300000 archive_bytes=752 duration_ms=295 throughput_mib_s=13.87 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv1:legacy: files=1 input_bytes=4300000 archive_bytes=820 duration_ms=307 throughput_mib_s=13.35 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
benchmark: wrote artifacts/hig-v1.2.0-benchmark.md
```

## random


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.yTR1NX2Pif/random`

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 8388608 | 8389950 | -0.02% | 1433 | 5.58 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 8 | 0 | 8 | default HIGV2 batch format |
| higv2 batch second pack | 8388608 | 8389949 | -0.02% | 1293 | 6.19 | 100.00% | 0.00% | 0 | 0 | 0 | 1 | 8 | 8 | 0 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 8388608 | 8389950 | -0.02% | 1289 | 6.21 | 100.00% | 100.00% | 0 | 0 | 0 | 1 | 8 | 8 | 0 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 8388608 | 8389950 | -0.02% | 1433 | 5.58 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 8 | 0 | 8 | HIGV2 with batching disabled |
| higv2 --no-chunk | 8388608 | 8389110 | -0.01% | 1306 | 6.13 | 0.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv1 legacy | 8388608 | 8389180 | -0.01% | 1198 | 6.68 | 0.00% | 0.00% | 0 | 1 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | 8388608 | 8390058 | -0.02% | 139 | 57.55 | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | 8388608 | 8389279 | -0.01% | 72 | 111.11 | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 8388608 | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

### CLI output

```text
bench:higv2:batch:first: files=1 input_bytes=8388608 archive_bytes=8389950 duration_ms=1433 throughput_mib_s=5.58 cache_hits=0 cache_misses=8 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=8 chunk_cache_hits=0 chunk_cache_misses=8 chunk_bytes_reused=0 chunk_bytes_compressed=8388608
bench:higv2:batch:second: files=1 input_bytes=8388608 archive_bytes=8389949 duration_ms=1293 throughput_mib_s=6.18 cache_hits=8 cache_misses=0 cache_hit_rate=100.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=8 chunk_cache_hits=8 chunk_cache_misses=0 chunk_bytes_reused=8388608 chunk_bytes_compressed=0
bench:higv2:batch:trusted-metadata: files=1 input_bytes=8388608 archive_bytes=8389950 duration_ms=1289 throughput_mib_s=6.21 cache_hits=8 cache_misses=0 cache_hit_rate=100.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=8 chunk_cache_hits=8 chunk_cache_misses=0 chunk_bytes_reused=8388608 chunk_bytes_compressed=0
bench:higv2:no-batch: files=1 input_bytes=8388608 archive_bytes=8389950 duration_ms=1433 throughput_mib_s=5.58 cache_hits=0 cache_misses=8 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=8 chunk_cache_hits=0 chunk_cache_misses=8 chunk_bytes_reused=0 chunk_bytes_compressed=8388608
bench:higv2:no-chunk: files=1 input_bytes=8388608 archive_bytes=8389110 duration_ms=1306 throughput_mib_s=6.12 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv1:legacy: files=1 input_bytes=8388608 archive_bytes=8389180 duration_ms=1198 throughput_mib_s=6.67 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
benchmark: wrote artifacts/hig-v1.2.0-benchmark.md
```

## small


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.yTR1NX2Pif/small`

| tool | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | cache hit rate | scan cache hit rate | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 batch first pack | 15000 | 22586 | -50.57% | 276 | 0.05 | 0.00% | 0.00% | 1 | 0 | 500 | 0 | 0 | 0 | 0 | default HIGV2 batch format |
| higv2 batch second pack | 15000 | 22586 | -50.57% | 280 | 0.05 | 0.00% | 0.00% | 1 | 0 | 500 | 0 | 0 | 0 | 0 | reuses batch/single cache but recomputes file hashes |
| higv2 batch second pack --trust-metadata | 15000 | 22586 | -50.57% | 297 | 0.05 | 0.00% | 100.00% | 1 | 0 | 500 | 0 | 0 | 0 | 0 | reuses metadata cached hashes and batch/single cache |
| higv2 --no-batch | 15000 | 72413 | -382.75% | 399 | 0.04 | 0.00% | 0.00% | 0 | 500 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk | 15000 | 22586 | -50.57% | 274 | 0.05 | 0.00% | 0.00% | 1 | 0 | 500 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv1 legacy | 15000 | 138132 | -820.88% | 345 | 0.04 | 0.00% | 0.00% | 0 | 500 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | 15000 | 88022 | -486.81% | 14 | 1.02 | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | 15000 | 14702 | 1.99% | 171 | 0.08 | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | 15000 | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

### CLI output

```text
bench:higv2:batch:first: files=500 input_bytes=15000 archive_bytes=22586 duration_ms=276 throughput_mib_s=0.05 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=0 batch_cache_misses=1 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:batch:second: files=500 input_bytes=15000 archive_bytes=22586 duration_ms=280 throughput_mib_s=0.05 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=1 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:batch:trusted-metadata: files=500 input_bytes=15000 archive_bytes=22586 duration_ms=297 throughput_mib_s=0.05 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=0 metadata_hash_reuses=500 scan_cache_hits=500 scan_cache_misses=0 scan_cache_hit_rate=100.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=1 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:no-batch: files=500 input_bytes=15000 archive_bytes=72413 duration_ms=399 throughput_mib_s=0.04 cache_hits=0 cache_misses=500 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=500 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv2:no-chunk: files=500 input_bytes=15000 archive_bytes=22586 duration_ms=274 throughput_mib_s=0.05 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=1 single_blocks=0 batched_files=500 batch_cache_hits=0 batch_cache_misses=1 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
bench:higv1:legacy: files=500 input_bytes=15000 archive_bytes=138132 duration_ms=345 throughput_mib_s=0.04 cache_hits=0 cache_misses=500 cache_hit_rate=0.00% hashed_files=500 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=500 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=500 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
benchmark: wrote artifacts/hig-v1.2.0-benchmark.md
```

## large-file-local-modification

```text
chunk first pack:
pack: files=1 input_bytes=33554432 archive_bytes=33558999 duration_ms=5380 throughput_mib_s=5.95 cache_hits=0 cache_misses=32 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=32 chunk_cache_hits=0 chunk_cache_misses=32 chunk_bytes_reused=0 chunk_bytes_compressed=33554432
chunk second pack after 4KiB middle modification:
pack: files=1 input_bytes=33554432 archive_bytes=33555001 duration_ms=4767 throughput_mib_s=6.71 cache_hits=31 cache_misses=1 cache_hit_rate=96.88% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=32 chunk_cache_hits=31 chunk_cache_misses=1 chunk_bytes_reused=32505856 chunk_bytes_compressed=1048576
no-chunk first pack:
pack: files=1 input_bytes=33554432 archive_bytes=33551497 duration_ms=4262 throughput_mib_s=7.51 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
no-chunk second pack after 4KiB modification:
pack: files=1 input_bytes=33554432 archive_bytes=33547697 duration_ms=4286 throughput_mib_s=7.47 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% batch_blocks=0 single_blocks=1 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=0 chunk_blocks=0 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0
```

