# HIG Native npm Distribution Design

Date: 2026-08-31

## Objective

Publish HIG v1.10.0 as a production npm installation without combining all
native binaries into one download or allowing an installation to silently run
a binary for the wrong platform. A global installation must expose both
`hig` and `hig-mcp-server`, retain the existing IDE package behavior, and
preserve the MCP path-confinement and process-lifecycle policies.

## Package topology

The public entry package is `@zorker/hig`. It contains JavaScript launchers,
the MCP adapter, documentation, and no native executable. It declares exact
v1.10.0 optional dependencies on:

- `@zorker/hig-darwin-universal` for macOS arm64 and x86_64;
- `@zorker/hig-linux-x64-gnu` for Linux x86_64 with glibc;
- `@zorker/hig-win32-x64-msvc` for Windows x86_64.

npm evaluates each native package's `os`, `cpu`, and, for Linux, `libc`
constraints. Unsupported packages remain optional and are not installed. The
main package therefore downloads one native payload while retaining one
cross-platform package name and version.

## Binary resolution

Both launchers use one resolver with a strict order:

1. the explicit `HIG_BIN` override;
2. a bundled `bin/hig` or `bin/hig.exe`, used by existing offline IDE packs;
3. the exact npm platform package for the current operating system and CPU.

Linux resolution also rejects a non-glibc runtime. An unsupported platform,
missing optional dependency, or missing executable produces an actionable
error. The resolver does not silently select an arbitrary executable from
`PATH`, preventing version drift and executable substitution.

## Build and publication

Release staging copies a previously built native CLI into a generated platform
package, creates the main package from tracked JavaScript sources, runs
`npm pack`, and emits machine-readable package metadata. Native CI performs
this process using the binary built on that runner. The verifier installs the
two resulting tarballs into a clean temporary prefix, executes `hig
--version`, runs the MCP smoke test, and runs the full constrained MCP
integration suite without `HIG_BIN` so that platform dependency resolution is
covered.

The three platform packages are published before `@zorker/hig`, all at the
same exact version. Public access is explicit. Registry credentials are
provided only through an ephemeral npm configuration and are never stored in
the repository, package tarballs, logs, or global npm configuration.
Publishing is restart-safe: an existing package is skipped only when its
registry integrity exactly matches the locally built tarball. The workflow
fails closed when a version exists with different content.

## Compatibility and failure safety

The current tar-based IDE distribution remains supported. Its package builder
copies the shared resolver and continues to bundle the native CLI next to the
MCP adapter. Existing `HIG_BIN` configurations remain valid. Publishing does
not change the HIG archive format, repository schema, cache schema, CLI
arguments, MCP schemas, or cryptographic behavior.
