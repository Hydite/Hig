# HIG v1.10.0 Native Repository Soak Evidence

Date: 2026-08-31

GitHub Actions run: `33351440350`

Run URL: <https://github.com/Hydite/Hig/actions/runs/33351440350>

Source commit at execution: `56e18c3d57484e2b91205b5d2c52a8c39786fa01`

Identity-rewritten equivalent: `7cc216864a1a377506c073ba8a5dc118d0847436`

## Scope and acceptance rule

This document records the release-qualified native repository and IDE
automatic-snapshot soak. The same source commit was built and exercised on
native macOS arm64, Linux x86_64 GNU, and Windows x86_64 MSVC GitHub-hosted
runners. Each job requested 7,200 seconds of mutation-driven operation and
then executed the interruption, garbage-collection, exact-restore, and full
repository-verification gates.

Acceptance required every platform report to declare `status: passed`, run
for at least 7,200 seconds, recover the MCP watcher at least twice, complete
one exact restore checkpoint for every recorded snapshot, preserve repository
state through interrupted snapshot, GC, and restore operations, and finish
with no unreachable or temporary objects. All requirements passed.

## Native soak results

| Platform | Duration | Snapshots | Exact restore checkpoints | MCP restart recoveries | Verified objects |
|---|---:|---:|---:|---:|---:|
| macOS arm64 | 7,232.744 s | 229 | 229 | 2 | 3,457 |
| Linux x86_64 GNU | 7,227.395 s | 237 | 237 | 2 | 3,553 |
| Windows x86_64 MSVC | 7,279.250 s | 234 | 234 | 2 | 3,517 |

The workload continuously exercised create, modify, rename, and delete
operations. Linux completed 934 such operations, macOS completed 892, and
Windows completed 922. Each snapshot checkpoint restored `HEAD` into a new
destination and compared its tree digest with the live workspace; a mismatch
would have aborted the harness and produced a failed report.

## Fault and interruption results

All three platforms passed the following destructive-path checks:

- snapshot interruption occurred after object publication and left `HEAD`
  unchanged;
- GC interruption occurred after deletion began, retained an unchanged
  `HEAD`, recovered temporary files, verified the repository, and was
  idempotent when repeated;
- restore interruption occurred after staging began, did not publish a
  partial destination, and left the repository verifiable.

The final real GC reported zero unreachable objects and zero temporary files
on every platform. Final repository verification checked 3,457 objects on
macOS, 3,553 on Linux, and 3,517 on Windows. The verified raw object totals
were 83,777,527, 86,119,767, and 85,232,111 bytes respectively.

## Machine-readable provenance

| Report | SHA-256 | Final workspace digest |
|---|---|---|
| `repository-soak-macos-aarch64.json` | `fd229589153bbcf5eb0b374329c03ede52150564cc4093f6732e11fc2814920e` | `7b8539286683a562bfee768ef2ec87e54bc475c4ecac78b1cf50454afb9b8e4d` |
| `repository-soak-linux-x86_64-gnu.json` | `c5ff4e068e0797051a44e87a14e0790fbe890a5fafdfa84b7a0175e0d5a81e9e` | `3f901951784425188e8b1f7ec015579480ad4363b3db56c5db6d2d9b995d8daa` |
| `repository-soak-windows-x86_64-msvc.json` | `0d4a8eb9aa50414e36b8d4b28ad5453c14ec7186d555cedb497f7c71bcd2513f` | `13944e3ccea6e971239c638a6ff0d6c00276b69e487f134478d7855599b76bd5` |

The JSON reports are retained as workflow artifacts under the run URL. Their
source commit, requested duration, observed duration, platform identity,
operation counts, interruption outcomes, final GC, final verification, and
workspace digest were independently asserted after download.

This evidence closes both the long-running repository fault-suite requirement
and the native three-platform IDE automatic-snapshot soak requirement in the
production completion matrix. The workflow executed against source commit
`56e18c3d`; the public-history identity correction maps that content-identical
commit to `7cc21686` without changing its tree, message, or timestamps.
