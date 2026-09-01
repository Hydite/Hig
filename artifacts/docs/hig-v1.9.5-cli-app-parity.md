# Hig v1.9.5 CLI-App Capability Parity

## Status

Hig Desktop now exposes every user-facing CLI capability either directly or through an equivalent visual workflow. Internal process commands and output formatting switches remain CLI-only by design.

| CLI surface | Desktop surface | Shared implementation | Status |
|---|---|---|---|
| `pack` basic policy | Create Archive | `DesktopPackRequest` -> `SerializablePackOptions` | Complete |
| HIGV1/HIGV2 and manifest format | Create Archive / Advanced / Archive | `ArchiveFormat`, `ManifestFormat` | Complete |
| level and workers | Create Archive / Advanced / Archive | zstd and Rayon options | Complete |
| cache enable/directory | Create Archive / Advanced / Caching | daemon cache binding | Complete |
| batch thresholds | Create Archive / Advanced / Batch | `BatchOptions` | Complete |
| chunk thresholds | Create Archive / Advanced / Chunks | `ChunkOptions` | Complete |
| solid mode | Create Archive / Advanced / Caching | `SolidMode` | Complete |
| KDF profile | Create Archive / Advanced / Caching | secure or interactive | Complete |
| project auto/off/required | Create Archive / Advanced / Caching | `ProjectMode` | Complete |
| metadata trust | Create Archive / Advanced / Caching | explicit warning and opt-in | Complete |
| `unpack --overwrite` | Open Archive / Extract | daemon unpack task | Complete |
| archive inspection | Open Archive and `hig inspect` | `hig_core::inspect_archive` | Complete |
| `init --exclude --cache-dir` | Projects / Initialize | `init_project` | Complete |
| `watch` | Projects diagnostics | daemon-owned watcher; no second foreground writer | Equivalent |
| project status | Projects details | daemon project status | Complete |
| project rebuild | Projects -> Tasks | `TaskRequest::ProjectRebuild` | Complete |
| session lifecycle | sidebar and Runtime | daemon in-memory session | Complete |
| daemon lifecycle | Runtime | status/start/restart/stop | Complete |
| cache status | Cache | daemon cache status | Complete |
| cache GC/compact | Cache -> Tasks | preview plus async maintenance task | Complete |
| task list/status/result/cancel | Tasks | daemon is source of truth across known cache dirs | Complete |
| benchmark compare | Diagnostics | constrained `hig bench --json` sidecar | Complete |

## Deliberate CLI-only Capabilities

- `daemon serve` is an internal sidecar entrypoint.
- `--json`, `--verbose`, and `--quiet` are automation/output contracts rather than product controls.
- Foreground `watch` is a terminal diagnostic; Desktop displays the same watcher state without starting a competing watcher.
- `fast-bench` is restricted to Diagnostics and cannot be selected for normal archives or sessions.

## Security Invariants

- Default archive mode remains HIGV2, compact manifest, balanced speed, secure Argon2id, password AEAD, cache enabled, and metadata trust disabled.
- Passwords are cleared after request submission and never enter settings, task history, reports, or command-line arguments for Desktop benchmarks.
- Daemon tasks are identified by cache binding and task id. A task from one cache cannot be controlled through another cache.
- Cache maintenance and project rebuild share the daemon task queue with pack operations.
- No archive format, compression policy, cache format, or cryptographic primitive changed in v1.9.5.

