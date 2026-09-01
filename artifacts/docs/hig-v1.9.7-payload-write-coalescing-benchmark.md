# HIG v1.9.7 Payload Write Coalescing Benchmark

Date: 2026-07-22

## Scope

The baseline is the validated v1.9.7 cold-pipeline-fusion release. The candidate
adds only payload write coalescing and associated telemetry.

Corpus and options:

- 17,583 files and 505,906,599 input bytes;
- approximately 248 MB archive;
- fresh cache and output for every sample;
- `--daemon off --project off --speed fastest --encryption none`;
- archive and cache deleted after every run;
- `sync` and a 10-second settling interval between final ABBA samples.

## Final ABBA Results

| Order | Binary | Total | Block prepare | Output write | Payload write | Flush | Writer batches |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | baseline | 3.101 s | 1,177 ms | 981 ms | 909.6 ms | 54.0 ms | 1,101 |
| 2 | coalesced | 2.484 s | 948 ms | 904 ms | 870.7 ms | 23.7 ms | 35 |
| 3 | coalesced | 2.113 s | 849 ms | 645 ms | 637.6 ms | 4.9 ms | 35 |
| 4 | baseline | 2.009 s | 640 ms | 750 ms | 732.5 ms | 17.1 ms | 1,101 |

Median comparison:

| Metric | Baseline | Coalesced | Change |
|---|---:|---:|---:|
| Total | 2.555 s | 2.299 s | -10.0% |
| Output write | 865.5 ms | 774.5 ms | -10.5% |
| Payload write | 821.1 ms | 754.1 ms | -8.2% |
| Writer batch submissions | 1,101 | 35 | -96.8% |

The candidate coalesced all 1,101 memory payloads and 236.1 MiB of payload
bytes. Physical `archive-write` telemetry contained 8 samples in each baseline
run and 9 in each candidate run; the 96.8% submission reduction is therefore
not presented as an equivalent syscall reduction.

The pack/writer peak-memory metric was exactly 795,469,786 bytes in all four
runs. The candidate adds only bounded `IoSlice` descriptors and no payload
staging allocation. Variation in the broader pipeline estimate came from the
independent warm-compression buffer pool.

## Correctness

- source files: 17,583;
- unpacked files: 17,583;
- source bytes: 505,906,599;
- unpacked bytes: 505,906,599;
- sorted SHA-256 manifests: byte-for-byte identical.

Raw reports are retained under:

```text
/private/tmp/Hig-Test/runs-20260722-write-coalescing-final-abba
```
