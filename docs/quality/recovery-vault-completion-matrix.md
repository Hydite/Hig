# HIG Recovery Vault Completion Matrix

This matrix is the authoritative gate for Recovery Vault 100% completion. A
claim of completion is prohibited while any checkbox is open.

## Architecture and Contract

- [x] Accepted ADR defines authority, durability domains, RPO/RTO, and limits.
- [x] Threat/failure model covers deletion, corruption, interruption, capacity,
  concurrency, path reuse, and ransomware boundaries.
- [x] Versioned on-disk schema, retention policy, and migration contract.
- [x] Independent design and security review findings are resolved.

## Vault Foundation

- [x] Platform global root and explicit `HIG_RECOVERY_VAULT` override.
- [x] Stable registration identity and source-path history.
- [x] Immutable reachable-object replication with destination verification.
- [x] Atomic protected recovery refs and catalog generation.
- [x] Source-absent list, verify, and exact restore.
- [x] Idempotent configurable filesystem mirrors and durability status.

## Retention and Operations

- [x] Tombstone event model for file, workspace, and registration deletion.
- [x] Pin/unpin, minimum age/count, quota, and protected-ref semantics.
- [x] Report-only default GC with interruption-safe apply mode.
- [x] Full scrub, degraded-state reporting, mirror repair, and promotion.
- [x] Owner-only permissions and actionable structured audit log.

## Filesystem Fidelity and Security

- [x] Regular files, directories, symlinks, modes, and timestamps restored.
- [x] Platform tests define ACL, xattr, hardlink, and sparse-file behavior.
- [x] Destination confinement and hostile path/link tests pass.
- [x] Encryption-at-rest and recoverable key-custody policy implemented or
  explicitly excluded from the first production profile.
- [x] MCP authorization, path confinement, overwrite, and destructive-action
  policy passes adversarial tests.

## Product Surfaces

- [x] CLI register/capture/list/status/restore/verify/scrub/repair/GC commands.
- [x] MCP and IDE schemas expose equivalent noninteractive operations.
- [x] Watcher captures to the vault and reports RPO/durability lag.
- [x] JSON reports remain versioned and machine-actionable.
- [x] Operator recovery runbook works when the original workspace is absent.

## Verification and Release

- [x] Unit and integration suites pass with strict Clippy and formatting.
- [x] Fault injection covers every capture/restore/GC publication transition.
- [x] Corruption, disk-full, permission, offline mirror, and race suites pass.
- [x] Whole-workspace and primary-vault loss drills restore exact digests.
- [x] Multi-hour native macOS/Linux/Windows soak passes.
- [x] RPO, RTO, throughput, memory, deduplication, and capacity evidence retained.
- [x] v1.10 compatibility and immutable vault-schema fixture gates pass.
- [x] Native CLI and MCP production packages pass extracted-package QA.
- [x] Final release audit finds no unchecked item in this matrix.

## Release Evidence

- Architecture and failure contract: `docs/adr/0007-recovery-vault.md`
- Schema and migration contract: `docs/compatibility/recovery-vault-schema.md`
- Source-absent operator drill: `docs/operations/recovery-vault-runbook.md`
- Security review: `artifacts/hig-v1.10.0-recovery-vault-security-review-cc05763f.md`
- Qualified 1 GiB benchmark: `artifacts/hig-v1.10.0-recovery-vault-qualified-cc05763f.md`
- Native two-hour soak: `artifacts/hig-v1.10.0-recovery-vault-soak-cc05763f.md`
- Complete Quality Gates: <https://github.com/Hydite/Hig/actions/runs/33498059314>
- Extended CodeQL: <https://github.com/Hydite/Hig/actions/runs/33498060294>
