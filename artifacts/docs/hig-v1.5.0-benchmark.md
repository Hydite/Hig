# Hig v1.5.0 Benchmark

Date: 2026-06-19

Build: release, 8 workers, macOS system data volume (`/private/tmp`), 39 GiB free.

## Acceptance Status

`NOT_RELEASE_READY`

Compression quality, metadata size, cache correctness, critical timing attribution, and Fastest median passed. Balanced secure latency versus zip and Fastest p95 did not pass. Default Argon2id security parameters were not reduced.

## Source Dataset

Input: 16 files, 295,974 bytes. Both tools used the same input tree and exclusions.

| Metric | Hig Balanced | zip |
|---|---:|---:|
| Archive bytes | 49,621 | 55,318 |
| Reduction | 83.23% | 81.31% |
| Difference | 10.30% smaller | baseline |
| Header bytes | 64 | - |
| Manifest plain bytes | 1,633 | - |
| Manifest compressed bytes | 712 | - |
| Manifest protected bytes | 728 | - |
| Header + protected manifest | 792 | - |
| Payload bytes | 48,829 | - |
| Compression levels | level 5: 3 blocks | implementation default |

The original v1.4.2 reference sample also improved from 53,318 bytes to 46,115 bytes; its fair zip result was 49,644 bytes.

## Balanced Latency

Each row uses 20 runs after warm-up. Hig timings are internal `pack()` wall time; zip uses high-resolution process wall time.

| Scenario | Median | p95 | Result |
|---|---:|---:|---|
| Hig Balanced secure, cold compressed cache | 18 ms | 21 ms | fail versus zip |
| Hig Balanced secure, warm compressed cache | 16 ms | 17 ms | fail versus zip |
| zip `-qr` | 7.55 ms | 9.00 ms | baseline |
| Cold `unattributed_ms` | 1 ms | 2 ms | pass |
| Warm `unattributed_ms` | 1 ms | 1 ms | pass |

Warm Balanced is 2.12x zip, above the 1.2x target. Its critical path is almost entirely secure Argon2id (typically 13-17 ms); scan, plan, block preparation, manifest, cache commit, and output are each 0-1 ms on a warm run.

## Compression Quality

| Dataset | Input bytes | Archive bytes | Result |
|---|---:|---:|---|
| 500 small text files | 17,500 | 4,273 | pass; below v1.4.2 by a wide margin |
| Repeated text | 4,575,600 | 715 | pass; 99.98% reduction |
| Random data | 8,388,608 | 8,389,889 | pass; 0.0153% expansion |
| Mixed source tree | 295,974 | 49,621 | pass; 10.30% smaller than zip |

## Fastest Regression

32 MiB random file, same output target, 20 sealed-cache runs:

| Metric | Result | Target | Status |
|---|---:|---:|---|
| Median | 22 ms | <90 ms | pass |
| p95 | 279 ms | <120 ms | fail, storage outliers |
| Sealed/cache-pack hits | 32 | 32 | pass |
| Cached range opens | 1 | <=1 | pass |
| Modified 4 KiB | 31 hits / 1 miss | approximately 31 / 1 | pass |

The system volume native 256 MiB copy probe measured about 244 MiB/s median and was marked `ENVIRONMENT_NOT_QUALIFIED`; Fastest p95 is therefore reported as a regression risk, not hidden.

## Verification

- `cargo fmt --all --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace`: pass, 57 tests
- `cargo build --release --workspace`: pass
- `hig --version`: `hig 1.5.0`
