# Native Repository Soak And Interruption Design

## Goal

Prove that IDE-managed HIG repository history remains exact across sustained
filesystem changes, MCP process restarts, snapshot/restore/GC interruption,
and native operating-system differences. The gate must use public CLI and MCP
interfaces and must not weaken repository locking, immutable objects, atomic
reference publication, path confinement, or restore safety.

## Watcher Recovery

`hig repo watch` subscribes to the native filesystem watcher before running an
idempotent catch-up snapshot. This ordering closes both restart gaps: changes
made while no watcher exists are found by the catch-up scan, while changes made
during the scan are already queued by the watcher. Catch-up publishes only when
content changed and uses the same writer lock, object verification, durability,
and atomic reference path as an explicit snapshot.

The MCP adapter starts repository watchers with a dedicated stdin lifecycle
pipe. The CLI monitors that pipe and exits after EOF. Normal MCP stop still
requests graceful child termination, while abrupt adapter death closes the pipe
at the operating-system boundary and prevents an orphan watcher. No arbitrary
process or shell control is added to MCP.

## Soak Harness

The cross-platform Node harness creates a synthetic repository, starts the MCP
server, and continuously applies deterministic create, modify, rename, and
delete operations. Every published snapshot is verified, restored into a fresh
destination, and compared by relative path, file type, size, and SHA-256. At
fixed intervals the harness terminates MCP, changes files while offline,
restarts MCP, and requires catch-up to publish the missing state.

The interruption phase kills a real snapshot only after new immutable objects
appear, verifies that HEAD did not advance, and runs repository verification.
It then interrupts a real GC after deletion begins, proves that HEAD and all
reachable objects remain valid, and requires a recovery GC plus an idempotent
repeat to remove all unreachable and temporary objects. It also interrupts a
full restore after staging begins and proves that the requested destination was
never partially published.

## Gates And Evidence

The same harness has a bounded CI mode and a release soak mode. Release evidence
requires at least two hours on native macOS, Linux, and Windows jobs, exact
restore after every checkpoint, at least two MCP restart recoveries, exercised
snapshot and restore interruption, final repository verification, and clean
idempotent GC. Each job uploads a JSON report containing source commit, platform,
duration, operation counts, commit IDs, interruption evidence, and final digest.
