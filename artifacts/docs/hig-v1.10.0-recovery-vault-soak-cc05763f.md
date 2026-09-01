# HIG v1.10.0 Native Recovery Vault Soak Evidence

Date: 2026-09-01

GitHub Actions run: `33498102964`

Run URL: <https://github.com/Hydite/Hig/actions/runs/33498102964>

Source commit: `cc05763fe3e13734327acb4cfc630776cde6884b`

## Scope and Acceptance Rule

This document records the release-qualified native Recovery Vault soak on
macOS arm64, Linux x86_64 GNU, and Windows x86_64 MSVC. Each platform built the
candidate CLI from source and executed at least 7,200 seconds of continuous
create, modify, rename, delete, automatic repository snapshot, mirrored Vault
capture, point verification, and exact-restore checkpoints through the MCP
watcher interface.

Acceptance required every report to declare `status: passed`, observe at least
7,200 seconds, recover two hard MCP restarts, pass capture, restore, and GC
interruption points, restore after complete workspace deletion, restore from a
mirror after primary-Vault deletion, finish with a healthy scrub and complete
audit chain, and retain zero unreachable or temporary objects after repeated
GC. The workflow validator and independent downloaded-artifact validation
passed every requirement on all three platforms.

## Native Soak Results

| Platform | Duration | Snapshots | Vault captures | Exact checkpoints | MCP restarts |
|---|---:|---:|---:|---:|---:|
| Linux x86_64 GNU | 7,272.845 s | 232 | 236 | 23 | 2 |
| macOS arm64 | 7,284.797 s | 222 | 226 | 22 | 2 |
| Windows x86_64 MSVC | 7,517.067 s | 212 | 216 | 21 | 2 |

The workloads performed 229/233/229/222 create, modify, rename, and delete
operations on Linux; 219/223/219/212 on macOS; and 209/213/209/202 on Windows.
Every checkpoint verified the primary and mirror recovery graph, restored into
a new destination, and compared the complete tree digest with the live
workspace.

## Interruption and Loss Results

All platforms passed the following destructive transitions:

- capture was killed after preparation and after object publication, then
  retried to a protected mirrored recovery point;
- restore was killed after preparation and staging without publishing a
  partial destination, then retried to exact bytes;
- GC was killed after preparation and pending-catalog publication, then
  retried exactly and repeated idempotently;
- the source workspace was recursively deleted and restored exactly from the
  primary Vault;
- the primary Vault was then recursively deleted and restored exactly from
  the surviving mirror;
- final scrub reported every location healthy and final audit reported no
  incomplete operation identifiers;
- repeated final GC reported zero candidate points, unreachable objects,
  temporary files, and temporary bytes.

The Linux run is also the regression proof for native notification-channel
recovery. Earlier candidate runs exposed Linux watcher loss at approximately
15 and 59 minutes. Commit `cc05763f` rebuilds a disconnected native backend and
forces authoritative repository reconciliation; the qualified Linux run then
completed 232 snapshots, 236 Vault captures, two MCP restarts, and all final
loss drills without exiting.

## Performance and Capacity Evidence

| Metric | Linux | macOS | Windows |
|---|---:|---:|---:|
| Watcher RPO median | 1,208.506 ms | 2,599.087 ms | 3,893.272 ms |
| Watcher RPO maximum | 3,318.281 ms | 6,814.690 ms | 13,333.794 ms |
| Object dedup reuse | 97.9028% | 97.7207% | 97.5546% |
| Storage write ratio | 0.988578 | 1.017682 | 1.022701 |
| Large restore throughput | 67.186 MiB/s | 29.290 MiB/s | 5.329 MiB/s |
| Harness peak RSS | 164,491,264 B | 182,681,600 B | 42,426,368 B |

All maximum RPO values remained below the 300-second release ceiling. The
separate qualified 1 GiB benchmark retains the absolute primary/mirror RTO,
peak CLI RSS, deduplication, and two-copy capacity evidence.

## Machine-Readable Provenance

| Report | SHA-256 | Final restored digest |
|---|---|---|
| `recovery-vault-soak-linux-x86_64-gnu.json` | `815953bbc2f8330c2fffc0e3e5cc5938ad9c65d88423a1f6cf438dd9ade206e9` | `d16d3c54079fa0f2e46348e86ff177089df9858176faeddd38d6ff8892bd3e4b` |
| `recovery-vault-soak-macos-aarch64.json` | `d2d221a33fa442493ce52ac7515bd77631c1d2fa0e8faeb59c7e5832406fef74` | `544e02c93cdbcbfb3d9da35822177dee113c1fcdb7c691358d7489985339e8b4` |
| `recovery-vault-soak-windows-x86_64-msvc.json` | `f98bb330aae959ad6e0c9531a05fb090491670009c3b37cf60666bfaa14c2ca6` | `2349df1a5681d9bed3c06db4f15dae5fe60f63a0cd6fe8bdbeca6aa3f62f09d9` |

The JSON artifacts are retained under the workflow run and were independently
validated with `scripts/validate-recovery-vault-soak-report.mjs` using the
exact source commit, `release` mode, and 7,200-second minimum. This evidence
closes the native multi-hour, interruption, source-loss, primary-loss,
throughput, memory, deduplication, capacity, and exact-restore release gates.
