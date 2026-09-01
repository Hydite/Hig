# HIG v1.10.0 Cold-Path ABBA Evidence

Date: 2026-08-14

Platform: macOS arm64

Corpus: 17,583 files, 505,906,599 bytes
Corpus SHA-256 tree digest: `a30668137ff4c1606471e4fcd3d40de743ee921c1b583fdf1c0226018502b663`

## Purpose

Measure the current cold scan/hash, block preparation, output write, memory,
archive size, and exact restore behavior against retained HIG v1.9.6 and v1.9.7
native binaries. Every pack used an empty cache, disabled daemon and project
reuse, selected fastest mode, disabled encryption, and restored with the
current reader before the sample was accepted.

## Provenance

| Variant | Version | SHA-256 |
|---|---|---|
| v1.9.6 retained | `hig 1.9.6` | `b9e1e90372b79a29892f60b1c1ca60faa1d783918ad6cd162b516909f3b3910e` |
| v1.9.7 adaptive | `hig 1.9.7` | `90f8d113de85d67bff292952b8e39838b1322a24cb7bd942214851aba86afe9a` |
| current | `hig 1.10.0` | `b418eb207243a0c0c5b5452aea7099c4581e5aef5c01bc86e40e5a02e394bfd0` |

The v1.9.7 source snapshot is also recoverable from historical commit
`2c74c7e6cb6f5bfb6bccb584a7262ad59b7001ba`; its CLI and core package versions
are both `1.9.7`.

## Method

The order was counterbalanced as two paired ABBA groups:

```text
v1.9.6, current, current, v1.9.6
v1.9.7, current, current, v1.9.7
```

Each sample used a fresh cache and output location. The archive, cache, and
restored tree were deleted before the next sample. No sample was discarded.
The table reports conservative upper medians for even sample counts.

## Results

| Metric | v1.9.6 | v1.9.7 | Current | Current vs v1.9.7 |
|---|---:|---:|---:|---:|
| Total core duration | 5.009 s | 4.801 s | 4.554 s | 5.2% faster |
| Scan wall duration | 1.387 s | 1.475 s | 1.196 s | 18.9% faster |
| Block preparation | 2.554 s | 2.229 s | 1.852 s | 16.9% faster |
| Output write | 1.058 s | 1.506 s | 1.222 s | 18.8% faster |
| Archive bytes | 248,071,582 | 248,071,630 | 248,071,630 | unchanged |
| Peak pipeline memory | 247,620,147 | 289,563,187 | 795,469,786 | within 1 GiB declared budget |

All eight samples processed exactly 17,583 files and 505,906,599 bytes. Every
restored tree produced the source tree digest above. Current samples retained
505,906,599 source bytes through the fused cold pipeline, retained 247,620,147
payload bytes in memory, and wrote zero spool bytes.

## Gate Result

All product gates passed:

- current total duration was below 1.10x both historical baselines;
- current scan was below 1.10x v1.9.6 and below 0.95x v1.9.7;
- block preparation and output write were below 1.10x v1.9.7;
- peak pipeline memory remained below the declared 1 GiB release budget;
- archive size remained below 1.01x the larger historical archive;
- every restored file tree matched exactly.

The environment result is `ENVIRONMENT_NOT_QUALIFIED`. The system APFS volume
reported only 8.07 GB free at probe time, and its 256 MiB buffered-copy median
was 203.94 MiB/s with 205.49 MiB/s p95. The release qualification rules require
at least 20 GiB free, 650 MiB/s median, and 500 MiB/s p95. Therefore these data
prove the measured scan/hash improvement and no-regression behavior on this
host, but they are not the final qualified-NVMe release claim.

## Automated Policy

`scripts/cold-path-regression-test.mjs` is the shared executable gate. CI mode
creates a deterministic 3,004-file, 62,619,648-byte corpus and rejects total,
scan, block preparation, output write, peak-memory, archive-size, or exact
restore regression against `fixtures/performance/cold-path-ci-policy.json`.
Its built-in negative tests prove that each protected metric and a restore
digest mismatch are rejected. Release mode performs the historical ABBA method
above and can require a qualified volume with `--require-qualified`.
