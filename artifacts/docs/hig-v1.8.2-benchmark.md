# Hig v1.8.2 Benchmark

Input: `/private/tmp/hig-v182-source-data`

## Gate Summary

| gate | status |
|---|---|
| environment_status | `QUALIFIED` |
| pack_core_gate | true |
| cli_wall_gate | true |
| size_quality_gate | true |

## Environment Qualification

Status: `QUALIFIED`

## Release Gate Samples

Warm Balanced secure daemon pack-core, daemon CLI wall, and zip were each sampled 20 times.

| tool | median ms | p95 ms | p99 ms | min ms | max ms |
|---|---:|---:|---:|---:|---:|
| Hig Balanced secure daemon pack-core | 1.037 | 1.245 | 1.245 | 0.889 | 1.245 |
| Hig Balanced secure daemon cli-wall | 2.907 | 3.064 | 3.064 | 2.643 | 3.064 |
| zip -qr | 11.262 | 23.817 | 23.817 | 10.408 | 23.817 |

| path | free bytes | used % | 32MiB cp median MiB/s | 32MiB cp p95 MiB/s | 256MiB cp median MiB/s | 256MiB cp p95 MiB/s |
|---|---:|---:|---:|---:|---:|---:|
| `/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/` | 33542332416 | 85.00 | 2960.71 | 3255.45 | 789.89 | 1673.00 |

| tool | encryption | speed | kdf profile | session used | session lookup ms | kdf skipped by session | workers | cache index | cache index open ms | cache index commit ms | daemon used | daemon lookup ms | scheduler queue ms | worker wait ms | buffer pool hits | buffer pool misses | cache pack range hits | cache pack opens | hot index reuses | hot metadata reuses | pipeline peak memory | writer | preallocated bytes | preallocation | cached opens | cached range opens | cached read bytes | prefetched bytes | direct writes | buffered writes | peak pipeline bytes | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload read ms | payload write ms | writer wait ms | flush ms | rename ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache pack hits | cache pack misses | cache pack fallbacks | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | solid groups | solid files | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| higv2 balanced first | Password | Balanced | Secure | false | 0 | false | 10 | index-v2 | 0 | 1 | false | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 | 4390912 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 99065 | 84.96% | 39 | 16.11 | 19 | 0 | 13 | 13 | 0 | 0 | 0 | 13 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97458 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 99066 | 84.96% | 15 | 41.87 | 0 | 0 | 12 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97458 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 balanced secure session | Password | Balanced | Secure | true | 0 | true | 10 | index-v2 | 0 | 0 | true | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 99067 | 84.96% | 4 | 157.03 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97458 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | secure session pack; unlock cost 12 ms reported separately |
| higv2 balanced secure daemon | Password | Balanced | Secure | true | 0 | true | 10 | index-v2 | 0 | 0 | true | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 99066 | 84.96% | 1 | 628.10 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97458 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | secure hot daemon/session path; KDF skipped and cache index is warm |
| higv2 fastest secure | Password | Fastest | Secure | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 1 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 1 | 115269 | 0 | 0 | 5 | 4194304 | 658611 | 116757 | 82.27% | 12 | 52.34 | 0 | 0 | 11 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 115269 | 0 | 5 | 0 | 5 | 0 | 0 | 0.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 37 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 1 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 1 | 115269 | 0 | 0 | 5 | 4194304 | 658611 | 116757 | 82.27% | 2 | 314.05 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 115269 | 0 | 5 | 0 | 5 | 0 | 0 | 0.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 37 | 0 | 0 | 0 | 0 | 0 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 4194304 | 658611 | 116757 | 82.27% | 2 | 314.05 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 5 | 0 | 115269 | 0 | 5 | 0 | 100.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 37 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 28 | 11 | 0 | 0 | 0 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 39 | 4194304 | 658611 | 120368 | 81.72% | 19 | 33.06 | 1 | 0 | 11 | 1 | 0 | 0 | 0 | 6 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 117151 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 | 4390912 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 99066 | 84.96% | 14 | 44.86 | 0 | 0 | 11 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97458 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 4194304 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 99065 | 84.96% | 12 | 52.34 | 0 | 0 | 11 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97458 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | false | 0 | false | 10 | index-v2 | 0 | 0 | false | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 | 4390912 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 658611 | 98936 | 84.98% | 3 | 209.37 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 97346 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 33 | 2 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | false | 0 | false | 10 | json | 0 | 0 | false | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 39 | 4194304 | 658611 | 126655 | 80.77% | 21 | 29.91 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 658611 | 124967 | 81.03% | 10 | 62.81 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.gz | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 658611 | 109587 | 83.36% | 32 | 19.63 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + gzip -6 |
| tar.zst | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 658611 | 116879 | 82.25% | 31 | 20.26 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 658611 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |
