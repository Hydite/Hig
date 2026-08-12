# HIG v1.10.0 Linux x86_64 GNU Release QA

## Target

- Package: `hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz`
- CLI: native Linux x86_64 GNU `hig` binary
- Adapter: Node MCP stdio server with constrained filesystem tools
- Validated host: Ubuntu 24.04 x86_64, glibc 2.39, Node.js 25.1.0
- Minimum adapter runtime: Node.js 18

## Build Provenance

The release binary is built natively from a source snapshot containing only
the Cargo workspace and `packages/hig-mcp-server`. It intentionally excludes
local build outputs, Git metadata, artifacts, benchmarks, caches, papers, and
user data. The source snapshot SHA-256 is recorded with the final build run.

The native build used Rust 1.97.1 (stable, 2026-07-14) on the Ubuntu 24.04
x86_64 host. The source snapshot transferred to the host had SHA-256
`4be7942ffecad7c3627fcaeafa9103f421a0288e7c18dbc50715c6045b47ab6e`.

## Required Checks

- `cargo fmt --all --check`
- `cargo test -p hig-cli`
- `cargo test -p hig-core`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --release -p hig-cli`
- archive pack/unpack byte-for-byte smoke test
- repository snapshot, byte-range restore, symbol restore, and `repo verify`
- extracted-package `bin/hig --version`
- extracted-package MCP `--smoke`, `initialize`, and constrained tools call
- `sha256sum -c` for the final tarball

## Status

Completed. Evidence:

- `cargo fmt --all --check`: pass;
- `cargo test -p hig-cli -p hig-core`: pass; `hig-core` 143 tests passed;
- `cargo clippy -p hig-cli -p hig-core -p hig-ffi --all-targets -- -D warnings`: pass;
- `cargo build --release -p hig-cli`: pass;
- archive pack/unpack byte comparison: pass;
- repository verify, symbol restore, commit diff, and byte-range restore: pass;
- extracted package `bin/hig --version`: `hig 1.10.0`;
- extracted package MCP smoke and JSON-RPC initialize/tools calls: pass;
- final package SHA-256: `5f2a239a87bd2a4af38e9e97f895516011b7e8f67c94964f3dbaeed79a56338f`.

The full workspace Clippy command is intentionally not a Linux CLI release
gate because it includes the optional Tauri desktop crate. On this host it
requires the unavailable `javascriptcoregtk-4.1` development package. The
CLI/core/FFI package scope passes Clippy with warnings denied.
