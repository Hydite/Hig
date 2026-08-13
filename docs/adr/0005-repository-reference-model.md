# ADR 0005: Repository Reference Model

## Status

Accepted

## Context

HIG repository history stores immutable content-addressed commit objects and a direct refs/HEAD compatibility pointer. Explicit branches and tags are needed for IDE workflows without rewriting existing objects or invalidating old repositories.

## Decision

New repositories use .hig/repository/HEAD as the active branch selector, refs/heads/<name> as mutable branch pointers, and refs/tags/<name> as immutable tag pointers. refs/HEAD remains a direct commit-ID compatibility view for older HIG versions.

Revision resolution supports HEAD, unqualified branch and tag names, heads/<name>, tags/<name>, refs/heads/<name>, refs/tags/<name>, and full or unique 8-or-more-character commit IDs. Legacy repositories containing only refs/HEAD remain linear and continue to support all existing operations.

## Atomicity and Security

All reference mutations acquire the repository writer lock and use temporary file write, fsync, rename, and parent-directory synchronization. Names reject empty or traversal components, NUL bytes, unsafe separators, and non-ASCII punctuation. Revision aliases are validated before path construction.

Verification and GC traverse every reachable branch, tag, and compatibility reference. The immutable object graph is never modified by reference changes.

## Consequences

Snapshots advance only the selected branch. An unqualified name that exists as both a branch and a tag is rejected as ambiguous; explicit namespaces remain available. Older clients see the coherent direct refs/HEAD view.

