# Hig v1.6.0 Profile

## Result

- Balanced secure without session: 17ms warm, including 15ms secure Argon2id.
- Balanced secure with session: 6.15ms median and 7.49ms p95 across 20 process executions.
- Zip: 9.08ms median and 9.96ms p95 on the same clean source tree.
- Session socket lookup: 0-1ms after replacing the 50ms polling interval.
- Warm L2 index: binary `index-v2`; process-local repeated opens report an L1 parsed-index hit.
- Warm cache commit: 0ms and zero dirty shards.
- Unattributed time: 0-2ms in observed release runs.

## Critical Fixes

1. Session key lookup is explicit through `--use-session`; a supplied password is never silently replaced by an existing session key.
2. The Unix socket is mode 0600, keys remain only in helper memory, and TTL/clear remove the socket.
3. Parsed cache indexes use a process-local L1 guarded by an on-disk signature.
4. L2 uses binary dirty shards while retaining legacy JSON read compatibility.
5. Balanced solid groups combine related source/text files in bounded 8MiB blocks.
6. Solid prewarm statistics distinguish real misses from later in-process cache reads.

## Environment

- Benchmark filesystem: `/private/tmp` on the system data volume.
- Free space: 38GiB during measurement.
- 32MiB native-copy median: 2,262.36 MiB/s.
- 256MiB native-copy median: 237.10 MiB/s.
- Qualification: `ENVIRONMENT_NOT_QUALIFIED` for absolute large sealed-I/O claims.

## Remaining Hotspots

1. No-session secure mode remains bounded by Argon2id, intentionally.
2. Cross-process L1 reuse requires the explicit session helper; parsed-index L1 is process-local.
3. Large sealed I/O must be remeasured on a volume meeting the 650 MiB/s native-copy gate.
