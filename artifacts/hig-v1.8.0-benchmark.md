# Hig v1.8.0 Benchmark

Input: `/private/tmp/hig-v18-source-data`

## Environment Qualification

Status: `ENVIRONMENT_NOT_QUALIFIED`

## Release Gate Samples

Warm Balanced secure daemon and zip were each sampled 20 times.

| tool | median ms | p95 ms | p99 ms |
|---|---:|---:|---:|
| Hig Balanced secure daemon | 3.994 | 4.642 | 4.642 |
| zip -qr | 11.569 | 21.705 | 21.705 |

## Additional Quality Gates

| dataset | Hig result | reference | status |
|---|---:|---:|---|
| 500 text files, warm daemon median | 7.135 ms | zip 15.429 ms | PASS |
| 500 text files, archive size | 4,917 B | zip 90,306 B | PASS |
| 4 MiB repeated text | 710 B | gzip-6 12,287 B | PASS |
| 8 MiB random data | 8,389,887 B | input 8,388,608 B | PASS, 0.015% expansion |
| 32 MiB sealed hit | 46 ms, 32 hits | target <90 ms | PASS |
| 32 MiB after 4 KiB modification | 31 hits / 1 miss | target about 31 / 1 | PASS |

| path | free bytes | used % | 32MiB cp median MiB/s | 32MiB cp p95 MiB/s | 256MiB cp median MiB/s | 256MiB cp p95 MiB/s |
|---|---:|---:|---:|---:|---:|---:|
| `/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/` | 34357469184 | 85.00 | 423.49 | 602.63 | 508.69 | 606.56 |

| tool | encryption | speed | kdf profile | session used | session lookup ms | kdf skipped by session | workers | cache index | cache index open ms | cache index commit ms | daemon used | daemon lookup ms | scheduler queue ms | worker wait ms | buffer pool hits | buffer pool misses | cache pack range hits | cache pack opens | hot index reuses | hot metadata reuses | pipeline peak memory | writer | preallocated bytes | preallocation | cached opens | cached range opens | cached read bytes | prefetched bytes | direct writes | buffered writes | peak pipeline bytes | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload read ms | payload write ms | writer wait ms | flush ms | rename ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache pack hits | cache pack misses | cache pack fallbacks | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | solid groups | solid files | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| higv2 balanced first | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 7 | false | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 | 4390912 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 90117 | 84.93% | 31 | 18.39 | 14 | 0 | 17 | 14 | 0 | 2 | 0 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 88384 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 90113 | 84.93% | 18 | 31.68 | 0 | 0 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 88384 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 balanced secure session | Password | Balanced | Secure | true | 0 | true | 8 | hybrid | 0 | 0 | true | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 90117 | 84.93% | 1 | 570.17 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 88384 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | secure session pack; unlock cost 37 ms reported separately |
| higv2 balanced secure daemon | Password | Balanced | Secure | true | 0 | true | 8 | hybrid | 0 | 0 | true | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 90117 | 84.93% | 1 | 570.17 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 88384 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | secure hot daemon/session path; KDF skipped and cache index is warm |
| higv2 fastest secure | Password | Fastest | Secure | false | 0 | false | 8 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 1 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 1 | 104630 | 0 | 0 | 5 | 4194304 | 597867 | 106224 | 82.23% | 14 | 40.73 | 0 | 0 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 104630 | 0 | 5 | 0 | 5 | 0 | 0 | 0.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 45 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | false | 0 | false | 8 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 1 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 1 | 104630 | 0 | 0 | 5 | 4194304 | 597867 | 106224 | 82.23% | 2 | 285.09 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 104630 | 0 | 5 | 0 | 5 | 0 | 0 | 0.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 45 | 0 | 0 | 0 | 0 | 0 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | false | 0 | false | 8 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 4194304 | 597867 | 106224 | 82.23% | 2 | 285.09 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 5 | 0 | 104630 | 0 | 5 | 0 | 100.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 45 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 11 | false | 0 | 0 | 0 | 37 | 10 | 0 | 0 | 0 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 47 | 4194304 | 597867 | 108598 | 81.84% | 35 | 16.29 | 1 | 0 | 13 | 1 | 0 | 1 | 0 | 9 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 104839 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 47 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 7 | false | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 | 4390912 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 90117 | 84.93% | 25 | 22.81 | 0 | 0 | 13 | 0 | 0 | 2 | 0 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 88384 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 90114 | 84.93% | 15 | 38.01 | 0 | 0 | 14 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 88384 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 7 | false | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 | 4390912 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 597867 | 89981 | 84.95% | 10 | 57.02 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 88272 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 29 | 2 | 16 | 0 | 0 | 0 | 0 | 0 | 0 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | false | 0 | false | 8 | json | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 47 | 4194304 | 597867 | 116399 | 80.53% | 35 | 16.29 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 47 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 597867 | 114403 | 80.86% | 15 | 38.01 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.gz | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 597867 | 101328 | 83.05% | 55 | 10.37 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + gzip -6 |
| tar.zst | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 597867 | 106613 | 82.17% | 49 | 11.64 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 597867 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |
