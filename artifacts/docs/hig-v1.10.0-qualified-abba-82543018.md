# HIG v1.10.0 Qualified Cold-Path ABBA Evidence

Date: 2026-08-14

GitHub Actions run: `31805607941`

Run URL: <https://github.com/Hydite/Hig/actions/runs/31805607941>

Source commit at execution: `82543018e9baa4d5850835dd8664e09caf81209e`

Identity-rewritten equivalent: `ca07031a49baa138faead466149d46c8c29815b6`

## Scope and provenance

This document records the release-qualified cold-path ABBA gate for the
current HIG implementation. The workflow ran on a qualified macOS arm64
runner and compared retained HIG v1.9.6 and v1.9.7 binaries with the current
v1.10.0 binary. Every sample used a fresh cache and output location, disabled
daemon and project reuse, selected the fastest I/O mode, disabled encryption,
and restored with the current reader before acceptance.

The source report is the workflow artifact
`cold-path-qualified-abba.json`. Its SHA-256 is intentionally not duplicated
here; the artifact itself is the machine-readable source of truth and remains
available from the run URL above.

| Variant | Version | Binary SHA-256 |
|---|---:|---|
| Retained baseline | v1.9.6 | `2d009e300dc4c361509f267fd556dbd50991613208c8237ff34c8d0f4ed69025` |
| Retained adaptive baseline | v1.9.7 | `90f8d113de85d67bff292952b8e39838b1322a24cb7bd942214851aba86afe9a` |
| Current | v1.10.0 | `f1441e9a8752e14f70531a4101e31acbff600489c7bdbea11038247055d2e24e` |

## Qualified environment

| Property | Observed value | Release requirement |
|---|---:|---:|
| Free space | 39,802,478,592 bytes | at least 20 GiB |
| 256 MiB copy median | 1,072.08 MiB/s | at least 650 MiB/s |
| 256 MiB copy p95 | 1,617.93 MiB/s | at least 500 MiB/s |
| Qualification | `true` | required |

The deterministic corpus contained 3,004 files and 62,619,648 bytes. Its
SHA-256 tree digest was
`eb5778d660c45d9e28b08fd0741f1cf15de42b8fe98c91a843a8187d8f4ffafe`.

## ABBA results

The counterbalanced order was `v1.9.6, current, current, v1.9.6` followed by
`v1.9.7, current, current, v1.9.7`. The release gate uses the conservative
upper median for each even-sized sample group.

| Metric | v1.9.6 | v1.9.7 | Current | Current vs v1.9.7 |
|---|---:|---:|---:|---:|
| Total core duration | 197,165 us | 273,164 us | 169,022 us | 38.1% faster |
| Scan wall duration | 58,000 us | 109,000 us | 63,713 us | 41.5% faster |
| Block preparation | 84,000 us | 126,000 us | 87,000 us | 31.0% faster |
| Output write | 15,378 us | 35,402 us | 13,546 us | 61.7% faster |
| Archive bytes | 50,376,424 | 50,376,425 | 50,376,427 | within gate |
| Peak pipeline memory | 50,349,256 | 92,292,296 | 142,623,944 | below 1 GiB budget |
| Payload spool bytes | 0 | 0 | 0 | no spool |

All eight samples processed the same input identity and restored to the exact
corpus digest. The current samples read 12,288,000 source bytes through the
fused cold path, retained 50,331,648 hot raw bytes and 50,349,256 payload
bytes, and used zero payload spool bytes.

## Gate evaluation

The machine-readable report marked `release_gate_status` as `PASS`; every gate
was true:

- total duration: 169,022 us, below 216,881.5 us for v1.9.6 and 300,480.4 us
  for v1.9.7;
- scan duration: 63,713 us, below 63,800 us for v1.9.6 and 103,550 us for the
  v1.9.7 improvement gate;
- block preparation: 87,000 us, below 138,600 us;
- output write: 13,546 us, below 38,942.2 us;
- peak pipeline memory: 142,623,944 bytes, below 1,073,741,824 bytes;
- archive size: 50,376,427 bytes, below 50,880,189.25 bytes;
- input identity and exact restore: passed for every sample.

This evidence closes the qualified-NVMe ABBA requirement in the production
completion matrix. The workflow executed against source commit `82543018`;
the public-history identity correction maps that content-identical commit to
`ca07031a` without changing its tree, message, or timestamps.
