# HIG v1.10.0 Repository History Phase 3

Date: 2026-07-25

## Scope

Phase 3 adds parser-derived semantic history and IDE/MCP operations to the
byte-exact repository implemented in Phases 1 and 2.

Supported source families are Rust, Swift, JavaScript/JSX, TypeScript/TSX, and
Python. The adapters index functions, methods, classes, protocols, and
applicable Rust/Swift type constructs. Every symbol stores its language, kind, short and qualified
names, source path, exact byte range, content hash, structural hash, and a
domain-separated symbol identity.

## Correctness Boundary

Semantic objects are optional indexes referenced by commits. Commit, tree,
file, and chunk objects remain the only recovery source of truth. A parser
failure is recorded in `parser_failures`; it cannot prevent snapshot creation,
path restore, byte-range restore, or non-semantic history recovery.

Function restore resolves a symbol at the requested revision, then reads the
corresponding bytes from the verified file/chunk DAG. It never reconstructs
source from an AST and never normalizes whitespace or encoding.

## Identity and History

Symbol identity is BLAKE3 over language, semantic kind, and qualified name.
The structural hash excludes declaration-name bytes, allowing an unambiguous
name-only change to be indexed as a rename. Stable identities with changed
content are modifications; stable identities moving between paths are moves.

Each semantic index contains committed cumulative history. HEAD therefore
supports direct symbol lookup without scanning every commit or trusting a
mutable side database. Ambiguous short names fail and require a qualified name
or symbol ID.

The compression-tree report also exposes cache provenance when the repository
root is a Hig project. This is diagnostic metadata only: project/cache
generations identify the acceleration state around the snapshot, while the
committed content-addressed tree and semantic index remain the recovery source
of truth.

## Commands

```text
hig repo symbols [--revision <commit>] [--path <path>]
hig repo symbol-history --symbol <id-or-name>
hig repo restore-symbol --revision <commit> --symbol <id-or-name> -o <file>
```

All commands support JSON output. Symbol restoration is staged and refuses
overwrite by default.

## MCP

The v1.10.0 MCP server adds structured tools for repository initialization,
snapshot, log, byte diff, path history, whole/range/symbol restore, storage
tree, symbols, symbol history, verification, and dry-run/apply GC. It does not
expose arbitrary shell execution. Filesystem arguments remain constrained by
`HIG_MCP_ALLOWED_ROOTS`.

## Focused Verification

- Rust function modification and rename history passed;
- Rust method qualified-name lookup passed;
- old-revision function byte restore passed;
- JavaScript class/method and function lookup passed;
- TypeScript function lookup passed;
- Python class/method lookup passed;
- Swift type/method and top-level function lookup passed;
- Swift overloads receive distinct signature-aware symbol IDs;
- real Swift project one-byte function history and restore passed;
- semantic indexes are reachable and independently verified;
- CLI and MCP schemas compile and parse.
