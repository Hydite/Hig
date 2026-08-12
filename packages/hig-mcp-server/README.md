# Hig MCP Server

This package wraps the `hig` CLI as a local MCP stdio server so IDE agents can call archive, cache, byte-history, and function-history operations through constrained tools instead of hand-writing shell commands.

## CLI Build Artifact

The production CLI binary is:

```text
target/release/hig
```

The macOS universal desktop sidecar binary is:

```text
apps/hig-desktop/src-tauri/binaries/hig-universal-apple-darwin
```

The Linux x86_64 GNU IDE package bundles its platform-native CLI at the same
path, `bin/hig`. The v1.10.0 Linux package is validated on Ubuntu 24.04 x86_64
(glibc 2.39) and requires Node.js 18 or later to run the MCP adapter.

The IDE MCP production package bundles a copy at:

```text
bin/hig
```

Set `HIG_BIN=/absolute/path/to/hig` if you want the server to call an external Hig binary instead.

## Run

```bash
node /path/to/hig-mcp-server/bin/hig-mcp-server.js
```

Smoke test:

```bash
node /path/to/hig-mcp-server/bin/hig-mcp-server.js --smoke
```

## IDE Configuration

Use the bundled server command and point `HIG_MCP_ALLOWED_ROOTS` at the workspace the AI is allowed to operate on.

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

For multiple roots, separate paths with `:` on macOS/Linux and `;` on Windows. You can also use a comma-separated list.

Linux example:

```json
{
  "mcpServers": {
    "hig": {
      "command": "node",
      "args": ["/opt/hig-mcp-server/bin/hig-mcp-server.js"],
      "env": {
        "HIG_MCP_ALLOWED_ROOTS": "/workspace",
        "HIG_MCP_WORKDIR": "/workspace"
      }
    }
  }
}
```

## Security Notes

- By default, paths are restricted to the server working directory.
- Set `HIG_MCP_ALLOWED_ROOTS` for IDE use.
- Set `HIG_MCP_ALLOW_ANY_PATH=1` only in a trusted local environment.
- The adapter does not log passwords, but Hig CLI password commands still receive passwords as child-process arguments today. Prefer `hig_session_unlock` plus `hig_pack` with `useSession: true` for repeated operations.
- `hig_bench` can be long-running and write large temporary files.
- Repository GC is report-only unless an MCP caller explicitly sets `apply: true`.
- Symbol restore rejects ambiguous names; call `hig_repo_symbols` to select a qualified name or ID.

## Tools

See [tools.md](./tools.md).
