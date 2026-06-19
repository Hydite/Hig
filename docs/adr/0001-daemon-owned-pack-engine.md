# ADR 0001: Daemon-Owned Pack Engine

- Status: Accepted
- Date: 2026-06-19
- Version: Hig v1.8.0

## Context

Before v1.8, the daemon retained session material but each CLI invocation still opened and decoded the cache index, initialized execution resources, and ran the pack operation in the client process. This left fixed process and cache costs on every warm archive operation.

## Decision

The v1.8 daemon owns a long-lived `PackEngine`. The engine opens `CacheStore` once and retains its Rayon pool, buffer pool, scheduler state, metadata cache boundary, and in-memory session key. Independent CLI processes submit framed `PackJobRequest` messages over a user-only Unix socket. The daemon performs scan, planning, compression, encryption, cache commit, and atomic archive output, then returns only a serializable report.

The protocol uses a 32-bit little-endian frame length, bincode payloads, a 1 MiB control-frame limit, request IDs, protocol version checks, socket mode `0600`, and peer UID validation. Passwords never cross the socket. A derived key may be installed for a TTL-bound session or supplied as job-scoped key material; temporary key buffers are zeroized.

One daemon holds the advisory write lock for each cache directory. Cache index changes and pack append operations remain single-writer. Archive output uses a same-volume temporary file and atomic rename.

## Cache Lifecycle

Cache maintenance is daemon-owned. GC applies the configured disk budget and removes expendable compressed objects. Sealed pack compaction writes a new generation, validates payload lengths, syncs the new pack and index, atomically switches generation metadata, and only then removes old pack files. Dry-run commands do not modify cache state.

## Compatibility

The archive format remains HIGV2. Unpack remains standalone and continues to read HIGV1 and prior HIGV2 archives. The daemon wire protocol is versioned independently and does not promise compatibility with the v1.7 session socket.

## Consequences

- Warm jobs avoid cache index parsing and repeated worker-pool initialization.
- Session keys remain inside the daemon during pack.
- The daemon is now part of the cache consistency boundary and must fail closed on protocol, credential, lock, or cache corruption errors.
- Standalone mode remains available, but it may only write a cache when the daemon lock is not held.
- C++ remains unnecessary until profiling shows compression and crypto dominate the critical path and a native proof of concept demonstrates a material gain.
