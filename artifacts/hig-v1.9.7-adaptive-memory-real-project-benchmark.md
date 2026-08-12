# HIG v1.9.7 Adaptive Payload Memory Benchmark

Date: 2026-07-05  
Corpus: 17,583 files, 505,906,599 bytes  
Archive size: approximately 248.15 MB

## Objective

Verify that the v1.9.7 default adaptive payload-memory policy avoids the
180.51 MB same-volume spool observed with the fixed 64 MiB policy on the real
IDE project corpus.

## Method

- Fixed release binary: `hig 1.9.7`
- Payload memory mode: `adaptive`
- Three runs, each with a new empty cache
- Daemon and project snapshot reuse disabled
- Speed mode: `fastest`
- Encryption disabled
- Output and cache stored on `/Volumes/Build`
- No samples discarded

Command shape:

```text
hig pack <corpus> --output <archive> --cache-dir <fresh-cache> \
  --daemon off --project off --speed fastest --encryption none \
  --memory-mode adaptive --json
```

## Results

| Metric | Run 1 | Run 2 | Run 3 | Median |
|---|---:|---:|---:|---:|
| Core duration | 94.33 s | 69.49 s | 63.01 s | 69.49 s |
| Scan | 41.34 s | 29.61 s | 23.76 s | 29.61 s |
| Plan | 0.002 s | 0.003 s | 0.002 s | 0.002 s |
| Block prepare | 47.31 s | 34.28 s | 33.40 s | 34.28 s |
| Output write | 5.51 s | 5.48 s | 5.75 s | 5.51 s |
| Payload memory | 247.62 MB | 247.62 MB | 247.62 MB | 247.62 MB |
| Spool payloads | 0 | 0 | 0 | 0 |
| Spool bytes | 0 | 0 | 0 | 0 |
| Reported peak memory | 289.56 MB | 289.56 MB | 289.56 MB | 289.56 MB |

The resolved adaptive budget was 505,906,599 bytes in all three runs. Reported
available physical memory ranged from 3.01 GB to 4.58 GB, so the workload target,
rather than the system-memory limit, selected the budget.

## Historical Comparison

| Variant | Duration median | Block prepare median | Spool bytes | Peak memory |
|---|---:|---:|---:|---:|
| v1.9.7 fixed 64 MiB spool | 503.20 s | 336.05 s | 180.51 MB | 125.83 MB |
| v1.9.7 adaptive | 69.49 s | 34.28 s | 0 | 289.56 MB |

The historical runs were collected on 2026-07-02 under severe, non-stationary
storage latency, while the adaptive runs were collected on 2026-07-05.
Therefore, the apparent 7.24x duration difference must not be attributed solely
to the memory policy. The spool-byte and memory measurements are deterministic
for this workload and are directly comparable.

## Correctness

The archive from run 3 was fully unpacked with the current v1.9.7 binary:

- Unpack result: success
- Output files: 17,583
- Output bytes: 505,906,599
- Input files: 17,583
- Input bytes: 505,906,599

## Conclusion

The default adaptive policy meets the target for this real IDE project:

1. It eliminates all 180.51 MB of payload spooling.
2. It retains the complete 247.62 MB compressed payload in memory.
3. It produces a valid archive with an exact full-corpus unpack.
4. Its reported peak pipeline memory is 289.56 MB, compared with 125.83 MB in
   explicit low-memory mode.

The fixed 64 MiB spool policy should remain available as `--memory-mode low`.
The adaptive policy is appropriate as the v1.9.7 default for this workload.

## Artifacts

```text
/Volumes/Build/hig-real-project-bench-20260702/bin/hig-v197-adaptive
/Volumes/Build/hig-real-project-bench-20260702/runs-adaptive-20260705
/Volumes/Build/hig-real-project-bench-20260702/unpacked-adaptive-20260705
```
