# HIG v1.10.0 Linux x86_64 GNU Release QA

## Target

- Package: `hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz`
- CLI: native Linux x86_64 GNU `hig` binary
- Adapter: Node MCP stdio server with constrained filesystem tools
- Validated host: GitHub-hosted Ubuntu 24.04 x86_64, glibc 2.39, Node.js 22
- Minimum adapter runtime: Node.js 18

## Build Provenance

The release binary was built natively by GitHub Actions run `31737147626`
from public source commit `361e6ce208588a5870f3cd3ecde1df67d409ac5e`
using Rust 1.97.1. The same run retained the package and checksum manifest.

## Required Checks

- `cargo fmt --all --check`
- `cargo test -p hig-cli`
- `cargo test -p hig-core`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --release -p hig-cli`
- archive pack/unpack byte-for-byte smoke test
- historical archive/repository migration and immutable object-hash checks
- repository branch/tag/refs, automatic snapshot, exact restore, and `repo verify`
- extracted-package `bin/hig --version`
- extracted-package MCP `--smoke`, protocol negotiation, 50-tool contract,
  automatic snapshot lifecycle, and path-confinement checks
- `sha256sum -c` for the final tarball

## Status

Completed. Evidence:

- `cargo fmt --all --check`: pass;
- `cargo test -p hig-cli -p hig-core`: pass; `hig-core` 159 tests and 18 CLI tests passed;
- `cargo clippy -p hig-cli -p hig-core -p hig-ffi --all-targets -- -D warnings`: pass;
- `cargo build --release -p hig-cli`: pass;
- archive pack/unpack byte comparison: pass;
- historical migration, repository verify, automatic snapshot, full restore,
  symbol restore, commit diff, and byte-range restore: pass;
- extracted package `bin/hig --version`: `hig 1.10.0`;
- extracted package MCP smoke and 50-tool persistent JSON-RPC workflow: pass;
- final package SHA-256: `bcb031521927687ee474d228caa4c18de8a575d92b95db25d298faa6a47c02bf`.

The full workspace Clippy command is intentionally not a Linux CLI release
gate because it includes the optional Tauri desktop crate. On this host it
requires the unavailable `javascriptcoregtk-4.1` development package. The
CLI/core/FFI package scope passes Clippy with warnings denied.
