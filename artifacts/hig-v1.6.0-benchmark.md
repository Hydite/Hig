# Hig v1.6.0 Benchmark

## Release Gate Measurements

Release build, 8 workers, warm cache, 20 independent process executions on `/private/tmp`:

| tool | median ms | p95 ms | archive bytes | versus zip time | versus zip size |
|---|---:|---:|---:|---:|---:|
| Hig balanced secure session | 6.15 | 7.49 | 58,112 | 32.3% faster | 10.7% smaller |
| zip -qr | 9.08 | 9.96 | 65,058 | baseline | baseline |
| tar + gzip -6 | 19.59 | 21.68 | 61,421 | 53.7% slower than zip | 5.6% smaller than zip |

The session unlock cost was measured separately and is not included in the session pack distribution. The secure Argon2id parameters are unchanged.

## Compression Quality Gates

| dataset | input bytes | Hig bytes | zip bytes | tar.gz bytes | status |
|---|---:|---:|---:|---:|---|
| 500 small text files | 27,500 | 4,992 | 102,306 | 27,119 | PASS |
| 4MiB repeated text | 4,194,304 | 416 | 4,255 | 4,572 | PASS |
| 8MiB random data | 8,388,608 | 8,389,889 | 8,390,058 | 8,391,763 | PASS (0.015% expansion) |

`CORE_V1_6_GATES_PASSED`: source-directory session speed, p95, archive size, small-file quality, repeated-text quality, and random-data expansion all pass.

`ABSOLUTE_IO_GATE_NOT_QUALIFIED`: the system volume produced only 237.10 MiB/s median for the 256MiB native-copy probe, below the 650 MiB/s qualification threshold. Absolute sealed-I/O throughput is not claimed as qualified on this machine.

Input: `/private/tmp/hig-v16-source`

## Environment Qualification

Status: `ENVIRONMENT_NOT_QUALIFIED`

| path | free bytes | used % | 32MiB cp median MiB/s | 32MiB cp p95 MiB/s | 256MiB cp median MiB/s | 256MiB cp p95 MiB/s |
|---|---:|---:|---:|---:|---:|---:|
| `/tmp` | 40356937728 | 82.00 | 2262.36 | 2387.53 | 237.10 | 337.31 |

| tool | encryption | speed | kdf profile | session used | session lookup ms | kdf skipped by session | workers | cache index | cache index open ms | cache index commit ms | writer | preallocated bytes | preallocation | cached opens | cached range opens | cached read bytes | prefetched bytes | direct writes | buffered writes | peak pipeline bytes | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | kdf overlap ms | read ms | compression ms | crypto ms | pack blocks ms | payload read ms | payload write ms | writer wait ms | flush ms | rename ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache pack hits | cache pack misses | cache pack fallbacks | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | solid groups | solid files | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| higv2 balanced first | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 2 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 349525 | 58118 | 83.37% | 19 | 17.54 | 1 | 0 | 14 | 1 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 57057 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 12 | 2 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 349525 | 58118 | 83.37% | 17 | 19.61 | 0 | 0 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 57057 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 12 | 2 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 balanced secure session | Password | Balanced | Secure | true | 1 | true | 8 | index-v2 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 349525 | 58118 | 83.37% | 2 | 166.67 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 57057 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 12 | 2 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | secure session pack; unlock cost 63 ms reported separately |
| higv2 fastest secure | Password | Fastest | Secure | false | 0 | false | 8 | index-v2 | 0 | 0 | Buffered | 0 | false | 0 | 1 | 66235 | 0 | 0 | 5 | 4194304 | 349525 | 67174 | 80.78% | 13 | 25.64 | 0 | 0 | 12 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 66235 | 0 | 5 | 0 | 5 | 0 | 0 | 0.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with secure KDF and sealed block reuse |
| higv2 fastest interactive | Password | Fastest | Interactive | false | 0 | false | 8 | index-v2 | 0 | 0 | Buffered | 0 | false | 0 | 1 | 66235 | 0 | 0 | 5 | 4194304 | 349525 | 67174 | 80.78% | 2 | 166.67 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 66235 | 0 | 5 | 0 | 5 | 0 | 0 | 0.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | explicit fastest mode; metadata trust and sealed encrypted cache enabled |
| higv2 fastest second --kdf-profile fast-bench | Password | Fastest | FastBench | false | 0 | false | 8 | index-v2 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 5 | 4194304 | 349525 | 67174 | 80.78% | 7 | 47.62 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | 5 | 0 | 66235 | 0 | 5 | 0 | 100.00% | 100.00% | 0 | 0 | 3 | 0 | 0 | 2 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 5 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 17 | 4194304 | 349525 | 61744 | 82.33% | 23 | 14.49 | 0 | 0 | 13 | 0 | 0 | 0 | 0 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 60190 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 17 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 4 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 349525 | 58112 | 83.37% | 21 | 15.87 | 0 | 0 | 13 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 57057 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 12 | 2 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Password | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 349525 | 58118 | 83.37% | 13 | 25.64 | 0 | 0 | 12 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 57057 | 0 | 0 | 0 | 100.00% | 0.00% | 0 | 0 | 2 | 3 | 12 | 2 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv2 no-encryption | None | Balanced | Secure | false | 0 | false | 8 | index-v2 | 0 | 4 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 7 | 4194304 | 349525 | 57983 | 83.41% | 7 | 47.62 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 56945 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 2 | 3 | 12 | 2 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| higv1 legacy | Password | Balanced | Secure | false | 0 | false | 8 | json | 0 | 0 | Buffered | 0 | false | 0 | 0 | 0 | 0 | 0 | 17 | 4194304 | 349525 | 64336 | 81.59% | 23 | 14.49 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 0 | 0 | 17 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 349525 | 65058 | 81.39% | 8 | 41.67 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.gz | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 349525 | 61427 | 82.43% | 68 | 4.90 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + gzip -6 |
| tar.zst | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 349525 | 66448 | 80.99% | 39 | 8.55 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | 349525 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |
