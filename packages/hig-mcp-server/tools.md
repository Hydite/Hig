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
- `hig_repo_watch_start`: start session-managed automatic snapshots for an IDE workspace.
- `hig_repo_watch_status`: inspect watcher state and the latest automatic commit.
- `hig_repo_watch_stop`: stop the managed watcher; repeated stops are safe.
- `hig_repo_migrate`: upgrade legacy direct-HEAD refs to the explicit main branch model.
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

An IDE should start one managed watcher after repository initialization, check
status when it needs the latest automatic commit, and stop it before unloading
the workspace. The MCP server also terminates all managed watchers when its
stdio session closes.

## Recovery Vault

- `hig_recovery_init`: initialize an authenticated primary Vault and mirrors.
- `hig_recovery_register`: bind stable repository identity and source history.
- `hig_recovery_capture`: publish and verify an immutable recovery point.
- `hig_recovery_list`: discover repositories and points without the workspace.
- `hig_recovery_status`: inspect RPO, durability, mirror, and audit lag.
- `hig_recovery_promote`: promote a verified surviving mirror.
- `hig_recovery_audit`: verify event pairing and the authenticated audit chain.
- `hig_recovery_auth_rotate`: rotate the lineage key across mirrors and primary.
- `hig_recovery_pin` / `hig_recovery_unpin`: control retention protection.
- `hig_recovery_tombstone`: record deletion evidence without deleting bytes.
- `hig_recovery_policy_show` / `hig_recovery_policy_set`: manage retention.
- `hig_recovery_gc`: preview or apply protected mirror-first collection.
- `hig_recovery_scrub`: verify all configured Vault copies and reachable graphs.
- `hig_recovery_repair`: restore damaged primary objects from a verified mirror.
- `hig_recovery_verify`: verify one complete recovery graph.
- `hig_recovery_restore`: restore exact bytes while the source is absent.

Use an explicit `vaultRoot` for every operation. A point is media-loss protected
only when status reports `protected`, durability lag is zero, and scrub passes.
GC defaults to preview and restore defaults to no-overwrite. Key rotation keeps
old keys for offline custody compatibility and is retryable after interruption.

Custody export/import and `migrate-auth` are operator-only CLI commands and are
not MCP tools. Never send a custody bundle through an AI context. Configure
`HIG_MCP_ALLOWED_ROOTS` with only the required workspace, Vault, mirror, and
restore roots.

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
| `hig_repo_watch_start` | managed `hig repo watch <dir> --json` |
| `hig_repo_watch_status` | managed watcher status |
| `hig_repo_watch_stop` | stop managed watcher |
| `hig_repo_migrate` | `hig repo migrate <dir> --json` |
| `hig_repo_diff` | `hig repo diff <dir> --json` |
| `hig_repo_path_history` | `hig repo history <dir> --path <path> --json` |
| `hig_repo_restore_range` | `hig repo restore-range <dir> --path <path> --start <n> -o <file> --json` |
| `hig_repo_symbols` | `hig repo symbols <dir> --json` |
| `hig_repo_symbol_history` | `hig repo symbol-history <dir> --symbol <symbol> --json` |
| `hig_repo_restore_symbol` | `hig repo restore-symbol <dir> --symbol <symbol> -o <file> --json` |
| `hig_repo_verify` | `hig repo verify <dir> --json` |
| `hig_repo_gc` | `hig repo gc <dir> [--apply] --json` |
| `hig_recovery_init` | `hig recovery init --vault-root <vault> --json` |
| `hig_recovery_register` | `hig recovery register <dir> --vault-root <vault> --json` |
| `hig_recovery_capture` | `hig recovery capture <dir> --vault-root <vault> --json` |
| `hig_recovery_list` | `hig recovery list --vault-root <vault> --json` |
| `hig_recovery_status` | `hig recovery status --vault-root <vault> --json` |
| `hig_recovery_promote` | `hig recovery promote --vault-root <survivor> --mirror <replacement> --json` |
| `hig_recovery_audit` | `hig recovery audit --vault-root <vault> --json` |
| `hig_recovery_auth_rotate` | `hig recovery auth rotate --vault-root <vault> --json` |
| `hig_recovery_pin` | `hig recovery pin <repository-id> <point-id> --vault-root <vault> --json` |
| `hig_recovery_unpin` | `hig recovery unpin <repository-id> <point-id> --vault-root <vault> --json` |
| `hig_recovery_tombstone` | `hig recovery tombstone <repository-id> ... --vault-root <vault> --json` |
| `hig_recovery_policy_show` | `hig recovery policy show --vault-root <vault> --json` |
| `hig_recovery_policy_set` | `hig recovery policy set --vault-root <vault> ... --json` |
| `hig_recovery_gc` | `hig recovery gc --vault-root <vault> [--apply] --json` |
| `hig_recovery_scrub` | `hig recovery scrub --vault-root <vault> --json` |
| `hig_recovery_repair` | `hig recovery repair <repository-id> <point-id> --vault-root <vault> --mirror <mirror> --json` |
| `hig_recovery_verify` | `hig recovery verify <repository-id> <point-id> --vault-root <vault> --json` |
| `hig_recovery_restore` | `hig recovery restore <repository-id> <point-id> --vault-root <vault> -d <dir> --json` |
| `hig_bench` | `hig bench <dir> --json` |
