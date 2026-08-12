# Hig MCP Tool Instructions

Use these tools when an IDE agent needs to operate Hig without constructing raw shell commands.

## Project and Index

- `hig_init_project`: create Hig project metadata and excludes.
- `hig_project_status`: inspect project snapshot status as JSON.
- `hig_project_rebuild`: rebuild the project snapshot/index, optionally waiting for completion.

Typical sequence:

```text
hig_init_project(dir)
hig_project_rebuild(dir, wait=true)
hig_project_status(dir)
```

## Runtime

- `hig_daemon_start`: start daemon for a cache directory.
- `hig_daemon_status`: check daemon state.
- `hig_daemon_stop`: stop daemon.
- `hig_session_unlock`: derive and store an in-memory session key.
- `hig_session_status`: check session state.
- `hig_session_clear`: clear session.

Recommended secure workflow:

```text
hig_daemon_start(cacheDir)
hig_session_unlock(cacheDir, password, ttlSecs=1800, kdfProfile="secure")
hig_pack(inputDir, output, cacheDir, useSession=true, daemon="required", project="auto")
hig_session_clear(cacheDir)
```

## Archive

- `hig_pack`: create `.hig` archive.
- `hig_unpack`: restore `.hig` archive.
- `hig_inspect`: inspect archive metadata, JSON by default.

Safe defaults:

```json
{
  "encryption": "password",
  "format": "higv2",
  "manifestFormat": "compact",
  "speed": "balanced",
  "daemon": "auto",
  "project": "auto"
}
```

Fast repeated local archives:

```json
{
  "speed": "fastest",
  "daemon": "required",
  "project": "required",
  "useSession": true
}
```

Only use `speed: "fastest"` when the user accepts metadata-trust and sealed-cache equality risks.

## Cache

- `hig_cache_status`: view cache state.
- `hig_cache_gc`: preview or run garbage collection. Defaults to dry-run.
- `hig_cache_compact`: preview or run compaction. Defaults to dry-run.

Always call dry-run first before destructive cache maintenance:

```text
hig_cache_gc(cacheDir, dryRun=true)
hig_cache_gc(cacheDir, dryRun=false)
```

## Tasks

- `hig_task_list`
- `hig_task_status`
- `hig_task_cancel`
- `hig_task_result`

Use these with daemon-backed operations.

## Repository History

- `hig_repo_init`: initialize independent immutable history.
- `hig_repo_snapshot`: atomically record a version with byte and semantic indexes.
- `hig_repo_log`: list commits.
- `hig_repo_diff`: inspect file and exact byte-range changes.
- `hig_repo_path_history`: query rename-aware path history.
- `hig_repo_restore`: restore a revision or path.
- `hig_repo_restore_range`: restore an exact byte range.
- `hig_repo_storage_tree`: inspect chunk reuse and stored object sizes.
- `hig_repo_symbols`: list functions, methods, classes, and Rust type symbols.
- `hig_repo_symbol_history`: query function-level history.
- `hig_repo_restore_symbol`: restore exact historical symbol bytes.
- `hig_repo_verify`: verify every reachable object.
- `hig_repo_gc`: preview or apply unreachable-object deletion.

For function recovery, list symbols first when the short name may be
ambiguous, inspect history, then restore from an explicit revision:

```text
hig_repo_symbols(dir, revision="HEAD", path="src/lib.rs")
hig_repo_symbol_history(dir, symbol="Thing::method")
hig_repo_restore_symbol(dir, revision="<commit>", symbol="Thing::method", output="/workspace/recovered.rs")
```

Repository GC defaults to preview. Set `apply=true` only after reviewing the
dry-run result.

## Benchmark

- `hig_bench`: run Hig benchmark and optional zip/tar comparisons.

Use benchmark tools only when the user asks for performance validation. They can create large temporary files and run for minutes.

## CLI Equivalents

| MCP tool | CLI equivalent |
| --- | --- |
| `hig_version` | `hig --version` |
| `hig_help` | `hig <command> --help` |
| `hig_init_project` | `hig init <dir>` |
| `hig_project_status` | `hig project status <dir> --json` |
| `hig_project_rebuild` | `hig project rebuild <dir> [--wait]` |
| `hig_daemon_start` | `hig daemon start` |
| `hig_daemon_status` | `hig daemon status` |
| `hig_session_unlock` | `hig session unlock` |
| `hig_pack` | `hig pack <dir> -o <archive> --json` |
| `hig_unpack` | `hig unpack <archive> -d <dir>` |
| `hig_inspect` | `hig inspect <archive> --json` |
| `hig_cache_status` | `hig cache status` |
| `hig_cache_gc` | `hig cache gc` |
| `hig_cache_compact` | `hig cache compact` |
| `hig_repo_init` | `hig repo init <dir> --json` |
| `hig_repo_snapshot` | `hig repo snapshot <dir> --json` |
| `hig_repo_diff` | `hig repo diff <dir> --json` |
| `hig_repo_path_history` | `hig repo history <dir> --path <path> --json` |
| `hig_repo_restore_range` | `hig repo restore-range <dir> --path <path> --start <n> -o <file> --json` |
| `hig_repo_symbols` | `hig repo symbols <dir> --json` |
| `hig_repo_symbol_history` | `hig repo symbol-history <dir> --symbol <symbol> --json` |
| `hig_repo_restore_symbol` | `hig repo restore-symbol <dir> --symbol <symbol> -o <file> --json` |
| `hig_repo_verify` | `hig repo verify <dir> --json` |
| `hig_repo_gc` | `hig repo gc <dir> [--apply] --json` |
| `hig_bench` | `hig bench <dir> --json` |
