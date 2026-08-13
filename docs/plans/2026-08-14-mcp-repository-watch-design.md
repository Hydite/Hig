# MCP Repository Watch Lifecycle Design

## Goal

Allow an IDE agent to start, inspect, and stop automatic HIG repository
snapshots without shell access while preserving repository atomicity and MCP
filesystem confinement.

## Architecture

The MCP adapter owns one `hig repo watch` child process per canonical repository
root. The Rust CLI remains the only component that observes filesystem events,
debounces changes, and publishes immutable repository snapshots. The adapter
only controls process lifecycle and parses the CLI's newline-delimited JSON
snapshot reports.

`hig_repo_watch_start` verifies that the repository exists within an allowed
root before spawning a watcher. Repeated start requests for an active root are
idempotent. `hig_repo_watch_status` reports active state, configuration, start
time, snapshot count, the most recent snapshot, and bounded diagnostics.
`hig_repo_watch_stop` is idempotent and waits for process termination, escalating
to forced termination only after a bounded grace period.

Watcher state is session-local and is never persisted by the protocol layer.
Closing stdin, receiving SIGINT/SIGTERM, or exiting the MCP process terminates
all managed watcher children. A watcher that exits unexpectedly remains visible
through status until it is restarted or stopped, including its exit code and
bounded stderr.

## Security And Failure Semantics

Every repository path passes through the existing allowed-root check. The new
tools expose fixed CLI arguments only and do not accept commands, environment
variables, or executable paths. Repository writes continue to use the Rust
repository lock and atomic publication path. A failed watcher start never
reports an active session, and adapter shutdown cannot intentionally leave a
managed watcher running.

## Verification

The persistent MCP integration test starts the watcher through MCP, mutates a
file, polls status until an automatic commit is observed, stops the watcher,
verifies the repository, and restores the automatically captured revision
through MCP for an exact byte comparison. The same test runs against every
extracted native package in CI.
