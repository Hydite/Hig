# HIG v1.10.1 Stable Engineering Release

HIG v1.10.1 is a patch release that publishes the qualified v1.10 runtime as
one coherent version across the native CLI, desktop application, MCP server,
and npm platform packages.

## Release Scope

- Includes the completed repository history and IDE automatic snapshot work.
- Includes the qualified cold-path, adaptive I/O, and payload-write changes.
- Includes Recovery Vault capture, verification, repair, and source-absent
  restore workflows.
- Publishes native npm payloads for macOS universal, Linux x86_64 GNU, and
  Windows x86_64 MSVC.
- Preserves the HIGV2 archive format, repository schema compatibility, and
  security defaults from v1.10.0.

## Installation

```bash
npm install --global @zorker/hig@1.10.1
hig --version
hig-mcp-server --smoke
```

Both commands must report `hig 1.10.1`.

## Compatibility

v1.10.1 does not rewrite or invalidate v1.10.0 archives, repositories, golden
fixtures, or Recovery Vault schema fixtures. Historical evidence remains
retained under `artifacts/docs/` with its original version identifiers.
