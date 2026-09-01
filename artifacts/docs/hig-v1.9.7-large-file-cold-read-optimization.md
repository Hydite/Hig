# Hig v1.9.7 Large-File Cold Read Optimization

Date: 2026-07-02

## Scope

This change targets first-pack cold compression for large files. The archive format is unchanged.

The optimized path removes redundant large-file reads between:

1. whole-file BLAKE3 hashing during scan,
2. per-chunk BLAKE3 hashing during block planning,
3. chunk compression during block preparation.

## Implementation

- `ScannedFile` now carries process-local `hot_chunks`.
- During computed scans, large files produce chunk hashes from the same bytes already read for the whole-file hash.
- Balanced mode stores the chunk compression-level hint during scan; fastest mode avoids the probe and keeps level 1.
- Chunk raw bytes are retained only within the existing hot raw byte budget.
- `PlannedBlock::Chunk` can carry process-local raw bytes for compression reuse.
- No `.hig` header, manifest, block layout, cache schema, or unpack logic changed.

## Benchmark Corpus

Path: `/private/tmp/hig-v197-large-read-bench/corpus`

- Files: 492
- Size: 292 MB
- Large files: 16 files x 16 MiB
- Chunk blocks: 256
- Corpus digest: `b1ce0d580ec41f7aa52ccbdf3a92a6668641e4d0226feb23a503ceb787de5734`

Command shape:

```bash
hig pack /private/tmp/hig-v197-large-read-bench/corpus \
  --output out.hig \
  --cache-dir fresh-cache \
  --daemon off \
  --encryption none \
  --json
```

Each run used a fresh Hig cache directory. OS page cache was not flushed.

## Median Results

| Metric | v1.9.6 | v1.9.7 optimized | Change |
|---|---:|---:|---:|
| CLI wall time | 1.47 s | 1.41 s | -4.1% |
| Core duration | 1.457 s | 1.405 s | -3.6% |
| Scan | 35 ms | 62 ms | +27 ms |
| Plan | 146 ms | 0 ms | -146 ms |
| Block prepare | 701 ms | 699 ms | -2 ms |
| Pipeline read | 2 ms | 4 ms | +2 ms |
| Compression | 54 ms | 64 ms | +10 ms |
| Output write | 513 ms | 539 ms | +26 ms |
| Archive size | 164,005,870 B | 164,005,874 B | effectively unchanged |

v1.9.7 scan telemetry:

- `hot_chunk_plans`: 256
- `hot_chunk_raw_bytes`: 13,631,488
- `hot_raw_bytes`: 52,855,796

## Interpretation

The intended optimization is visible in stage movement:

- v1.9.6 spent about 146 ms in planning because large files were reread to compute chunk hashes.
- v1.9.7 moves that work into scan, where the bytes are already available from whole-file hashing.
- Planning drops to 0 ms for this corpus.

End-to-end speedup is modest in this macOS benchmark because output writing and block preparation dominate, and the OS page cache hides part of the repeated-read cost. On colder disks or larger large-file-heavy projects, the removed second large-file read should matter more.

## Compatibility

The optimized v1.9.7 archive was unpacked with:

- v1.9.7 current CLI
- v1.9.6 release CLI

Both restored trees matched the source corpus digest:

`b1ce0d580ec41f7aa52ccbdf3a92a6668641e4d0226feb23a503ceb787de5734`

## Verification

- `cargo check -p hig-core -p hig-cli`: passed
- `cargo test -p hig-core`: 98 passed
- `cargo test -p hig-cli`: 10 passed
- `cargo build --release -p hig-cli`: passed
- Cross-version unpack digest check: passed
