# Hig v1.5.0 Balanced Profile

Date: 2026-06-19

## Method

- Release build, 8 workers, system data volume.
- 20 independent cold-cache Balanced secure packs.
- 20 warm compressed-cache Balanced secure packs.
- 20 fair `zip -qr` runs from the same input root.
- `xctrace` Time Profiler trace captured at `/private/tmp/hig-v150-acceptance/time-profile.trace`.
- Non-overlapping critical-path timings are emitted directly by `PackReport`.

## Findings

The former 36 ms “ghost overhead” is resolved. `unattributed_ms` is 1 ms median and 2 ms p95 cold, and 1 ms median/p95 warm.

Largest remaining hotspots:

1. **Argon2id secure KDF: 13-17 ms.** It represents nearly the entire 16 ms warm pack. Parameters are unchanged from v1.4.2 (`19 MiB`, time cost 2, parallelism 1).
2. **Cold block preparation: 1-6 ms.** This includes source reads, zstd level 5 compression, and parameterized cache writes. It disappears on the warm compressed-cache path.
3. **Directory scan/content hashing: 0-4 ms.** It overlaps only a small part of KDF on this 296 KiB source dataset.

Warm-run representative critical path:

| Stage | Time |
|---|---:|
| setup | 0 ms |
| cache open | 0 ms |
| scan + KDF wall | 13-16 ms |
| plan | 0 ms |
| block prepare | 0 ms |
| cache commit | 0 ms |
| manifest build/protect | 0 ms |
| output write/rename | 0-1 ms |
| unattributed | 0-1 ms |

## Conclusion

There is no remaining 36 ms unclassified Balanced overhead. The requested small-directory target of <=1.2x zip is incompatible with an independent 13-17 ms memory-hard KDF when zip completes in about 7.55 ms and directory work exposes only 0-4 ms for overlap.

v1.5.0 therefore keeps security unchanged and remains not release-ready under the supplied hard gate. Meeting that gate requires an explicit architectural decision such as a trusted in-memory key service/session, OS credential integration, or redefining the comparison to separate password derivation from compression. Lowering Argon2id is intentionally rejected.
