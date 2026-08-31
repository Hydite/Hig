# Repository Watcher Reconciliation Design

## Context

Native repository soak run `31806498790` completed its macOS and Windows
two-hour jobs, but Linux stopped after 100 automatic snapshots and 99 exact
restore checkpoints. The next workspace mutation produced no watcher snapshot
within five minutes. The watcher process remained reachable through MCP, and
no repository operation reported corruption. Increasing the soak timeout would
hide the missing native event without improving IDE reliability.

## Design

The repository watcher retains its native event and debounce path as the
primary mechanism. It also records the time of the most recent snapshot
attempt. A successful event-driven attempt resets a 60-second reconciliation
watchdog. If no attempt occurs before the watchdog expires, the watcher runs
the same idempotent `snapshot_repository` operation used by explicit and
event-driven snapshots.

This is not an independent writer or a second publication path. Reconciliation
uses the repository writer lock, immutable object verification, exact content
comparison, durability operations, and atomic reference update already
enforced by `snapshot_repository`. If the tree is unchanged, no commit is
created and the CLI emits no automatic-snapshot record. Continuous normal
events keep resetting the watchdog, so active healthy workspaces do not gain
an additional scan after every event.

## Safety and verification

The recovery path cannot publish partial state and does not change archive or
repository formats. It only makes the existing snapshot operation reachable
when a native filesystem backend silently drops an event. A regression test
modifies a repository before watcher registration, ensuring no corresponding
native event can be queued, then requires the bounded reconciliation interval
to discover the difference and create the exact child commit.

The existing CI repository smoke, core/CLI suites, strict Clippy, native MCP
integration, and two-hour Linux/macOS/Windows soak remain mandatory. The
release report persists the final GC and repository verification results so
the zero-unreachable-object and exact graph checks can be audited directly.
The production completion matrix stays open until all three reports pass
against the same source commit.
