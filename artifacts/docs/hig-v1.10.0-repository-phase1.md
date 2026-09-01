# HIG v1.10.0 Repository History Phase 1

Date: 2026-07-25

## Scope

Phase 1 introduces HIG's independent repository history. It is not a Git object
adapter and does not use the mutable project/cache snapshot as historical
truth. Repository state lives under `.hig/repository`.

Implemented object graph:

```text
ref -> commit -> tree -> file -> chunk
               -> subtree
```

Every object is immutable, domain-separated with its type and raw length,
addressed by BLAKE3, independently zstd-compressed, and wrapped in a checked
length/checksum envelope. Commit objects reserve typed links for Phase 2 change
and compression-tree indexes and the Phase 3 semantic index.

## Atomicity

- A repository writer lock serializes init, snapshot, and GC mutation.
- Objects are written through same-directory temporary files and renamed.
- New object files are synced in parallel and shard directories are synced once
  per snapshot; the commit and all dependencies are durable before `refs/HEAD`
  is atomically replaced.
- A failed snapshot may leave unreachable objects but never publishes a partial
  commit.
- Restore builds a complete staging tree before replacing an existing output;
  overwrite uses a rollback-capable backup rename.

## Commands

```text
hig repo init
hig repo snapshot
hig repo log
hig repo diff
hig repo restore
hig repo verify
hig repo gc
```

Revision lookup accepts `HEAD`, a full commit ID, or an unambiguous prefix of at
least eight hexadecimal characters. GC is report-only unless `--apply` is
specified.

## Recovery Coverage

- regular files;
- empty directories;
- symbolic links with byte-preserved targets on Unix;
- permissions on regular files;
- whole repository, directory, or single-path selection;
- no-op snapshots without synthetic commits;
- exact reconstruction without walking a delta chain.

Phase 1 fixed-size chunking uses 1 MiB chunks. A three-chunk test that changes
one byte writes one new chunk and reuses two unchanged chunks. Phase 2 replaces
this coarse boundary behavior with versioned content-defined micro-chunking.

## Safety Tests

- reachable object corruption fails verification;
- corrupt object reuse aborts snapshot without moving HEAD;
- default GC leaves unreachable objects untouched;
- applied GC removes only unreachable objects;
- restore refuses overwrite by default;
- staged overwrite preserves the existing target until reconstruction passes;
- unsafe tree paths and repository metadata restore paths are rejected;
- stored and decompressed object sizes are bounded before allocation.

## Phase Boundary

Phase 1 establishes durable history and exact recovery. It does not yet claim
automatic watcher commits, content-defined micro-change indexing, rename-aware
indexes, function-level lookup, or IDE/MCP history tools. Those remain required
for Phases 2 and 3 and for completion of the overall repository-history goal.
