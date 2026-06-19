# Hig v1.4.0 Benchmark

Release build, 8 workers. Password rows use Argon2id plus ChaCha20-Poly1305. `none` rows provide no confidentiality or AEAD and retain BLAKE3 corruption checks only. Timings are local measurements and include filesystem cache effects.

## Summary

| scenario | Hig mode | Hig ms | zip ms | result |
|---|---|---:|---:|---|
| source tree | fastest interactive | 2 | 9 | 4.5x faster |
| source tree | no encryption | 1 | 9 | 9x faster |
| 4MiB repetitive | fastest interactive | 3 | 18 | 6x faster |
| 8MiB random | no encryption | 68 | 167 | 2.46x faster |
| 500 small files | fastest interactive | 4 | 65 | 16.25x faster |

The secure balanced path keeps the existing Argon2id parameters. The 32MiB unchanged fastest run completed in 139ms with 32 sealed hits; 134ms was payload writing. After a 4KiB modification it reused 31 chunks and recompressed/re-encrypted one 1MiB chunk.

## source


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.7fRDYYgUyW/source`

| tool | encryption | speed | kdf profile | workers | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload write ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 balanced first | Password | Balanced | Secure | 8 | 211617 | 42019 | 80.14% | 25 | 8.07 | 1 | 0 | 22 | 1 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 40749 | 0.00% | 0.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | 8 | 211617 | 42019 | 80.14% | 16 | 12.61 | 0 | 0 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 40749 | 100.00% | 0.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 fastest secure | Password | Fastest | Secure | 8 | 211617 | 42019 | 80.14% | 14 | 14.42 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 0 | 40749 | 0 | 3 | 0 | 0.00% | 100.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | 8 | 211617 | 42019 | 80.14% | 2 | 100.91 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 0 | 40749 | 0 | 3 | 0 | 0.00% | 100.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | 8 | 211617 | 42019 | 80.14% | 1 | 201.81 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 0 | 3 | 0 | 40749 | 100.00% | 100.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | 8 | 211617 | 43755 | 79.32% | 17 | 11.87 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 41932 | 0.00% | 0.00% | 0 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | 8 | 211617 | 42019 | 80.14% | 16 | 12.61 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 40749 | 0.00% | 0.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | 8 | 211617 | 42019 | 80.14% | 15 | 13.45 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 40749 | 100.00% | 0.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | 8 | 211617 | 41948 | 80.18% | 1 | 201.81 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 40701 | 0.00% | 0.00% | 0 | 0 | 2 | 1 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | 8 | 211617 | 45373 | 78.56% | 19 | 10.62 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | 211617 | 39984 | 81.11% | 9 | 22.42 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | - | - | - | - | 211617 | 40733 | 80.75% | 39 | 5.17 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | 211617 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

## repetitive


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.7fRDYYgUyW/repetitive`

| tool | encryption | speed | kdf profile | workers | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload write ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 balanced first | Password | Balanced | Secure | 8 | 4194304 | 450 | 99.99% | 19 | 210.53 | 2 | 0 | 17 | 2 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 163 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | 8 | 4194304 | 450 | 99.99% | 15 | 266.67 | 2 | 0 | 15 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 163 | 100.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 fastest secure | Password | Fastest | Secure | 8 | 4194304 | 450 | 99.99% | 15 | 266.67 | 0 | 0 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 163 | 0 | 1 | 0 | 0.00% | 100.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | 8 | 4194304 | 450 | 99.99% | 3 | 1333.33 | 0 | 0 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 163 | 0 | 1 | 0 | 0.00% | 100.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | 8 | 4194304 | 450 | 99.99% | 1 | 4000.00 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 1 | 0 | 163 | 100.00% | 100.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | 8 | 4194304 | 450 | 99.99% | 22 | 181.82 | 2 | 0 | 20 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 163 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | 8 | 4194304 | 450 | 99.99% | 27 | 148.15 | 2 | 0 | 25 | 2 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 163 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | 8 | 4194304 | 450 | 99.99% | 16 | 250.00 | 2 | 0 | 15 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 163 | 100.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | 8 | 4194304 | 415 | 99.99% | 3 | 1333.33 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 147 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | 8 | 4194304 | 519 | 99.99% | 20 | 200.00 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | 4194304 | 4254 | 99.90% | 18 | 222.22 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | - | - | - | - | 4194304 | 574 | 99.99% | 26 | 153.85 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | 4194304 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

## random-8MiB


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.7fRDYYgUyW/random`

| tool | encryption | speed | kdf profile | workers | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload write ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 balanced first | Password | Balanced | Secure | 8 | 8388608 | 8389950 | -0.02% | 78 | 102.56 | 5 | 4 | 16 | 9 | 0 | 0 | 3 | 33 | 25 | 0 | 0 | 0 | 0 | 0 | 8389008 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 1 | 8 | 0 | 8 | 0 | 8 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | 8 | 8388608 | 8389950 | -0.02% | 53 | 150.94 | 5 | 5 | 15 | 10 | 0 | 0 | 3 | 8 | 29 | 0 | 0 | 0 | 0 | 0 | 8389008 | 100.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 1 | 8 | 8 | 0 | 0 | 8 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 fastest secure | Password | Fastest | Secure | 8 | 8388608 | 8389950 | -0.02% | 49 | 163.27 | 0 | 0 | 13 | 0 | 0 | 0 | 0 | 0 | 35 | 8 | 0 | 8389008 | 0 | 8 | 0 | 0.00% | 100.00% | 1 | 8388608 | 0 | 0 | 0 | 1 | 8 | 0 | 0 | 8 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | 8 | 8388608 | 8389950 | -0.02% | 35 | 228.57 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 32 | 8 | 0 | 8389008 | 0 | 8 | 0 | 0.00% | 100.00% | 1 | 8388608 | 0 | 0 | 0 | 1 | 8 | 0 | 0 | 8 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | 8 | 8388608 | 8389950 | -0.02% | 85 | 94.12 | 0 | 0 | 0 | 0 | 0 | 0 | 8 | 50 | 34 | 0 | 8 | 0 | 8 | 0 | 8389008 | 100.00% | 100.00% | 1 | 8388608 | 0 | 0 | 0 | 1 | 8 | 8 | 0 | 8 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | 8 | 8388608 | 8389950 | -0.02% | 105 | 76.19 | 6 | 9 | 27 | 15 | 0 | 0 | 3 | 46 | 31 | 0 | 0 | 0 | 0 | 0 | 8389008 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 1 | 8 | 0 | 8 | 0 | 8 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | 8 | 8388608 | 8389110 | -0.01% | 99 | 80.81 | 4 | 0 | 16 | 4 | 0 | 1 | 13 | 50 | 31 | 0 | 0 | 0 | 0 | 0 | 8388826 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | 8 | 8388608 | 8389110 | -0.01% | 53 | 150.94 | 4 | 0 | 13 | 4 | 0 | 0 | 13 | 18 | 21 | 0 | 0 | 0 | 0 | 0 | 8388826 | 100.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | 8 | 8388608 | 8389808 | -0.01% | 68 | 117.65 | 4 | 4 | 0 | 0 | 0 | 0 | 0 | 34 | 25 | 0 | 0 | 0 | 0 | 0 | 8388880 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 1 | 8 | 0 | 8 | 0 | 8 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | 8 | 8388608 | 8389180 | -0.01% | 102 | 78.43 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | 8388608 | 8390058 | -0.02% | 167 | 47.90 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | - | - | - | - | 8388608 | 8389278 | -0.01% | 87 | 91.95 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | 8388608 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

## 500-small-files


Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.7fRDYYgUyW/small`

| tool | encryption | speed | kdf profile | workers | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload write ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 balanced first | Password | Balanced | Secure | 8 | 16000 | 23391 | -46.19% | 32 | 0.48 | 12 | 0 | 21 | 12 | 9 | 0 | 0 | 10 | 0 | 0 | 0 | 0 | 0 | 0 | 946 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | 8 | 16000 | 23391 | -46.19% | 17 | 0.90 | 4 | 0 | 15 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 946 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 fastest secure | Password | Fastest | Secure | 8 | 16000 | 23391 | -46.19% | 15 | 1.02 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 946 | 0 | 1 | 0 | 0.00% | 100.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | 8 | 16000 | 23391 | -46.19% | 4 | 3.81 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 946 | 0 | 1 | 0 | 0.00% | 100.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | 8 | 16000 | 23391 | -46.19% | 2 | 7.63 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 1 | 0 | 946 | 0.00% | 100.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | 8 | 16000 | 68847 | -330.29% | 93 | 0.16 | 4 | 0 | 14 | 4 | 0 | 0 | 0 | 77 | 0 | 0 | 0 | 0 | 0 | 0 | 24000 | 0.00% | 0.00% | 0 | 0 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | 8 | 16000 | 23391 | -46.19% | 20 | 0.76 | 4 | 0 | 13 | 4 | 5 | 0 | 0 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 946 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | 8 | 16000 | 23391 | -46.19% | 16 | 0.95 | 4 | 0 | 14 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 946 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | 8 | 16000 | 23358 | -45.99% | 10 | 1.53 | 4 | 0 | 0 | 0 | 5 | 0 | 0 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 930 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | 8 | 16000 | 136024 | -750.15% | 95 | 0.16 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 500 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | 16000 | 86806 | -442.54% | 65 | 0.23 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | - | - | - | - | 16000 | 15774 | 1.41% | 202 | 0.08 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | 16000 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |

## 32MiB Large File Incremental Path

The third run changes one 4KiB region.

```text
first:
pack: files=1 input_bytes=33554432 archive_bytes=33554927 duration_ms=488 throughput_mib_s=65.46 encryption=Password speed=Fastest kdf_profile=Interactive workers=8 scan_ms=29 plan_ms=19 kdf_ms=7 kdf_overlapped_ms=7 read_ms=0 compression_ms=0 crypto_ms=33 pack_blocks_ms=317 manifest_ms=0 write_ms=120 payload_write_ms=120 cache_hits=0 cache_misses=32 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% chunk_metadata_reuses=0 chunk_metadata_misses=1 trusted_bytes_skipped=0 batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=32 chunk_cache_hits=0 chunk_cache_misses=32 chunk_bytes_reused=0 chunk_bytes_compressed=33554432 chunk_plan_cache_hits=0 chunk_plan_cache_misses=32 sealed_block_hits=0 sealed_block_misses=32 sealed_bytes_reused=0 reencrypted_cache_hits=0 payload_source_cache_files=0 payload_source_memory_bytes=33551947
second unchanged:
pack: files=1 input_bytes=33554432 archive_bytes=33554927 duration_ms=139 throughput_mib_s=229.97 encryption=Password speed=Fastest kdf_profile=Interactive workers=8 scan_ms=0 plan_ms=0 kdf_ms=2 kdf_overlapped_ms=0 read_ms=0 compression_ms=0 crypto_ms=0 pack_blocks_ms=0 manifest_ms=0 write_ms=134 payload_write_ms=134 cache_hits=0 cache_misses=0 cache_hit_rate=0.00% hashed_files=0 metadata_hash_reuses=1 scan_cache_hits=1 scan_cache_misses=0 scan_cache_hit_rate=100.00% chunk_metadata_reuses=1 chunk_metadata_misses=0 trusted_bytes_skipped=33554432 batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=32 chunk_cache_hits=0 chunk_cache_misses=0 chunk_bytes_reused=0 chunk_bytes_compressed=0 chunk_plan_cache_hits=32 chunk_plan_cache_misses=0 sealed_block_hits=32 sealed_block_misses=0 sealed_bytes_reused=33551947 reencrypted_cache_hits=0 payload_source_cache_files=32 payload_source_memory_bytes=0
after 4KiB modification:
pack: files=1 input_bytes=33554432 archive_bytes=33558999 duration_ms=210 throughput_mib_s=151.75 encryption=Password speed=Fastest kdf_profile=Interactive workers=8 scan_ms=18 plan_ms=17 kdf_ms=3 kdf_overlapped_ms=3 read_ms=0 compression_ms=0 crypto_ms=1 pack_blocks_ms=10 manifest_ms=0 write_ms=162 payload_write_ms=162 cache_hits=0 cache_misses=1 cache_hit_rate=0.00% hashed_files=1 metadata_hash_reuses=0 scan_cache_hits=0 scan_cache_misses=1 scan_cache_hit_rate=0.00% chunk_metadata_reuses=0 chunk_metadata_misses=1 trusted_bytes_skipped=0 batch_blocks=0 single_blocks=0 batched_files=0 batch_cache_hits=0 batch_cache_misses=0 chunked_files=1 chunk_blocks=32 chunk_cache_hits=0 chunk_cache_misses=1 chunk_bytes_reused=0 chunk_bytes_compressed=1048576 chunk_plan_cache_hits=0 chunk_plan_cache_misses=32 sealed_block_hits=31 sealed_block_misses=1 sealed_bytes_reused=32507406 reencrypted_cache_hits=0 payload_source_cache_files=31 payload_source_memory_bytes=1048626
```
