# HIG v1.10.0 Repository History Phase 2

Date: 2026-07-25

## Scope

Phase 2 adds byte-level micro history on top of the Phase 1 recovery DAG. Exact
restore still depends only on commit, tree, file, and chunk objects. Change and
compression-tree indexes are independently verified acceleration and
introspection objects referenced by each new commit.

## Content-Defined Chunking

New repositories use FastCDC v2020 with a versioned schema and deterministic
parameters:

```text
minimum:  16 KiB
target:   64 KiB
maximum: 256 KiB
```

Existing schema-1 repositories are atomically upgraded before their next
snapshot. Historical file objects remain readable because every file stores
its ordered chunk references; restore never needs to rerun the old chunker.

A 3 MiB randomized test inserts one byte near the beginning and verifies that
all but at most three prior chunks are reused. A separate equal-length edit
test verifies that a one-byte change is recorded as a byte range of length one.

## Change Index

Each changed commit references an immutable `ChangeIndex` object containing:

- added, deleted, modified, metadata, and unambiguous rename records;
- old and new file object IDs and content hashes;
- encoding-independent old/new byte ranges;
- a committed cumulative path-history map.

The cumulative path history is part of the content-addressed object referenced
by HEAD. Path lookup therefore does not scan every commit and does not trust a
mutable side index. Pure renames are detected only when one deleted path maps
unambiguously to one added path with the same content and file type.

For equal-length modifications, disjoint differing byte runs are recorded
separately. For insertion or deletion, the range is minimized by common byte
prefix and suffix. These records locate changes; they are not a delta chain and
are never required for exact reconstruction.

## Compression Tree Index

Each changed commit also references a `CompressionTreeIndex` object with
per-path and aggregate raw bytes, ordered chunk counts, unique chunk counts,
and stored object bytes. This exposes the recovery tree's storage behavior
without reading and decompressing every payload.

## Commands

```text
hig repo history --path <path>
hig repo restore-range --path <path> --start <offset> [--len <bytes>] -o <file>
hig repo storage-tree [--revision <commit>]
hig repo watch [--debounce-ms <milliseconds>]
```

All commands support stable JSON output. Range restore uses a staged,
durability-synced publication and refuses accidental overwrite unless
`--overwrite` is supplied. Repository metadata cannot be selected as output.

The watcher ignores configured exclusions, including `.hig`, coalesces editor
event bursts, and creates a commit only when the source tree changes.

## Verification

- repository tests: 13 passed;
- one-byte range location and range restore passed;
- leading-insertion CDC reuse passed;
- rename-aware committed path history passed;
- compression-tree traversal and index reachability passed;
- storage-tree provenance links the committed compression tree to the
  discovered project ID, project snapshot generation, cache generation, cache
  index format, and cache directory when the repository root is also a Hig
  project;
- watcher debounce and parent-chain publication passed;
- CLI parsing for range restore and watcher passed.

## Phase Boundary

Phase 2 can recover any byte range and identify a one-letter change. It does
not infer that a range is a function, method, or class. Phase 3 adds
Tree-sitter-derived symbol identities, rename-aware semantic history,
function-level restore, and IDE/MCP history tools. Parser output remains an
optional index and cannot become a recovery dependency.
