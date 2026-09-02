# Hig CLI and IDE Tooling

## Engineering Status

Hig is at `v1.10.1` development. The CLI and Desktop App are usable for local engineering workflows:

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

The Linux IDE package is named `hig-v1.10.1-ide-mcp-linux-x86_64-gnu.tar.gz`.
It contains a Linux x86_64 GNU `bin/hig` and the same Node MCP adapter. The
validated runtime baseline is Ubuntu 24.04 x86_64 (glibc 2.39) with Node.js 18
or later.

## Common CLI Commands

```bash
hig --version
hig init /path/to/project --cache-dir /path/to/cache
hig project rebuild /path/to/project --wait
hig project status /path/to/project --json
hig project policy show /path/to/project --json
hig project policy set /path/to/project --quiescence-ms 15 --periodic-interval-ms 900000 --json

hig daemon start --cache-dir /path/to/cache
hig session unlock --cache-dir /path/to/cache --password "$HIG_PASSWORD"
hig pack /path/to/project -o /path/to/out.hig --cache-dir /path/to/cache --use-session --daemon required --project auto --json
hig inspect /path/to/out.hig --json
hig unpack /path/to/out.hig -d /path/to/restored --password "$HIG_PASSWORD"
hig migrate /path/to/legacy.hig -o /path/to/migrated.hig --password "$HIG_PASSWORD" --json
hig cache status --cache-dir /path/to/cache

hig repo init /path/to/project
hig repo snapshot /path/to/project -m "before refactor"
hig repo refs /path/to/project --json
hig repo migrate /path/to/project --json
hig repo branch list /path/to/project --json
hig repo branch create feature/refactor /path/to/project --from HEAD --json
hig repo branch switch feature/refactor /path/to/project --json
hig repo branch delete feature/refactor /path/to/project --json
hig repo tag create v1.10.1 /path/to/project --from HEAD --json
hig repo tag list /path/to/project --json
hig repo tag delete v1.10.1 /path/to/project --json
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

hig recovery init --vault-root /path/to/vault --mirror /path/to/mirror --json
hig recovery register /path/to/project --vault-root /path/to/vault --json
hig recovery capture /path/to/project --vault-root /path/to/vault --json
hig recovery list --vault-root /path/to/vault --json
hig recovery status --vault-root /path/to/vault --json
hig recovery audit --vault-root /path/to/vault --json
hig recovery migrate-auth --vault-root /path/to/legacy-vault --json
hig recovery auth export --vault-root /path/to/vault --output /offline/custody.json --json
hig recovery auth import --vault-root /path/to/vault --input /offline/custody.json --json
hig recovery auth rotate --vault-root /path/to/vault --json
hig recovery verify <repository-id> <recovery-point-id> --vault-root /path/to/vault --json
hig recovery restore <repository-id> <recovery-point-id> --vault-root /path/to/vault -d /path/to/restored --json
hig recovery scrub --vault-root /path/to/vault --json
hig recovery repair <repository-id> <recovery-point-id> --vault-root /path/to/vault --mirror /path/to/mirror --json
hig recovery promote --vault-root /path/to/survivor --mirror /path/to/replacement --json
hig recovery gc --vault-root /path/to/vault --json
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

Recovery Vault is an independent global durability layer. A completed capture
copies and verifies the complete immutable object graph outside the workspace.
It can restore after the source workspace is deleted; a verified mirror can be
promoted after primary Vault loss without the source. Recovery GC is report-only
unless `--apply` is explicit. See the
[operator runbook](../operations/recovery-vault-runbook.md) before configuring
production retention or disaster recovery.

Recovery authentication keys and monotonic checkpoints live outside the Vault
under `HIG_RECOVERY_AUTH_DIR` or the platform user default. Custody bundles
contain raw lineage keys and must be encrypted and controlled outside the
workspace, Vault, IDE, and source repository. Legacy unauthenticated Vaults
require the explicit, full-graph-verifying `migrate-auth` command. Rotation is
mirror-first, dual-authenticated, retryable, and retains old keys for offline
custody compatibility.

Repository references use the following model:

- New repositories have an active main branch selected by
  .hig/repository/HEAD and stored at refs/heads/main.
- refs/HEAD remains a direct, atomically updated compatibility view for older
  HIG CLI versions.
- Branches are mutable pointers; snapshots advance only the active branch.
- Tags are immutable pointers and duplicate tag creation is rejected.
- Revision aliases include HEAD, an unqualified branch or tag name,
  heads/<name>, tags/<name>, refs/heads/<name>, and refs/tags/<name>, in
  addition to full or unique 8+ character commit IDs.
- Legacy repositories containing only refs/HEAD remain readable and can
  continue recording snapshots.
- `hig repo migrate` upgrades a legacy repository in place, preserves every
  object ID, and is idempotent. A conflicting existing `refs/heads/main` is
  rejected before the selector changes.

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
| `hig_project_policy_show` | read automatic snapshot policy |
| `hig_project_policy_set` | atomically update automatic snapshot policy |
| `hig_daemon_start` | start daemon |
| `hig_daemon_status` | check daemon |
| `hig_daemon_stop` | stop daemon |
| `hig_session_unlock` | create in-memory session key |
| `hig_session_status` | check session |
| `hig_session_clear` | clear session |
| `hig_pack` | create archive |
| `hig_unpack` | restore archive |
| `hig_inspect` | inspect archive metadata |
| `hig_migrate` | verify and atomically migrate an archive to HIGV2 |
| `hig_cache_status` | check cache |
| `hig_cache_gc` | preview/run GC |
| `hig_cache_compact` | preview/run compaction |
| `hig_task_list` | list daemon tasks |
| `hig_task_status` | task status |
| `hig_task_cancel` | cancel task |
| `hig_task_result` | task result |
| `hig_repo_init` | initialize immutable repository history |
| `hig_repo_snapshot` | record an atomic byte/semantic snapshot |
| `hig_repo_refs` | list HEAD, branches, and tags |
| `hig_repo_migrate` | upgrade legacy refs to the explicit main branch model |
| `hig_repo_branch_list` | list branches |
| `hig_repo_branch_create` | create a branch at a revision |
| `hig_repo_branch_switch` | switch the active branch |
| `hig_repo_branch_delete` | delete an inactive branch |
| `hig_repo_tag_list` | list tags |
| `hig_repo_tag_create` | create an immutable tag |
| `hig_repo_tag_delete` | delete a tag |
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
| `hig_recovery_init` | initialize a Vault and independent mirrors |
| `hig_recovery_register` | bind stable repository identity and source history |
| `hig_recovery_capture` | verify and publish a complete recovery point |
| `hig_recovery_list` | discover repositories and points without a workspace |
| `hig_recovery_status` | report RPO, durability, mirror, and audit lag |
| `hig_recovery_promote` | promote a verified survivor and create replacement mirrors |
| `hig_recovery_audit` | validate mutation and interruption history |
| `hig_recovery_auth_rotate` | rotate authenticated lineage keys across every Vault copy |
| `hig_recovery_pin` | protect a recovery point from retention GC |
| `hig_recovery_unpin` | remove an explicit recovery pin |
| `hig_recovery_tombstone` | record deletion without removing recovery bytes |
| `hig_recovery_policy_show` | inspect versioned retention policy |
| `hig_recovery_policy_set` | update validated retention and quota limits |
| `hig_recovery_gc` | preview/apply protected mirror-first GC |
| `hig_recovery_scrub` | verify control data, refs, audit, and reachable objects |
| `hig_recovery_repair` | repair primary objects from a verified configured mirror |
| `hig_recovery_verify` | verify one published recovery graph |
| `hig_recovery_restore` | restore exact bytes with source absent |
| `hig_bench` | benchmark diagnostics |

## Security Defaults

- The adapter restricts both lexical and resolved physical paths to
  `HIG_MCP_ALLOWED_ROOTS`; it revalidates immediately before spawn and the HIG
  child independently enforces the physical roots. Symlink escapes and changed
  path identities are rejected.
- It does not expose arbitrary shell execution.
- It does not persist passwords.
- Destructive and overwrite flags require actual JSON booleans. GC remains
  report-only and restore remains no-overwrite unless explicit `true` is used.
- Recovery operations require `vaultRoot` unless
  `HIG_MCP_ALLOW_GLOBAL_RECOVERY=1` is explicitly configured.
- Custody export/import and legacy authentication migration are deliberately
  absent from MCP because they expose or establish root recovery authority.
- It supports session-based packing so IDE agents can avoid repeatedly passing passwords.
- `hig_bench` is intentionally exposed but should only be used when the user asks for benchmark work.
