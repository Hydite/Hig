# ADR 0003: HIG Repository History

## Status

Accepted for HIG v1.10.0.

## Context

HIG needs independent repository history with finer recovery and indexing than
file-oriented version control. Existing project snapshots and cache indexes are
mutable acceleration state. They cannot serve as the source of truth for
history because cache maintenance may rewrite or remove them.

The history system must eventually support content-defined micro-chunks,
function-level semantic lookup, automatic change capture, compressed-tree
inspection, and exact byte recovery. Recovery must remain correct even when a
language parser is unavailable.

## Decision

HIG stores history in `.hig/repository` as a content-addressed object graph.
Objects are immutable, BLAKE3-addressed, independently zstd-compressed, and
verified before use. Commit objects reference a root tree and an optional
parent. Trees reference files and subtrees. Files reference ordered chunks.

Only refs are mutable. A snapshot writes and verifies every required object,
writes the commit object, and atomically replaces `refs/HEAD` last. A failed
snapshot may leave unreachable immutable objects, which are safe and removable
by reachability GC; it cannot publish a partial commit.

The cache remains an acceleration layer and may mirror compressed history
objects later, but repository correctness never depends on cache retention.

Object schemas include chunking and semantic-index versions. Phase 1 uses
deterministic fixed-size chunks. Phase 2 adds content-defined micro-chunks and
change indexes. Phase 3 attaches parser-derived semantic objects to commits.
Semantic objects locate code precisely but are never required to reconstruct
the original bytes.

## Consequences

- Historical commits remain readable across cache compaction and daemon state.
- Exact restore cost depends on reachable objects, not a linear delta chain.
- A one-byte change is always recoverable; Phase 2 reduces how much new data it
  stores around that change.
- Object storage can contain harmless unreachable objects after interruption.
- Repository GC must begin from every ref and traverse typed references.
- Object schema evolution requires explicit versioned decoders.

