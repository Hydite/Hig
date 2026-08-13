# HIG v1.10.0 Public Engineering Release

HIG v1.10.0 is the first public engineering release of the HIG archive,
repository-history, CLI, and IDE/MCP integration stack.

## Release Positioning

This release makes the implementation available for engineering evaluation,
format interoperability work, and IDE integration. It is deliberately
described as an engineering release: the archive format and recovery model are
documented, the core/CLI test suites pass, and platform-specific packages are
provided with checksums and validation evidence.

HIG's repository history is independent of Git. It provides HIG-native,
content-addressed history with exact byte recovery and higher-level indexes for
paths, renames, compression storage, and source symbols. It does not claim Git
wire compatibility, remote synchronization, branch merge semantics, or rebase.

## Included Capabilities

- HIGV2 archive creation, inspection, extraction, and authenticated encryption;
- Argon2id key derivation, ChaCha20-Poly1305 authentication, and BLAKE3 integrity;
- project-aware cache and daemon workflows;
- FastCDC content-defined chunks and byte-range change indexes;
- rename-aware path history and compression-tree storage introspection;
- Tree-sitter indexes for Rust, Swift, JavaScript/JSX, TypeScript/TSX, and Python;
- exact function, method, and class byte restoration;
- constrained MCP stdio tools for IDE agents;
- macOS universal and Linux x86_64 GNU CLI/MCP packages.

## Platform Packages

| Package | Target | SHA-256 manifest |
| --- | --- | --- |
| `hig-v1.10.0-ide-mcp-macos-universal.tar.gz` | macOS arm64/x86_64 | `artifacts/hig-v1.10.0-ide-mcp-macos-universal.tar.gz.sha256` |
| `hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz` | Linux x86_64, glibc | `artifacts/hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz.sha256` |

The Linux package was built natively on Ubuntu 24.04 x86_64 with Rust 1.97.1
and passed CLI/core/FFI tests, Clippy, archive round-trip, repository verify,
symbol restore, byte-range restore, and extracted MCP JSON-RPC checks.

## Public Boundary

The public release excludes IEEE paper sources, PDFs, figures, LaTeX template
materials, paper archives, local dependency trees, build outputs, cache data,
and private research artifacts. The repository's `.gitignore` encodes these
boundaries for future contributors.

## Verification

- `cargo fmt --all --check`: passed;
- workspace CLI/core tests: passed, including 153 `hig-core` tests and 18 CLI tests;
- CLI/core/FFI Clippy with warnings denied: passed;
- macOS universal CLI/MCP smoke: passed;
- Linux x86_64 native build and extracted package smoke: passed;
- MCP `initialize`, `hig_version`, and constrained repository verification:
  passed;
- package SHA-256 manifests: passed.

The macOS universal package was rebuilt from the post-release compatibility
migration commit and its current SHA-256 is recorded in the repository
checksum file. The Linux package listed above remains the previously validated
native Linux build; rebuilding it requires access to the Ubuntu x86_64 build
host and must be completed before publishing a package that includes the new
repository migration tools.

The current enterprise-volume cold-path evidence is recorded in
`artifacts/hig-v1.10.0-post-migration-cold-benchmark.md`. It reports stage
timings and exact restore checks without making a cross-version speed claim.

## Known Scope

The desktop Tauri application remains a platform-specific distribution concern.
The public CLI/MCP package is the primary cross-IDE integration artifact.
Remote repository synchronization, branch merging, and Git interoperability are
outside this initial public release.
