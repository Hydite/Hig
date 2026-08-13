# HIG Production Completion Matrix

This matrix is the authoritative definition of 100% completion. Percentages
are descriptive only; the `Complete` state requires every listed gate.

## Repository History

- [x] Immutable commit/tree/file/chunk object graph and exact restore.
- [x] FastCDC byte-level reuse, range history, rename history, and semantic indexes.
- [x] Atomic HEAD, branch, tag, and revision resolution model.
- [x] Verification and reachability GC across every reference namespace.
- [x] Legacy direct-HEAD migration without object rewriting.
- [x] Golden legacy repository fixture passes verify, migrate, restore, and idempotence locally.
- [x] Golden legacy repository fixture passes the required CI gate.
- [x] Branch/tag/history/MCP workflow passes on macOS, Linux, and Windows packages.
- [ ] Long-running snapshot/restore/GC fault and interruption suite passes.

## IDE Automatic Snapshot

- [x] Versioned debounce, periodic rebuild, queue, and resource-pressure policy.
- [x] Watcher overflow recovery and monotonic generation/event sequence.
- [x] CLI and MCP policy/status control surface.
- [x] Deterministic event-burst, queue-overflow, pressure pause/resume, restart, periodic, and corruption suite.
- [ ] Multi-hour native macOS/Linux/Windows soak with exact repository verification.
- [x] IDE package integration verifies automatic snapshots without shell access.

## Cold Path Performance

- [x] Whole-file/chunk single-read reuse and cold pipeline fusion.
- [x] Runtime-adaptive full-pipeline I/O controller.
- [x] Adaptive payload memory and write coalescing.
- [x] Stage-level scan/hash/compression/write telemetry and exact restore checks.
- [ ] Same-host ABBA gate against retained v1.9.6 and v1.9.7 binaries on qualified NVMe.
- [ ] Current scan/hash hotspot receives a measured, no-regression improvement.
- [ ] CI benchmark policy rejects quality, memory, archive-size, or protected-stage regression.

## Native CLI and MCP Packages

- [x] Current macOS universal package and 50-tool MCP protocol smoke.
- [x] Previously released Linux x86_64 GNU package provenance.
- [x] Current-source native Linux x86_64 GNU package and extracted-package QA.
- [x] Current-source native Windows x86_64 MSVC package and extracted-package QA.
- [x] CI workflow generates checksums and retained artifacts for all native platforms.
- [x] The native package workflow has completed successfully on GitHub runners.
- [x] Native package MCP path-confinement and repository workflow pass on all platforms.

## Long-Term Compatibility

- [x] HIGV1 and HIGV2 legacy/compact readers and archive migration.
- [x] Legacy repository reference and cache-index/journal migrations.
- [x] Corruption, wrong-password, truncation, version, and atomic-publication tests.
- [x] Immutable v1.9.6 archive and v1.10.0 direct-HEAD repository golden fixtures.
- [x] Golden fixture SHA-256 manifests and current-reader migration gate implemented and passing locally.
- [x] Golden fixture gate has completed successfully in CI.
- [x] Native-platform compatibility matrix and MCP protocol-version gate.
- [x] Documented fixture addition policy for every future format/schema release.

## Release Rule

No track may be labelled 100% while any checkbox in that track is open. A new
format, repository schema, cache schema, or MCP protocol release must add a
fixture before release publication; replacing an older fixture is prohibited.
