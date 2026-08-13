# HIG Completion Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete HIG repository references, IDE snapshot policy, cold-path performance, cross-platform packages, and long-term compatibility without weakening correctness or security guarantees.

**Architecture:** Preserve HIGV2 and repository object compatibility while adding explicit repository reference namespaces, policy-driven workspace snapshots, measurable cold-path optimizations, native platform packaging, and a versioned migration/fixture suite. Every performance change must retain byte-for-byte recovery and atomic failure semantics.

**Tech Stack:** Rust 2024, Clap, serde, BLAKE3, FastCDC, zstd, notify, Tree-sitter, Node.js MCP stdio adapter, GitHub release artifacts.

---

## Delivery Tracks

1. Stabilize the CLI modularization and preserve command/help/MCP behavior.
2. Add explicit `HEAD`, `refs/heads/*`, and `refs/tags/*` semantics with atomic branch/tag operations and revision resolution.
3. Add workspace snapshot policy configuration for debounce, idle, periodic capture, queue limits, and resource-aware pause/resume.
4. Profile and optimize real-project cold packing across scan, hash, compression, payload staging, and output writing; publish before/after evidence.
5. Produce native Linux, Windows MSVC, and macOS CLI/MCP packages with platform smoke tests and checksums.
6. Add archive/repository migration commands, golden fixtures, cross-version matrix tests, corruption tests, and long-term compatibility gates.
7. Rebuild release artifacts, rerun the full benchmark matrix, and publish only claims supported by measured evidence.

## Non-Negotiable Invariants

- Archive and repository writes remain atomic and never replace a valid target after a failed operation.
- Unpack, restore, symbol restore, and byte-range restore remain exact and encoding-independent.
- Secure defaults remain password-based Argon2id plus ChaCha20-Poly1305; speed modes do not silently weaken secure mode.
- MCP filesystem roots remain constrained and arbitrary shell execution remains unavailable.
- Every reported performance improvement includes the same corpus, options, cache state, volume qualification, and correctness digest.
