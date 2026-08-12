# HIG Repository History Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build HIG's independent three-phase repository history system with exact byte recovery, micro-change indexing, compressed-tree introspection, and function-level semantic lookup.

**Architecture:** `.hig/repository` contains immutable BLAKE3-addressed and zstd-compressed objects plus atomically updated refs. Commit, tree, file, and chunk objects form the recovery DAG; later change and semantic-index objects attach to commits without becoming correctness dependencies.

**Tech Stack:** Rust 2024, BLAKE3, zstd, bincode, serde, walkdir, fs2, Tree-sitter in Phase 3.

---

## Phase 1: Repository DAG

### Task 1: Object protocol and storage

**Files:**
- Create: `crates/hig-core/src/repository.rs`
- Modify: `crates/hig-core/src/lib.rs`
- Test: `crates/hig-core/src/repository.rs`

Implement typed object IDs, checked envelopes, deterministic serialization,
zstd compression, immutable writes, verified reads, prefix resolution, and
repository configuration.

### Task 2: Atomic snapshots

Implement repository initialization, stable source reads, fixed-size Phase 1
chunks, file objects, recursive tree objects, commit objects, writer locking,
and HEAD-last atomic publication. Test no-op snapshots and one-byte changes.

### Task 3: History operations

Implement log traversal, tree flattening, added/deleted/modified/mode diff,
whole-tree and path restore, object verification, reachable-object traversal,
and dry-run/active GC. Test corrupted objects, missing references, restore
overwrite safety, and unreachable object removal.

### Task 4: CLI

**Files:**
- Modify: `crates/hig-cli/src/main.rs`

Add `hig repo init|snapshot|log|diff|restore|verify|gc` with stable JSON output
and concise human output.

## Phase 2: Micro History

Add versioned content-defined chunking, byte-range change records, automatic
watcher commits, path/rename indexes, compressed-tree indexes, and micro-range
restore. Verify one-character edits reuse unaffected chunks and can be located
without scanning all commits.

Implemented in v1.10.0 with FastCDC v2020, content-addressed cumulative path
history, unambiguous content-based rename detection, staged range restore,
compression-tree reports, and debounced repository watching.

## Phase 3: Semantic History

Add Tree-sitter language adapters, symbol identities, function/class/method
range objects, semantic change indexes, rename-aware lookup, function restore,
and IDE/MCP tools. Parser failures must degrade to byte/path history without
affecting restore correctness.

Implemented in v1.10.0 for Rust, Swift, JavaScript/JSX, TypeScript/TSX, and Python,
with content-addressed cumulative symbol history, exact symbol-byte restore,
and constrained MCP repository tools.

## Release Gates

- All repository objects independently verify against their IDs.
- Interrupted snapshots never move HEAD.
- Every commit restores byte-for-byte without walking a delta chain.
- GC preserves every object reachable from every ref.
- One-character and function-level changes are indexed and restorable after
  Phases 2 and 3.
- CLI, daemon/IDE integration, compatibility, security, and real-project
  performance tests pass before the three-phase goal is complete.
