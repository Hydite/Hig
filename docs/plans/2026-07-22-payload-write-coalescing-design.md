# HIG v1.9.7 Payload Write Coalescing Design

## Status

Implemented and validated on 2026-07-22.

## Problem

After cold pipeline fusion, the reference IDE archive contained 1,101 memory
payloads. `ArchiveWriter` submitted every payload independently to a 32 MiB
`BufWriter`. The buffer already limited physical writes, but the path still
performed 1,101 writer calls, adaptive-I/O checks, timing updates, and buffer
state transitions for approximately 248 MB of payload data.

Copying payloads into another staging buffer would reduce calls but add a full
memory copy. Moving coalescing into archive preparation would couple the
optimization to manifest and block construction. The selected design keeps the
optimization inside `ArchiveWriter` and uses vectored I/O descriptors over the
existing payload allocations.

## Design

Consecutive `PayloadSource::Memory` values are processed as a memory run. Small
payloads are grouped into bounded batches and written through `IoSlice` and
`BufWriter::write_vectored`. No payload bytes are copied.

Normal-state limits are:

- 64 payload slices per batch;
- 8 MiB total payload bytes per batch.

When the shared adaptive-I/O controller is constrained, the writer silently
changes to:

- 16 payload slices per batch;
- 1 MiB total payload bytes per batch.

The limits are evaluated for every batch, so a task can move between normal and
constrained behavior while writing the same archive. Payloads at or above the
existing 8 MiB direct-write threshold flush the buffered prefix and retain the
existing direct-write path.

`write_vectored_once` measures the `BufWriter` buffer before and after each
operation. The adaptive controller receives only the bytes inferred to have
reached the underlying file, matching the scalar writer's telemetry semantics.
Partial vectored writes are advanced across slice boundaries until the complete
batch is written. A zero-progress write fails with `WriteZero`.

## Cached Payloads

Cached files and cached ranges preserve their existing validation, open-file
reuse, and prefetch pipeline. Consecutive memory payloads encountered between
cached payloads use the same coalescing helper. A cached payload therefore acts
as an ordering boundary but does not disable memory coalescing elsewhere in the
archive.

## Telemetry

Logical and physical concepts remain separate:

- `memory_payload_count` and `memory_payload_bytes` describe archive blocks;
- `buffered_write_count` describes writer batch submissions;
- `coalesced_write_count` describes vectored batches;
- `coalesced_payload_count` and `coalesced_bytes` describe their logical input.

The new fields are optional during deserialization so older JSON reports remain
readable.

## Safety

Archive offsets and the byte stream are unchanged. Header, manifest, payload
ordering, block IDs, encryption, preallocation, final flush, and atomic rename
semantics are unchanged. Any write or validation failure still drops the
temporary file and leaves an existing target archive untouched.

