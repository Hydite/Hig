# HIG v1.10.0 Native Package Provenance

- Source commit: `361e6ce208588a5870f3cd3ecde1df67d409ac5e`
- Branch: `Public`
- GitHub Actions run: `31737147626`
- Workflow: `HIG Quality Gates`
- Rust toolchain: `1.97.1`
- Node.js test runtime: `22`

## Retained Packages

| Target | SHA-256 |
| --- | --- |
| Linux x86_64 GNU | `bcb031521927687ee474d228caa4c18de8a575d92b95db25d298faa6a47c02bf` |
| macOS universal arm64/x86_64 | `cdf8070941daf22b5ad5618a682cdce2455ce65b1f855278d811df737bdf1c63` |
| Windows x86_64 MSVC | `e0b639010aedea0d0fb153c48b63168835f8afd3a85953c5dd03d4bc05e07048` |

## Native Gates

All four jobs completed successfully. The run covered 159 core tests, 18 CLI
tests, strict Clippy, format checks, runtime archive compatibility, immutable
golden archive and repository migration, object-hash preservation, migration
idempotence, package checksum verification, and extracted-package QA.

Each native package executed the 50-tool MCP protocol contract in one
persistent stdio session. The workflow included protocol-version negotiation,
archive round-trip, branch/tag/reference operations, IDE-managed automatic
repository snapshots, exact byte-range and full revision restore, repository
verification, and rejection of paths outside the configured root.
