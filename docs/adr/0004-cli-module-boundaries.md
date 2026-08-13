# ADR 0004: CLI Module Boundaries

## Status

Accepted on 2026-08-13.

## Context

The `hig-cli` binary originally kept command definitions, command execution,
daemon/session coordination, report rendering, repository commands, benchmark
workloads, and tests in one `main.rs` file. This made independent review and
platform packaging work unnecessarily risky.

## Decision

Keep `main.rs` as the process entrypoint only. Keep `cli.rs` as the command-line
schema, top-level dispatcher, and parser test boundary. Move implementation
code to modules with a single primary responsibility:

- `commands/archive.rs`: archive command argument schemas and pack, unpack,
  inspect, migrate, and benchmark execution;
- `commands/repository.rs`: HIG-native repository command execution and output;
- `runtime.rs`: project, daemon, session, cache, task, and daemon-backed pack
  coordination;
- `output.rs`: short, JSON, and verbose `PackReport` rendering;
- `benchmark.rs`: benchmark data model, corpus materialization, comparisons,
  report generation, volume probes, and benchmark tests.

Modules may depend on `hig-core` directly. Shared CLI-level policy helpers are
explicitly `pub(crate)` and must not expose new public library APIs.

## Invariants

- command names, options, defaults, and help text are unchanged;
- `.hig` archive format, repository object format, daemon protocol, and MCP
  tool contracts are unchanged;
- the CLI remains one binary with no runtime plugin loading;
- behavior is verified through parser tests, CLI/core tests, Clippy, command
  help comparison, and repository smoke coverage.

## Consequences

The entrypoint is intentionally inert, and the Clap model is grouped by command
domain. Cross-platform packaging invokes the unchanged binary and MCP adapter,
so this refactor has no release-package compatibility impact.
