# HIG v1.9.7 Cold Pipeline Fusion Design

## Status

Implemented and validated on 2026-07-22.

## Problem

The HIGV2 cold path compressed missing blocks in parallel, appended every
compressed object to `.hig-cache/object-packs/objects.pack`, and then entered a
second block-preparation pass. That pass called `get`, `get_batch`, or
`get_chunk` for the objects that had just been produced. Once the 128 MiB hot
payload cache was exhausted, the second pass reread those bytes from the object
pack. On the 17,583-file IDE corpus this caused 113,441,671 bytes of redundant
cache-pack reads.

The old implementation also collected all `WarmResult` values before cache
submission. Cache insertion copied up to another 128 MiB into the hot payload
cache, increasing transient memory pressure without helping the active pack.

## Design

`prewarm_compressed_cache` now returns a `WarmOutput` containing the existing
telemetry summary and a key-addressed set of newly compressed payloads. An
actual archive pack requests retained payloads. Project snapshot prewarming
does not retain payloads and preserves the existing hot-cache behavior.

Cold production and cache submission run as a bounded pipeline:

1. The existing scheduler orders missing plans.
2. An indexed Rayon parallel iterator reads raw data and compresses blocks.
3. A bounded crossbeam channel limits completed work waiting for submission to
   twice the active Rayon worker count.
4. A single cache-owning consumer appends objects sequentially and updates pack
   offsets and index records.
5. Newly compressed bytes are retained by object key and consumed directly by
   the existing encryption, payload staging, and manifest preparation loop.

`rayon::join` runs the indexed producer and sequential consumer concurrently.
This preserves the prior scheduler's buffer reuse behavior while allowing
cache writes to overlap compression. A single-worker pool uses a sequential
fallback to avoid a blocked producer/consumer pair.

Repeated references to the same object use a reference count. Intermediate
uses clone the compressed bytes in memory and the final use moves the original
allocation. This removes the remaining cache-pack fallback for duplicate
single or chunk objects. The measured duplicate payload was approximately
0.3 MiB in the reference corpus.

## Cache Semantics

Pipeline-specific cache insertion appends to the durable object pack and
updates the normal cache index, but does not copy the payload into the 128 MiB
hot cache. The active pack already owns that payload through `WarmOutput`.

Normal cache insertion APIs are unchanged. Project snapshot prewarming uses
those normal APIs because a later project pack, rather than the prewarm call
itself, consumes the object. Legacy block-file fallback and existing object
pack records remain readable.

## Safety

Archive format, block identifiers, compression levels, nonce selection,
encryption, manifest ordering, and atomic target replacement are unchanged.
Cache pack mutation remains single-owner and sequential, so object offsets
cannot race. In-flight compression failures can leave unreferenced bytes at the
end of the cache pack, but cannot replace or partially publish the target
archive. This is the same allowed cache-orphan model used by interrupted cache
appends.

## Acceptance Criteria

- Remove cold `cache-pack-read` bytes for newly compressed objects.
- Reduce median `block_prepare` by at least 15% on the IDE corpus.
- Do not regress total median pack time.
- Keep reported pipeline peak memory within baseline variance.
- Preserve hot-cache behavior for project snapshot prewarming.
- Pass full core and CLI tests, clippy, release build, unpack, and full-file
  SHA-256 comparison.

