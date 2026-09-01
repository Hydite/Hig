# Hig v1.9.7 Write Path Profiling

Date: 2026-07-02

## Scope

This stage adds write-path profiling only. It does not change the `.hig` archive format, block layout, compression, encryption, or unpack compatibility.

The report now separates output writing into:

- temporary file creation
- preallocation
- header write
- manifest write
- payload read
- payload write
- direct write
- buffered write
- writer wait
- flush
- fsync
- rename
- payload source counts and bytes

The new data is available under `PackReport.write_profile` and mirrored into selected `PackTimingsUs` fields.

## Benchmark Corpus

Path: `/private/tmp/hig-v197-large-read-bench/corpus`

- Files: 492
- Size: 292 MB
- Archive payload bytes: 163,982,192
- Payload count: 396 memory payloads
- Cached payload count: 0

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

## Median Write Profile

| Metric | Median |
|---|---:|
| Core duration | 1.989 s |
| Total write stage | 658 ms |
| `output_write_us` | 658,705 us |
| Payload write | 641,068 us |
| Buffered write | 640,911 us |
| Direct write | 0 us |
| Flush | 15,872 us |
| Rename | 122 us |
| Preallocate | 9 us |
| Temp create | 52 us |
| Header write | 1 us |
| Manifest write | 1 us |

Payload organization:

- `memory_payload_count`: 396
- `memory_payload_bytes`: 163,982,192
- `cached_file_payload_count`: 0
- `cached_range_payload_count`: 0
- `direct_write_count`: 0
- `buffered_write_count`: 396

## Diagnosis

The write bottleneck is not manifest writing, preallocation, rename, or flush/fsync.

The dominant cost is payload writing:

- About 97% of `output_write_us` is `payload_write_us`.
- All payloads are memory payloads.
- All payloads go through buffered writes.
- There are no cached payload reads and no writer wait time in this workload.
- `fsync_us` is currently 0 because the writer flushes the userspace buffer but does not call `sync_all`.

This means the next optimization should target the memory-payload write path:

1. reduce the number of buffered payload writes,
2. evaluate direct-write thresholds for medium payloads,
3. consider coalescing adjacent memory payloads before writing,
4. consider streaming prepared blocks to the writer earlier to reduce peak memory and final write burst.

## Verification

- `cargo check -p hig-core -p hig-cli`: passed
- `cargo build --release -p hig-cli`: passed
- pack/unpack smoke with `write_profile` JSON validation: passed

Note: full debug `cargo test -p hig-core` and filtered `cargo test -p hig-core writer` both stalled in the local test binary at 0% CPU after successful compilation in this environment. No assertion failure was emitted. Earlier in this stage, before the final direct/buffered timing fields were added, `cargo test -p hig-core` passed 98 tests and `cargo test -p hig-cli` passed 10 tests.
