# Hig v1.3.0 Benchmark

Input: `/private/var/folders/7_/472hlb690038yh2tgb4ypfzc0000gn/T/tmp.UNxF2oMaal/in`

| tool | speed | kdf profile | input bytes | archive bytes | reduction | duration ms | throughput MiB/s | scan ms | plan ms | kdf ms | pack blocks ms | sealed hits | sealed misses | sealed bytes reused | reencrypted cache hits | payload cache files | payload memory bytes | cache hit rate | scan cache hit rate | chunk metadata reuses | trusted bytes skipped | batch blocks | single blocks | batched files | chunked files | chunk blocks | chunk hits | chunk misses | chunk plan hits | chunk plan misses | notes |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| higv2 balanced first | Balanced | Secure | 6 | 307 | -5016.67% | 257 | 0.00 | 0 | 0 | 256 | 0 | 0 | 0 | 0 | 0 | 0 | 31 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | default HIGV2 batch/chunk format |
| higv2 balanced second | Balanced | Secure | 6 | 307 | -5016.67% | 246 | 0.00 | 0 | 0 | 246 | 0 | 0 | 0 | 0 | 0 | 0 | 31 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | reuses batch/single/chunk cache but recomputes file hashes |
| higv2 fastest second | Fastest | Secure | 6 | 307 | -5016.67% | 248 | 0.00 | 0 | 0 | 247 | 0 | 0 | 1 | 0 | 1 | 0 | 31 | 0.00% | 100.00% | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | reuses trusted metadata hashes, chunk plans, and block cache |
| higv2 fastest second --kdf-profile fast-bench | Fastest | FastBench | 6 | 307 | -5016.67% | 7 | 0.00 | 0 | 0 | 6 | 0 | 0 | 1 | 0 | 1 | 0 | 31 | 0.00% | 100.00% | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | fastest mode with benchmark-only KDF profile |
| higv2 --no-batch | Balanced | Secure | 6 | 307 | -5016.67% | 248 | 0.00 | 0 | 0 | 247 | 0 | 0 | 0 | 0 | 0 | 0 | 31 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with batching disabled |
| higv2 --no-chunk first | Balanced | Secure | 6 | 307 | -5016.67% | 244 | 0.00 | 0 | 0 | 243 | 0 | 0 | 0 | 0 | 0 | 0 | 31 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 with large-file chunking disabled |
| higv2 --no-chunk second | Balanced | Secure | 6 | 307 | -5016.67% | 250 | 0.00 | 0 | 0 | 249 | 0 | 0 | 0 | 0 | 0 | 0 | 31 | 0.00% | 0.00% | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | HIGV2 second pack with large-file chunking disabled |
| higv1 legacy | Balanced | Secure | 6 | 380 | -6233.33% | 248 | 0.00 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0.00% | 0.00% | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | legacy one-file-per-block format |
| zip | - | - | 6 | 166 | -2666.67% | 3 | 0.00 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | zip -qr |
| tar.zst | - | - | 6 | 433 | -7116.67% | 10 | 0.00 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | tar -cf + zstd -1 |
| 7z | - | - | 6 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | skipped (not installed) |
