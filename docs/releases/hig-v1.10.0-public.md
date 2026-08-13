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
- native macOS universal, Linux x86_64 GNU, and Windows x86_64 MSVC CLI/MCP packages.

## Platform Packages

| Package | Target | SHA-256 manifest |
| --- | --- | --- |
| `hig-v1.10.0-ide-mcp-macos-universal.tar.gz` | macOS arm64/x86_64 | `artifacts/hig-v1.10.0-ide-mcp-macos-universal.tar.gz.sha256` |
| `hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz` | Linux x86_64, glibc | `artifacts/hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz.sha256` |
| `hig-v1.10.0-ide-mcp-windows-x86_64-msvc.tar.gz` | Windows x86_64, MSVC | `artifacts/hig-v1.10.0-ide-mcp-windows-x86_64-msvc.tar.gz.sha256` |

All three packages were built natively with Rust 1.97.1 by GitHub Actions run
`31737147626` from commit `361e6ce2`. Each extracted package passed archive
round-trip, historical archive/repository migration, branch/tag/reference
workflows, automatic repository snapshots through MCP, path confinement,
repository verification, exact restore, and the 50-tool protocol contract.

## Public Boundary

The public release excludes IEEE paper sources, PDFs, figures, LaTeX template
materials, paper archives, local dependency trees, build outputs, cache data,
and private research artifacts. The repository's `.gitignore` encodes these
boundaries for future contributors.

## Verification

- `cargo fmt --all --check`: passed;
- workspace CLI/core tests: passed, including 159 `hig-core` tests and 18 CLI tests;
- CLI/core/FFI Clippy with warnings denied: passed;
- macOS universal, Linux x86_64 GNU, and Windows x86_64 MSVC native package QA: passed;
- MCP protocol negotiation, 50 closed tool schemas, IDE automatic snapshot,
  constrained repository operations, and path escape rejection: passed;
- package SHA-256 manifests: passed.

The packages and checksum manifests in `artifacts/` are the retained outputs
of the same successful native CI run.

The current enterprise-volume cold-path evidence is recorded in
`artifacts/hig-v1.10.0-post-migration-cold-benchmark.md`. It reports stage
timings and exact restore checks without making a cross-version speed claim.

## Known Scope

The desktop Tauri application remains a platform-specific distribution concern.
The public CLI/MCP package is the primary cross-IDE integration artifact.
Remote repository synchronization, branch merging, and Git interoperability are
outside this initial public release.
