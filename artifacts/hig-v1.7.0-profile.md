# Hig v1.7.0 Profile

## Scope

- Build: Rust release, 8 workers
- Dataset: 380,206-byte Hig source sample, 18 files
- Volume: `/private/tmp`, 35+ GiB free
- Security: password encryption, secure Argon2id, explicit in-memory session

## Results

| path | median | p95 | archive bytes |
|---|---:|---:|---:|
| Hig Balanced secure daemon/session, warm cache | 3.122 ms | 3.891 ms | 62,666 |
| zip -qr | 9.056 ms | 45.362 ms | 70,407 |
| tar + gzip -6 | 48 ms single run | - | 65,789 |

Hig was 2.9x faster than zip by median and produced an archive 11.0% smaller. The
single-run Balanced secure daemon path reported `kdf_ms=0`, a 100% compressed cache
hit rate, and no dirty cache shards.

## Pipeline Evidence

- First-pack miss path used the scheduler and buffer pool for seven prepared blocks.
- Warm daemon/session path reused the L1 index in the benchmark process.
- Fastest 32 MiB regression: 5 ms, 32 sealed hits, 32 chunk-plan hits, one cache-pack
  range open, and 32 MiB of source reads skipped.
- Fastest critical stages reported scan, plan, compression, crypto, and payload write
  at 0 ms timer resolution.

## Environment Gate

The selected volume was not qualified for the absolute large-volume gate:

- 256 MiB native copy median: 462.76 MiB/s
- Required median: 650 MiB/s
- 256 MiB native copy p95: 496.50 MiB/s
- Required p95: 500 MiB/s

The source/zip comparative gate is valid because both tools ran on the same volume.
The 400 MiB/s large-volume Hig release claim is not made from this environment.

## Remaining Hotspots

1. A standalone CLI pack still reparses the cache index unless it runs repeatedly in
   one process; the current daemon owns session material but does not execute remote
   pack jobs.
2. Sub-millisecond stages are rounded to zero in `PackReport`; benchmark wall timing
   uses higher-resolution samples, but stage-level microsecond telemetry remains future work.
3. The large-volume absolute throughput gate needs a volume that passes the native-copy
   qualification threshold.

## Security

- Secure Argon2id parameters are unchanged.
- Session keys remain memory-only and the Unix socket is mode `0600`.
- `--daemon required` fails before packing when no active daemon/session service exists.
- Default Balanced mode does not enable metadata trust or sealed equality reuse.
