# HIG Recovery Vault Completion Matrix

This matrix is the authoritative gate for Recovery Vault 100% completion. A
claim of completion is prohibited while any checkbox is open.

## Architecture and Contract

- [x] Accepted ADR defines authority, durability domains, RPO/RTO, and limits.
- [x] Threat/failure model covers deletion, corruption, interruption, capacity,
  concurrency, path reuse, and ransomware boundaries.
- [ ] Versioned on-disk schema, retention policy, and migration contract.
- [ ] Independent design and security review findings are resolved.

## Vault Foundation

- [ ] Platform global root and explicit `HIG_RECOVERY_VAULT` override.
- [ ] Stable registration identity and source-path history.
- [ ] Immutable reachable-object replication with destination verification.
- [ ] Atomic protected recovery refs and catalog generation.
- [ ] Source-absent list, verify, and exact restore.
- [ ] Idempotent configurable filesystem mirrors and durability status.

## Retention and Operations

- [ ] Tombstone event model for file, workspace, and registration deletion.
- [ ] Pin/unpin, minimum age/count, quota, and protected-ref semantics.
- [ ] Report-only default GC with interruption-safe apply mode.
- [ ] Full scrub, degraded-state reporting, mirror repair, and promotion.
- [ ] Owner-only permissions and actionable structured audit log.

## Filesystem Fidelity and Security

- [ ] Regular files, directories, symlinks, modes, and timestamps restored.
- [ ] Platform tests define ACL, xattr, hardlink, and sparse-file behavior.
- [ ] Destination confinement and hostile path/link tests pass.
- [ ] Encryption-at-rest and recoverable key-custody policy implemented or
  explicitly excluded from the first production profile.
- [ ] MCP authorization, path confinement, overwrite, and destructive-action
  policy passes adversarial tests.

## Product Surfaces

- [ ] CLI register/capture/list/status/restore/verify/scrub/repair/GC commands.
- [ ] MCP and IDE schemas expose equivalent noninteractive operations.
- [ ] Watcher captures to the vault and reports RPO/durability lag.
- [ ] JSON reports remain versioned and machine-actionable.
- [ ] Operator recovery runbook works when the original workspace is absent.

## Verification and Release

- [ ] Unit and integration suites pass with strict Clippy and formatting.
- [ ] Fault injection covers every capture/restore/GC publication transition.
- [ ] Corruption, disk-full, permission, offline mirror, and race suites pass.
- [ ] Whole-workspace and primary-vault loss drills restore exact digests.
- [ ] Multi-hour native macOS/Linux/Windows soak passes.
- [ ] RPO, RTO, throughput, memory, deduplication, and capacity evidence retained.
- [ ] v1.10 compatibility and immutable vault-schema fixture gates pass.
- [ ] Native CLI and MCP production packages pass extracted-package QA.
- [ ] Final release audit finds no unchecked item in this matrix.

