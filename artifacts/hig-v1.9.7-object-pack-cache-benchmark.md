# HIG v1.9.7 Object Pack Cache Benchmark

Date: 2026-07-22

## Scope

This stage targets first-pack cache-object write cost. Previous profiling on
the 17k-file IDE corpus showed that block preparation still spent about
5.4-5.7 s even after source hot raw reuse removed repeated source-file reads.

The implementation stores parameterized compressed cache objects in a packed
cache file:

```text
.hig-cache/object-packs/objects.pack
```

The cache index records `pack_file` and `pack_offset` for parameterized single,
batch, solid, and chunk objects. Legacy object files in `.hig-cache/blocks`
remain readable as fallback.

Archive format, manifest format, unpack behavior, and sealed payload cache
format are unchanged.

## Implementation Notes

- `CacheRecord` now supports optional `pack_file` and `pack_offset`.
- `CacheObjectRecord` now supports optional `pack_file` and `pack_offset`.
- `insert_parameterized`, `insert_batch`, and `insert_chunk` append compressed
  payloads to the object pack.
- `get`, `get_batch`, `get_chunk`, `has`, `has_batch`, and `has_chunk` first
  consult packed object locations and then fall back to legacy block files.
- The object pack writer is reused for the lifetime of a `CacheStore` and
  flushed before cache index/journal save.
- Existing legacy cache records deserialize with missing pack fields.

## Test Corpus

```text
/Volumes/Windows/Hig-Test/corpus-links
```

- Files: 17,583
- Input bytes: 505,906,599

Command shape:

```text
hig pack /Volumes/Windows/Hig-Test/corpus-links \
  --output /Volumes/Windows/Hig-Test/corpus-links/.hig-real-benchmark-output.hig \
  --cache-dir /Volumes/Windows/Hig-Test/runs-20260722-object-pack-cache/<cache> \
  --daemon off --project off --speed fastest --encryption none \
  --memory-mode adaptive --json
```

## Results

| Run | Core duration | Scan | Block prepare | Output write | Cache blocks files | Object pack files | Object pack bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Baseline default, old object files | 27.75 s | 17.53 s | 5.52 s | 4.61 s | ~1,101 | 0 | 0 |
| Object pack, per-insert open | 28.78 s | 16.92 s | 7.07 s | 4.68 s | 0 | 1 | 247,620,147 |
| Object pack, reused writer | 28.70 s | 18.97 s | 4.93 s | 4.70 s | 0 | 1 | 247,620,147 |
| Object pack, second pack same cache | 9.56 s | 0.68 s | 3.80 s | 4.85 s | 0 | 1 | 247,620,147 |

The first object-pack attempt removed the many object files but reopened the
same pack file for every inserted object. That was slower and was replaced with
a reused writer.

## Interpretation

The final implementation reduces the cache object file count from roughly 1,101
files to a single object pack and modestly improves block preparation on the
same Windows test disk:

```text
5.52 s -> 4.93 s
```

The total duration is still dominated by scan variability on the APFS/iSCSI
volume. In this run, scan was 18.97 s versus 17.53 s in the previous baseline.

The same-cache second pack confirms that packed object records are reusable:

- `cache_misses`: 0
- `batch_cache_hits`: 394
- `chunk_cache_hits`: 137
- `blocks_files`: 0
- `object_pack_files`: 1

## Correctness

The archive from the same-cache run was unpacked successfully:

- Input files: 17,583
- Input bytes: 505,906,599
- Output files: 17,583
- Output bytes: 505,906,599

## Verification

- `cargo fmt --all --check`: passed
- `cargo test -p hig-core -p hig-cli`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- Full real-corpus unpack count/byte check: passed

## Artifacts

```text
/Volumes/Windows/Hig-Test/runs-20260722-object-pack-cache/default.json
/Volumes/Windows/Hig-Test/runs-20260722-object-pack-cache/default-2.json
/Volumes/Windows/Hig-Test/runs-20260722-object-pack-cache/writer-reuse.json
/Volumes/Windows/Hig-Test/runs-20260722-object-pack-cache/writer-reuse-second.json
/Volumes/Windows/Hig-Test/unpacked-object-pack-20260722
```
