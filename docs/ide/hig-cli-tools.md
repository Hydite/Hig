# Hig CLI and IDE Tooling

## Engineering Status

Hig is at `v1.10.0` development. The CLI and Desktop App are usable for local engineering workflows:

- HIGV2 archive creation and extraction.
- Project snapshot/index mode.
- daemon-backed tasks.
- session unlock for repeated secure operations.
- cache GC/compact.
- inspect and benchmark diagnostics.
- independent HIG repository history with atomic snapshots, FastCDC micro
  chunks, byte-range indexes, and exact restore.

The current CLI production binary is:

```text
target/release/hig
```

The macOS universal sidecar binary is:

```text
apps/hig-desktop/src-tauri/binaries/hig-universal-apple-darwin
```

The Linux IDE package is named `hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz`.
It contains a Linux x86_64 GNU `bin/hig` and the same Node MCP adapter. The
validated runtime baseline is Ubuntu 24.04 x86_64 (glibc 2.39) with Node.js 18
or later.

## Common CLI Commands

```bash
hig --version
hig init /path/to/project --cache-dir /path/to/cache
hig project rebuild /path/to/project --wait
hig project status /path/to/project --json

hig daemon start --cache-dir /path/to/cache
hig session unlock --cache-dir /path/to/cache --password "$HIG_PASSWORD"
hig pack /path/to/project -o /path/to/out.hig --cache-dir /path/to/cache --use-session --daemon required --project auto --json
hig inspect /path/to/out.hig --json
hig unpack /path/to/out.hig -d /path/to/restored --password "$HIG_PASSWORD"
hig cache status --cache-dir /path/to/cache

hig repo init /path/to/project
hig repo snapshot /path/to/project -m "before refactor"
hig repo log /path/to/project --json
hig repo diff /path/to/project --from <commit> --to HEAD --json
hig repo history /path/to/project --path src/lib.rs --json
hig repo restore /path/to/project --revision <commit> -d /path/to/restored
hig repo restore-range /path/to/project --revision <commit> --path src/lib.rs --start 120 --len 8 -o /path/to/range.bin
hig repo storage-tree /path/to/project --revision HEAD --json
hig repo symbols /path/to/project --revision HEAD --path src/lib.rs --json
hig repo symbol-history /path/to/project --symbol 'Thing::method' --json
hig repo restore-symbol /path/to/project --revision <commit> --symbol 'Thing::method' -o /path/to/method.rs
hig repo watch /path/to/project --debounce-ms 750 --json
hig repo verify /path/to/project
hig repo gc /path/to/project
```

`hig init` configures the mutable daemon/project acceleration snapshot. `hig
repo init` creates independent immutable history under `.hig/repository`.
Repository GC is report-only unless `--apply` is provided.

`repo history` reads the content-addressed path index committed by HEAD rather
than scanning the complete commit chain. `repo restore-range` uses byte offsets
and is encoding-independent. `repo storage-tree` additionally reports the
committed compression tree and, when available, its project/cache provenance.
Function and symbol lookup use the committed Phase 3 semantic index; restore
returns the exact source bytes from the file/chunk DAG.

## MCP Adapter

The IDE-facing protocol adapter lives in:

```text
packages/hig-mcp-server
```

It exposes Hig commands as MCP tools over stdio and calls a bundled or configured `hig` binary.

Recommended IDE config:

```json
{
  "mcpServers": {
    "hig": {
      "command": "node",
      "args": ["/absolute/path/to/hig-mcp-server/bin/hig-mcp-server.js"],
      "env": {
        "HIG_MCP_ALLOWED_ROOTS": "/absolute/path/to/workspace",
        "HIG_MCP_WORKDIR": "/absolute/path/to/workspace"
      }
    }
  }
}
```

On Linux, place the extracted package in a trusted local location such as
`/opt/hig-mcp-server` and substitute that absolute path in `args`. Retain
`HIG_MCP_ALLOWED_ROOTS`; the server never treats an IDE workspace as an
unrestricted shell boundary.

## Tool Mapping

| Tool | Purpose |
| --- | --- |
| `hig_version` | verify CLI version |
| `hig_help` | read command help |
| `hig_init_project` | initialize project metadata/index config |
| `hig_project_status` | read project status JSON |
| `hig_project_rebuild` | rebuild project snapshot/index |
| `hig_daemon_start` | start daemon |
| `hig_daemon_status` | check daemon |
| `hig_daemon_stop` | stop daemon |
| `hig_session_unlock` | create in-memory session key |
| `hig_session_status` | check session |
| `hig_session_clear` | clear session |
| `hig_pack` | create archive |
| `hig_unpack` | restore archive |
| `hig_inspect` | inspect archive metadata |
| `hig_cache_status` | check cache |
| `hig_cache_gc` | preview/run GC |
| `hig_cache_compact` | preview/run compaction |
| `hig_task_list` | list daemon tasks |
| `hig_task_status` | task status |
| `hig_task_cancel` | cancel task |
| `hig_task_result` | task result |
| `hig_repo_init` | initialize immutable repository history |
| `hig_repo_snapshot` | record an atomic byte/semantic snapshot |
| `hig_repo_diff` | inspect byte-range changes |
| `hig_repo_path_history` | query rename-aware path history |
| `hig_repo_restore` | restore a revision or path |
| `hig_repo_restore_range` | restore exact bytes |
| `hig_repo_storage_tree` | inspect chunk/storage tree |
| `hig_repo_symbols` | list semantic symbols |
| `hig_repo_symbol_history` | query function-level history |
| `hig_repo_restore_symbol` | restore historical function bytes |
| `hig_repo_verify` | verify reachable history objects |
| `hig_repo_gc` | preview/apply repository GC |
| `hig_bench` | benchmark diagnostics |

## Security Defaults

- The adapter restricts paths to `HIG_MCP_ALLOWED_ROOTS`.
- It does not expose arbitrary shell execution.
- It does not persist passwords.
- It supports session-based packing so IDE agents can avoid repeatedly passing passwords.
- `hig_bench` is intentionally exposed but should only be used when the user asks for benchmark work.
