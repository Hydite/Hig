# HIG v1.10.0 Post-Migration Cold-Cache Benchmark

Date: 2026-08-13
Corpus: 17,583 files, 505,906,599 bytes
Storage: `/Volumes/Windows/Hig-Test/corpus-links` (enterprise NAS/iSCSI volume)

## Objective

Verify that the current post-migration CLI preserves cold-pack correctness and
the adaptive payload/write behavior after the CLI modularization, repository
reference migration, and archive migration changes.

This is a cold HIG-cache benchmark. It is not an OS page-cache benchmark;
macOS page-cache eviction was not forced because the operation is unavailable
to the unprivileged test process.

## Method

- Current CLI: `hig 1.10.0` from the post-migration build.
- Three independent empty HIG cache directories and output archives.
- Daemon and project snapshot reuse disabled.
- Fastest compression, HIGV2 compact manifest, and no encryption.
- Archive and cache were written on the same enterprise volume as the corpus.
- Every output archive was unpacked into a fresh directory.
- Source and restored trees were compared by relative-path list, total bytes,
  and per-file SHA-256 digest.

Command shape:

```text
hig pack <corpus> --output <archive> --cache-dir <fresh-cache> \
  --daemon off --project off --speed fastest --encryption none --json
```

## Results

| Metric | Run 1 | Run 2 | Run 3 | Median |
|---|---:|---:|---:|---:|
| Total duration | 38.117 s | 25.794 s | 25.780 s | 25.794 s |
| Scan | 28.716 s | 16.896 s | 16.736 s | 16.896 s |
| Block preparation | 4.771 s | 4.401 s | 4.592 s | 4.592 s |
| Output write | 4.510 s | 4.404 s | 4.359 s | 4.404 s |
| Payload write | 3.980 s | 3.882 s | 4.114 s | 3.980 s |
| Output flush | 333 ms | 345 ms | 82 ms | 333 ms |
| Archive bytes | 248,145,553 | 248,145,670 | 248,145,548 | 248,145,553 |
| Pipeline peak memory | 795,469,786 | 795,469,786 | 795,469,786 | 795,469,786 |

All runs processed exactly 17,583 files and 505,906,599 input bytes. The
adaptive payload policy retained 247,620,147 compressed payload bytes in
memory and generated zero spool payloads in each run. The write coalescer
reduced the 1,101 memory payloads to 35 coalesced writer submissions in the
representative second run.

## Adaptive I/O Observations

The controller responded during the same pack operation rather than relying on
a startup-only disk classification. In the representative second run, source
scan concurrency moved from 10 to 5 and then 2 after repeated small-read
latency windows. The write path independently reduced its target from 10 to 5
and later recovered through 6, 7, and 8. This behavior is consistent with the
runtime adaptive-I/O design; it should not be interpreted as a fixed NAS
profile.

## Correctness

- Source files: 17,583.
- Restored files: 17,583.
- Source bytes: 505,906,599.
- Restored bytes: 505,906,599.
- Sorted relative-path lists: identical.
- Per-file SHA-256 manifests: byte-for-byte identical.

## Interpretation

The median current run is approximately 25.8 seconds on this enterprise
volume, with scan/hash work accounting for the dominant stage. The first run
was slower at 38.1 seconds because NAS and filesystem cache state varied. The
three samples are therefore evidence of correctness and current stage shape,
not a portable speedup claim against v1.9.6 or v1.9.7. Cross-version speed
claims require a counterbalanced run on the same qualified storage state.

Raw reports are retained outside the repository under:

```text
/Volumes/Windows/Hig-Test/current-v1.10.0-post-migration-cold/report-1.json
/Volumes/Windows/Hig-Test/current-v1.10.0-post-migration-cold/report-2.json
/Volumes/Windows/Hig-Test/current-v1.10.0-post-migration-cold/report-3.json
```
