# Hig v1.9.7 Payload Memory Strategy

Date: 2026-07-02

## Scope

This stage reduces the memory retained by prepared compressed/encrypted block payloads before final archive writing.

The `.hig` archive format, manifest schema, block order, block identifiers, encryption format, and unpack compatibility are unchanged.

## Design

The final manifest must contain each block's final identifier, size, and archive offset. Therefore, compressed blocks cannot be written directly to the final archive before block preparation and manifest construction complete.

The implementation introduces a process-local `PayloadStager`:

1. Prepared payloads remain in memory while the payload memory budget is available.
2. Payloads beyond that budget are appended to a temporary spool file next to the final output.
3. Each spilled payload becomes a `PayloadSource::CachedRange`.
4. Before final archive writing, the spool writer is flushed and closed.
5. `ArchiveWriter` reads the consecutive spool ranges with one open file and bounded prefetch.
6. The spool file is removed by `Drop` after success or failure.

The default payload memory budget is half of the pipeline memory budget:

- Pipeline budget: 128 MiB
- Payload memory budget: 64 MiB

Password-mode blocks are now encrypted when each block is prepared. This allows ciphertext to be staged immediately instead of retaining every compressed plaintext until a final parallel encryption pass.

## Failure Safety

The existing atomic replacement model remains in effect:

- payload spool: temporary sidecar in the output directory
- archive output: separate temporary archive in the output directory
- existing destination: unchanged until final rename
- staging/write failure: temporary spool and archive are removed
- successful pack: temporary archive is atomically renamed to the destination

## Benchmark Corpus

- Files: 492
- Input size: 292 MB
- Archive payload: approximately 164 MB
- Prepared payloads: 396

Final unencrypted telemetry:

| Metric | Result |
|---|---:|
| Payload memory bytes | 67,108,856 |
| Spool payloads | 174 |
| Spool bytes | 96,873,336 |
| Estimated concurrent pipeline peak | 125,829,112 |
| Spool open count during final write | 1 |
| Writer strategy | `PrefetchedCachedFiles` |

Previous payload-only memory was approximately 163,982,192 bytes. The staged strategy reduces retained payload memory by about 59%.

The final peak telemetry now adds concurrent payload and writer/prefetch buffers instead of taking only their maximum. This is intentionally more conservative and more representative than the previous estimate.

## Password Mode

Password-mode validation produced:

- Payload memory: 67,108,861 bytes
- Spool payloads: 167
- Spool bytes: 96,879,667
- Block/manifest crypto telemetry: 268,341 us

The generated password archive was unpacked by both:

- current v1.9.7 CLI
- v1.9.6 release CLI

Both restored trees matched the source digest:

`b1ce0d580ec41f7aa52ccbdf3a92a6668641e4d0226feb23a503ceb787de5734`

## Performance Trade-off

Spooling adds one sequential write and one sequential read for spilled bytes. It primarily targets predictable memory use and avoids retaining the entire archive payload in RAM.

Absolute write-time comparisons from this run are not suitable as a release gate because:

- the system temporary volume reached `ENOSPC` during an earlier benchmark attempt,
- the `/Volumes/Build` write latency varied substantially between paired runs,
- spool staging intentionally performs additional I/O.

Future work should overlap spool production with compression and use a qualified benchmark volume before tuning spool thresholds.

## Verification

- `cargo check -p hig-core -p hig-cli`: passed
- `cargo test -p hig-core`: 100 passed
- `cargo test -p hig-cli`: 10 passed
- payload budget/order/cleanup unit test: passed
- direct-write prefix-order test: passed
- existing-target preservation test: passed
- release pack/unpack smoke: passed
- v1.9.6 cross-version unpack digest: passed
- spool sidecar cleanup check: passed
