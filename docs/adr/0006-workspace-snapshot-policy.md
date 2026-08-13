# ADR 0006: Versioned Workspace Snapshot Policy

## Status

Accepted

## Context

IDE workspaces need automatic snapshots that react quickly to saves while remaining bounded during generated-file bursts, watcher overflow, periodic maintenance, and host memory pressure. The watcher already supports incremental updates and recovery rebuilds, but its timing and queue limits were fixed in code and were not visible to IDE clients.

## Decision

ProjectConfig carries a versioned WorkspaceSnapshotPolicy containing an enable switch, a post-event quiescence window, a periodic full-rebuild interval, event and pending-file budgets, and an optional resource policy with pressure and resume thresholds.

The default policy preserves responsive save behavior, performs a periodic full rebuild every 15 minutes, bounds event and path queues, and uses hysteresis for available-memory pressure. Legacy project configurations omit the field and deserialize to this default.

The daemon watcher is the single policy executor for foreground and pack workflows. A queue overflow, watcher error, or rescan request invalidates the snapshot and forces a full rebuild when automatic policy is enabled. Memory pressure pauses automatic snapshot publication and exposes a structured pause reason; it never reports a stale snapshot as Ready. A disabled policy leaves changes Dirty or Invalid until an explicit rebuild.

## Configuration and Failure Semantics

project policy set validates the complete candidate policy, writes project.json atomically, and applies the same policy to the registered watcher. Invalid thresholds, unsupported policy schemas, and resource threshold inversions fail before state changes. Snapshot generations and event sequences are monotonic across full rebuilds.

## Consequences

IDE clients can tune responsiveness and resource use without changing archive format or repository history. Status responses expose policy schema, pause state, resource pressure, and the last observed memory value. The policy is independent from archive encryption and compression settings, so automatic snapshot behavior cannot weaken secure archive defaults.

