# Hig v1.8.0 Profile

Date: 2026-06-19

Build: release, 8 workers. Benchmarks use independent CLI processes talking to the daemon over its Unix socket.

## Daemon Ownership

- Two concurrently submitted CLI jobs completed with different output paths.
- Daemon status after both jobs: `jobs_completed=2`, `cache_open_count=1`.
- Session jobs reported `kdf_ms=0` and `session_used=true`.
- Socket mode was verified as `0600`.
- Warm daemon reports `cache_index_open_us=0` and no shard deserialization per job.

## Source Dataset

- Input: 597,867 bytes, 47 files.
- Warm daemon full CLI median/p95/p99: 3.994 / 4.642 / 4.642 ms.
- zip median/p95/p99: 11.569 / 21.705 / 21.705 ms.
- Hig archive: 90,114 bytes; zip: 114,403 bytes; tar.gz: 101,328 bytes.
- Representative internal warm pack: 1,426 us.
- Representative critical stages: walk 168 us, manifest serialization 15 us, output write 471 us, flush 37 us, rename 57 us, response serialization 1 us, unattributed 4 us.

The remaining difference between the 1.4 ms internal pack and 4.0 ms full CLI median is process startup, argument parsing, socket framing, and report formatting. Cache index parsing is no longer on the warm path.

## Quality Gates

- 500 text files: warm daemon median 7.135 ms versus zip 15.429 ms; Hig 4,917 bytes versus zip 90,306 bytes.
- 4 MiB repeated text: Hig 710 bytes versus gzip-6 12,287 bytes; reduction exceeds 99%.
- 8 MiB random data: Hig 8,389,887 bytes, 0.015% expansion.
- 32 MiB sealed hit: 46 ms, 695.25 MiB/s, 32 sealed hits, one cache-pack range open.
- 32 MiB with a 4 KiB modification: 31 sealed hits and one sealed miss.

## Environment

The selected system temporary volume had more than 20 GiB free, but its 256 MiB native-copy median was 413.65 MiB/s. This is below the 650 MiB/s qualification threshold, so the benchmark correctly reports `ENVIRONMENT_NOT_QUALIFIED`. Functional and relative zip/gzip gates pass; absolute qualified-volume I/O claims are intentionally withheld.

## Hotspots

1. First-time 500-file cache population writes hundreds of path shards and is dominated by cache commit. Warm daemon operation is fast, but a future index generation should coalesce path metadata shards or append them transactionally.
2. Full CLI startup and protocol overhead is now larger than the internal warm pack for small source trees. Keeping a client process alive or adding a batch request would reduce this without changing archive security.
3. Output creation/write/flush/rename is the largest internal critical-path component on warm jobs. Further gains require qualified storage or carefully gated platform I/O primitives.

Compression and crypto do not dominate the warm critical path, so the C++ decision gate is not met.
