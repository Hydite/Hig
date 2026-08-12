# HIG v1.9.7 Real IDE Project Cold-Path Benchmark

Date: 2026-07-02  
Host volume: `/Volumes/Build`  
Corpus: 17,583 files, 505,906,599 bytes

## Objective

This benchmark compares three packer implementations on the previously measured
real IDE project corpus:

1. **v1.9.6**: released v1.9.6 source build.
2. **v1.9.7 baseline**: v1.9.7 cold-read and large-file hashing improvements,
   with payload spooling disabled.
3. **v1.9.7 write-optimized**: current v1.9.7 implementation with a 64 MiB
   in-memory payload budget and same-volume payload spooling.

The benchmark is intended to evaluate first-pack behavior. It does not measure
incremental cache reuse.

## Method

- The corpus was reconstructed from the original archive manifest and verified
  to contain exactly 17,583 files and 505,906,599 bytes.
- Each run used a new empty cache directory.
- Project snapshot and daemon reuse were disabled.
- Compression mode was `fastest`; encryption was disabled.
- Output and cache data were placed on the same `/Volumes/Build` volume.
- Each implementation ran three times in an interleaved Latin-square order:
  `A B C / C A B / B C A`.
- No samples were discarded.

Command shape:

```text
hig pack <corpus> --output <archive> --cache-dir <fresh-cache> \
  --daemon off --project off --speed fastest --encryption none --json
```

## Results

All nine runs processed exactly 17,583 files and 505,906,599 input bytes.
Archive sizes remained within 156 bytes of one another.

| Variant | Core duration, median (range) | Scan median | Plan median | Block prepare median | Output write median |
|---|---:|---:|---:|---:|---:|
| v1.9.6 | 311.86 s (242.97-646.46) | 163.29 s | 1.822 s | 139.16 s | 8.55 s |
| v1.9.7 baseline | 491.43 s (188.40-492.92) | 259.19 s | 0.003 s | 188.27 s | 8.43 s |
| v1.9.7 write-optimized | 503.20 s (461.52-604.83) | 179.09 s | 0.074 s | 336.05 s | 11.45 s |

Individual core durations:

| Variant | Run 1 | Run 2 | Run 3 |
|---|---:|---:|---:|
| v1.9.6 | 242.97 s | 646.46 s | 311.86 s |
| v1.9.7 baseline | 188.40 s | 491.43 s | 492.92 s |
| v1.9.7 write-optimized | 503.20 s | 604.83 s | 461.52 s |

## Memory and Payload Staging

| Variant | In-memory payload | Spool payload | Reported peak pipeline memory |
|---|---:|---:|---:|
| v1.9.6 | 247.62 MB | 0 | 247.62 MB |
| v1.9.7 baseline | 247.62 MB | 0 | 289.56 MB |
| v1.9.7 write-optimized | 67.11 MB | 180.51 MB | 125.83 MB |

The write-optimized implementation reduces reported peak pipeline memory by
56.5% relative to the v1.9.7 baseline. It does so by moving 180.51 MB of payload
through a same-volume spool.

## Interpretation

The storage device exhibited severe non-stationary latency during the run.
For example, v1.9.6 varied from 242.97 seconds to 646.46 seconds, while its CPU
time remained below six seconds. Consequently, absolute median comparisons
between versions are not reliable evidence of a general performance regression
or improvement on their own.

The first interleaved v1.9.6 and v1.9.7 baseline samples were collected under
similar early-run conditions. In that pair, v1.9.7 decreased core duration from
242.97 seconds to 188.40 seconds (22.5%) and reduced plan time from 1.601 seconds
to 2 milliseconds. This is consistent with the intended cold large-file
hash/chunk reuse improvement, but a less variable storage environment is needed
for a publishable speed claim.

The 64 MiB payload-spool strategy has a clear and repeatable memory benefit.
It does not demonstrate a throughput benefit on this workload. Its median block
preparation time was 336.05 seconds, compared with 188.27 seconds for the
non-spooling v1.9.7 baseline. Final archive output remained only 8-15 seconds,
so the primary cost occurs while preparing and staging payloads, not during the
final sequential archive write.

## Decision

1. Keep the v1.9.7 cold large-file read/hash/chunk optimization.
2. Do not make the 64 MiB same-volume spool policy the unconditional default.
3. Retain spooling as an explicit low-memory mode or activate it only above a
   substantially larger adaptive memory threshold.
4. For the default mode, prefer an adaptive budget based on available memory and
   expected payload size; avoid spooling a roughly 248 MB archive on ordinary
   developer machines.
5. Repeat the benchmark on a stable local NVMe/APFS volume before publishing a
   v1.9.7 speed comparison.

## Artifacts

Raw JSON reports, timing files, fresh caches, and fixed benchmark binaries are
stored under:

```text
/Volumes/Build/hig-real-project-bench-20260702
```

The benchmark corpus and generated output are retained for reproducibility.
