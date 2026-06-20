# Hig

## Language Index

- English: [README.md](README.md)
- Chinese: [hig-docs/README.zh-CN.md](hig-docs/README.zh-CN.md)
- Korean: [hig-docs/README.ko.md](hig-docs/README.ko.md)
- German: [hig-docs/README.de.md](hig-docs/README.de.md)
- Russian: [hig-docs/README.ru.md](hig-docs/README.ru.md)
- Japanese: [hig-docs/README.ja.md](hig-docs/README.ja.md)

## Abstract

We build Hig as a desktop application for fast, compact, encrypted project archiving. Our goal is to make project snapshots practical during active development: quick enough to run often, small enough to keep or move, and strict enough to verify.

In our latest public benchmark with direct `zip`, `tar.gz`, and `tar.zst` comparisons, Hig produced a smaller archive while completing the measured project archive workflow much faster than the baseline tools.

## Key Advantages

| Advantage | Result in the latest public comparison |
| --- | --- |
| Speed | `164.008 ms` project CLI wall time. Compared with zip, tar.gz, and tar.zst, measured time was reduced by `96.0%`, `98.4%`, and `97.6%`. |
| Archive size | `57,108,395 bytes`. The archive was `15.7%` smaller than zip, `6.9%` smaller than tar.gz, and `12.0%` smaller than tar.zst. |
| Incremental workflow | Single-edit and five-edit archive operations completed in `253.535 ms` and `96.243 ms`. |
| Burst handling | A 1000-event catch-up completed in `111.635 ms`, with `0` watcher overflows. |
| Correctness | The benchmark correctness digest matched: `true`. |
| Desktop readiness | v1.9.4 desktop package build, checksum verification, linting, tests, and UI overflow checks passed. |

## What Hig Does

Hig packages a project into a recoverable encrypted archive with an emphasis on repeated project snapshots. We use this model for workflows such as saving a project state before a risky edit, moving a compact archive between machines, keeping a fast local recovery point, and preserving a verified release artifact.

This public desktop release repository focuses on the user-facing application, documentation, and downloadable packages.

## Benchmark Method

The latest public comparison dataset used a test corpus of `15,330` files totaling `198,974,618` bytes (`198.97 MB`, `189.76 MiB`). The same corpus was used for Hig, zip, tar.gz, and tar.zst comparisons.

Environment status: `ENVIRONMENT_NOT_QUALIFIED`  
Correctness digest match: `true`  
Watcher overflow count: `0`

Because the environment was not marked as fully qualified, the numbers below should be read as a transparent benchmark snapshot rather than a universal performance guarantee.

## Benchmark Results

| Tool or scenario | Duration | Archive size | Time reduction vs baseline | Size reduction vs baseline |
| --- | ---: | ---: | ---: | ---: |
| Hig project CLI wall | `164.008 ms` | - | - | - |
| Hig project burst archive | `120.430 ms` | `57,108,395 bytes` | - | - |
| zip | `4,088 ms` | `67,749,381 bytes` | Hig CLI wall was `96.0%` lower, `24.9x` faster | Hig archive was `15.7%` smaller |
| tar.gz | `10,098 ms` | `61,313,475 bytes` | Hig CLI wall was `98.4%` lower, `61.6x` faster | Hig archive was `6.9%` smaller |
| tar.zst | `6,724 ms` | `64,898,790 bytes` | Hig CLI wall was `97.6%` lower, `41.0x` faster | Hig archive was `12.0%` smaller |

Repeat-pack and hot-path measurements:

| Scenario or stage | Measurement |
| --- | ---: |
| Same-corpus warm pack sample #2, full archive write | `171,100 us` / `171.100 ms` |
| Same-corpus warm pack sample #3, full archive write | `150,134 us` / `150.134 ms` |
| Same-corpus warm pack median, 20 full-write samples | `108,916 us` / `108.916 ms` |
| Same-corpus warm pack p95, 20 full-write samples | `455,894 us` / `455.894 ms` |
| Project metadata verify, warm median | `10,102 us` |
| Planning, warm median | `2,639 us` |
| Manifest serialization, warm median | `1,004 us` |
| Manifest encryption, warm median | `690 us` |
| Output file create, warm median | `119 us` |
| Read and compression, warm median | `0 us` / `0 us` |
| Single-edit pack | `253.535 ms` |
| Five-edit pack | `96.243 ms` |
| 1000-event burst catch-up | `111.635 ms` |

## v1.9.4 Desktop Release

Latest public build: `v1.9.4`  
Primary package: `hig-v1.9.4-desktop-macos-universal.dmg`  
SHA-256: `b7075058b98b848a332efeca31f5320ccfe1ccd2accd83173145b5e00df7a7af`  
Package size: about `21 MB`

| Verification item | Result |
| --- | --- |
| Desktop package build | Passed |
| macOS universal build | Passed |
| CLI version in bundle | `hig 1.9.4` |
| DMG SHA-256 verification | Passed |
| Release checksum verification | Passed |
| Core quality checks | Passed |
| Desktop lint, tests, and build | Passed |
| Frontend tests | Passed, 9 tests |
| UI overflow checks | Passed on sampled desktop sizes |

The app bundle was signed with the locally available Apple Development identity and hardened runtime. Notarization was not performed for this build because Developer ID notarization credentials were not configured.

## Interpretation

Our reading of the benchmark is that Hig is strongest in project snapshot workflows where repeated archive operations, compact output, and correctness checks all matter. Traditional general-purpose archive tools remain broadly compatible and useful, but in this measured project workload Hig delivered substantially lower wall time and smaller output.

## Developer

Yike Wang  
GitHub: [Aiomx](https://github.com/Aiomx)  
Published under: [Hydite](https://github.com/Hydite)
