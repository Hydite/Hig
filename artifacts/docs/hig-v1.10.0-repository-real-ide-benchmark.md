# HIG v1.10.0 Repository Real IDE Benchmark

Date: 2026-07-25  
Volume: `/Volumes/Windows` enterprise storage  
Corpus: 17,583 files, 526 MiB before repository defaults

## Objective

Validate the three-phase repository implementation on the existing real IDE
corpus, including cold snapshot behavior, interrupted-snapshot cleanup,
one-byte indexing, content-defined chunk reuse, Swift symbol lookup, function
restore, and full reachability verification.

## Cold-Path Finding and Fix

The first unfiltered run used one `sync_all` plus one directory sync per loose
object. It was interrupted after 719.34 seconds with no HEAD published. GC
found 21,629 unreachable objects and one interrupted temporary file.

The write path was changed to:

1. write each object through a same-directory temporary file and rename;
2. collect every new object ID in the snapshot transaction;
3. sync new object files in parallel;
4. deduplicate and sync object shard directories;
5. publish HEAD only after all durability barriers pass.

This preserves HEAD-last publication and removes repeated serial directory
barriers. GC was also hardened to ignore strict-ID-invalid files and report or
remove only recognized interrupted `.tmp` files.

The corpus contained a large `.venv-*` tree and `.build` output. Parsing those
third-party/generated trees produced an oversized semantic index and is not a
valid source-history workload. `.venv`, `.venv-*`, `venv`, `.build`, and
`DerivedData` are now repository defaults and are migrated into existing
configuration before snapshot.

## Source Corpus After Defaults

```text
files:       50
input bytes: 3,006,708
languages:   primarily Swift
```

This is the project-owned source/configuration set. Archive packing remains a
separate operation and can still include dependency/build payloads when its own
rules request them.

## Final Results

| Operation | Wall time | Objects written | Chunks written | Chunks reused |
| --- | ---: | ---: | ---: | ---: |
| Cold repository snapshot | 4.27 s | 150 | 79 | 3 |
| One-byte Swift edit | 1.16 s | 9 | 1 | 81 |
| Semantic schema upgrade, no content change | 2.23-7.17 s | 3 | 0 | 82 |

The one-byte edit changed `0.001` to `0.002` inside
`RouteInterpolator::location`.

```text
path: Sources/TelemetryLocationKit/RouteInterpolator.swift
old_start: 1466
old_len: 1
new_start: 1466
new_len: 1
```

Two overloaded `RouteInterpolator::location` methods produced two unique
signature-aware symbol IDs. The modified overload's history was `added ->
modified`. Restoring the function from baseline and HEAD produced files with
exactly one differing byte.

Final verification:

```text
checked_objects: 159
change_indexes: 2
semantic_indexes: 2
function_byte_differences: 1
```

## Safety Evidence

- Both failed/interrupted cold runs left HEAD unchanged.
- Reachability GC removed 21,629 and later 38,611 unpublished objects without
  touching corpus files.
- One recognized interrupted temporary file was reported and removed.
- New object data is durable before HEAD publication under the batched barrier.
- Parser schema changes create index-only commits even when the content tree is
  unchanged; file chunks remain fully reused.

## Retained Test Data

```text
/Volumes/Windows/Hig-Test/unpacked-object-pack-20260722
/Volumes/Windows/Hig-Test/repository-v110-incremental-20260725
/Volumes/Windows/Hig-Test/route-location-baseline.swift
/Volumes/Windows/Hig-Test/route-location-one-byte.swift
```
