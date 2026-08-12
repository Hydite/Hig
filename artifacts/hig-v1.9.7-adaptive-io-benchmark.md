# HIG v1.9.7 Runtime-Adaptive I/O Benchmark

Date: 2026-07-22

## Scope

This stage implements one runtime-adaptive I/O controller across the HIGV2
pack path. It replaces the proposed startup-only disk classification. The goal
is to preserve full local-system-disk performance while adapting to temporary
load and slow external or enterprise storage during the same operation.

The benchmark uses the established IDE corpus:

- files: 17,583;
- input bytes: 505,906,599;
- archive bytes: approximately 248 MB;
- fresh HIG cache for every first-pack run;
- `--daemon off --project off --speed fastest --encryption none`;
- OS page cache was not forcibly purged because macOS denied the unprivileged
  purge operation.

## Storage

| Label | Path | Storage role |
|---|---|---|
| System | `/private/tmp/Hig-Test/corpus-system` | normal local IDE workload |
| Enterprise | `/Volumes/Windows/Hig-Test/corpus-links` | APFS over iSCSI pressure workload |

Both copies were verified as 17,583 files and 505,906,599 logical bytes.

## System-Disk Behavior

Three normal-state final-controller runs completed in 2.95 s, 2.97 s, and
3.80 s. Two stayed at concurrency 10 for the whole task. The third observed an
archive-write throughput drop below half of its own stage baseline and reduced
the target from 10 to 5 near task completion.

Counterbalanced old/new runs showed a strong order effect because each pack
writes roughly 496 MB across cache and archive output. When the new binary ran
first, it was 2-7% faster than the old binary that followed. When it ran second,
the second run was slower. These rows are retained as environmental evidence,
not as a release speedup claim.

A later ABBA sequence demonstrated accumulating system-disk pressure:

| Sequence | Binary | Core duration | Scan | Block prepare | Output write |
|---:|---|---:|---:|---:|---:|
| 1 | old | 6.87 s | 1.49 s | 1.68 s | 3.66 s |
| 2 | adaptive | 13.32 s | 1.57 s | 6.18 s | 5.46 s |
| 3 | adaptive | 20.31 s | 1.20 s | 4.85 s | 14.18 s |
| 4 | old | 26.32 s | 2.98 s | 9.85 s | 13.31 s |

The monotonic sequence rules out a binary-only explanation. During the two
adaptive runs, cache-pack write throughput crossed 47, 32, and 45 MiB/s; the
controller reduced 10 -> 5 -> 2 -> 1 and later observed recovery to 60 MiB/s,
raising the target to 2. No explicit disk profile was selected.

## Enterprise-Disk Behavior

The final three cold/variable iSCSI runs were:

| Run | Core duration | Scan | Block prepare | Output write | Min target | Final target | Transitions | Recoveries |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 40.27 s | 31.29 s | 4.55 s | 4.35 s | 1 | 2 | 6 | 2 |
| 2 | 22.26 s | 13.29 s | 4.47 s | 4.40 s | 2 | 4 | 4 | 2 |
| 3 | 21.88 s | 12.86 s | 4.50 s | 4.41 s | 2 | 4 | 4 | 2 |

The median is 22.26 s. Runs 2 and 3 reduced 10 -> 5 -> 2 after repeated
30-33 ms small-read p95 windows, then recovered to 3 during source scan and to
4 during fast packed-cache reads. The 750 ms cooldown reduced transition count
from approximately 11 in the pre-cooldown prototype to 4 in the stable runs.

A consecutive old/new slow-disk pair measured 25.07 s and 25.32 s. The 1.0%
difference is below the run-to-run scan and output variance seen on this iSCSI
volume and is not treated as a confirmed regression or speedup.

## Correctness

The final archive was unpacked with the release CLI:

- input files: 17,583;
- output files: 17,583;
- input bytes: 505,906,599;
- output bytes: 505,906,599;
- sorted SHA-256 manifests: byte-for-byte identical.

Archive format, manifest, compression levels, encryption semantics, and atomic
output replacement are unchanged.

## Verification Gates

- `cargo fmt --all --check`
- `cargo test -p hig-core -p hig-cli`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --release -p hig-cli`
- full 17,583-file unpack and SHA-256 comparison

Raw reports are retained under:

```text
/private/tmp/Hig-Test/runs-20260722-adaptive-io
/Volumes/Windows/Hig-Test/runs-20260722-adaptive-io
```
