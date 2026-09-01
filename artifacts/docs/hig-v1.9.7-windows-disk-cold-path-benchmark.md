# HIG v1.9.7 Windows Disk Cold-Path Benchmark

Date: 2026-07-22

## Scope

This benchmark moves real-project performance testing from `/Volumes/Build` to
`/Volumes/Windows/Hig-Test`.

The goal is to isolate HIG performance measurements from the heavily used build
volume and to evaluate the next cold-path optimization target after hot raw
reuse.

No archive format, manifest, cache schema, or unpack behavior changed in this
stage.

## Test Workspace

```text
/Volumes/Windows/Hig-Test
```

Disk:

- Mount: `/Volumes/Windows`
- File system: APFS
- Protocol: iSCSI
- Free space before tests: approximately 495 GiB

Corpus:

```text
/Volumes/Windows/Hig-Test/corpus-links
```

- Files: 17,583
- Input bytes: 505,906,599

The copied corpus initially contained a stale `.hig-real-benchmark-output.hig`
from the source benchmark directory. It was removed from the Windows test
workspace only, bringing the corpus back to the expected file and byte counts.

## Command Shape

```text
hig pack /Volumes/Windows/Hig-Test/corpus-links \
  --output /Volumes/Windows/Hig-Test/corpus-links/.hig-real-benchmark-output.hig \
  --cache-dir /Volumes/Windows/Hig-Test/<run-cache> \
  --daemon off --project off --speed fastest --encryption none \
  --memory-mode adaptive --json
```

Each run used a fresh HIG cache directory. OS page cache was not flushed.

## Baseline Results

| Run | Threads | Core duration | Scan | Block prepare | Output write | Source read during block prepare | Hot raw reused |
|---|---:|---:|---:|---:|---:|---:|---:|
| default | 10 | 27.75 s | 17.53 s | 5.52 s | 4.61 s | 0 B | 505,906,599 B |
| threads4 | 4 | 26.35 s | 15.98 s | 5.70 s | 4.59 s | 0 B | 505,906,599 B |
| threads8 | 8 | 10.65 s | 0.44 s | 5.42 s | 4.70 s | 0 B | 505,906,599 B |

The 8-thread run benefited from a hot OS page cache and should not be treated
as a cold-cache result. It is useful as a lower-bound measurement for CPU,
cache-object preparation, and output writing when source bytes are already in
memory.

## Observations

- Moving the benchmark to `/Volumes/Windows/Hig-Test` produced a best cold-ish
  result of 26.35 s, substantially better than the previous `/Volumes/Build`
  adaptive median of 69.49 s.
- Hot raw reuse is working: all measured runs report `source_read_bytes=0` and
  `source_hot_raw_bytes=505,906,599`.
- The remaining stable hot-cache floor is approximately:
  - block preparation: 5.4-5.7 s
  - output write: 4.6-4.7 s
- Scan time is highly variable on the iSCSI/APFS volume. Later runs observed
  scan times of 47.85 s and 85.57 s, while block preparation and output write
  remained in the same range.

## Discarded Optimization Attempt

A low-risk attempt was evaluated: return freshly compressed prewarm payloads to
the pack loop directly, avoiding a hot-payload map clone after cache insertion.

The attempt was not retained because it did not improve the measured
`block_prepare` stage on the real corpus:

- before: approximately 5.52 s
- after attempt: approximately 5.47 s and 6.35 s in noisy runs

It also risked increasing transient memory by retaining an extra compressed
payload map during prewarm. This failed the stage requirement that performance
and quality both improve.

## Next Target

The next worthwhile optimization is cache object write organization, not
payload clone avoidance:

1. First-pack cache preparation writes about 248 MB across roughly 1,101 cache
   object files.
2. Archive output writes another roughly 248 MB.
3. `--no-cache` hot-cache smoke showed block preparation can fall below 1 s,
   confirming cache-object persistence is a major part of the remaining
   prepared-block cost.

A future optimization should evaluate a packed compressed-object cache or
deferred grouped cache writer, while preserving the current cache reuse
semantics and crash safety.

## Verification

- `cargo fmt --all --check`: passed
- `cargo test -p hig-core -p hig-cli`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed

## Artifacts

```text
/Volumes/Windows/Hig-Test/runs-20260722-scan-baseline/default.json
/Volumes/Windows/Hig-Test/runs-20260722-scan-baseline/threads4.json
/Volumes/Windows/Hig-Test/runs-20260722-scan-baseline/threads8.json
/Volumes/Windows/Hig-Test/runs-20260722-warm-payload-opt/default.json
/Volumes/Windows/Hig-Test/runs-20260722-warm-payload-opt/default-2.json
```
