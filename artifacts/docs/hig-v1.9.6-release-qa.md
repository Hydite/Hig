# Hig v1.9.6 Release QA

## Release Status

Status: RC checks passed with an environment-qualified performance limitation.

Hig v1.9.6 does not change HIGV2, compression policy, cache format, or the default security model.

## Automated Checks

| Check | Result | Evidence |
| --- | --- | --- |
| Rust format | Pass | `cargo fmt --all --check` |
| Rust clippy | Pass | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust tests | Pass | 109 tests passed |
| Release workspace build | Pass | `cargo build --release --workspace` |
| Frontend lint | Pass | ESLint |
| Frontend tests | Pass | 9 Vitest tests |
| Frontend build | Pass | Vite production build |
| Universal macOS bundle | Pass | `.app` and DMG generated |

## Release Artifacts

| Artifact | Result |
| --- | --- |
| Universal DMG | Pass, 21MiB |
| DMG SHA-256 | `be8ea2247ee552eeaa794e1400c69ce69f433d76ff02736a88c0cc3d1f4862de` |
| Source archive | Pass, 7.5MiB, excluded artifacts/build/cache/binary outputs |
| Source SHA-256 | `fcb432e18e2954d0a124745a27920e413d65083a4c3c09d1ec629734849ab67e` |
| Code signature | Pass, Apple Development identity, strict/deep verification |

## Benchmark Qualification

The LobeHub release benchmark completed successfully:

- status: `NOT_ABSOLUTE_PASS_ENV_UNQUALIFIED`;
- selected volume: `/private/tmp/hig-v196-bench`;
- selected 256MiB copy median: `538.21 MiB/s`;
- workspace 256MiB copy median: `48.18 MiB/s`;
- project warm median/p95: `169.99ms / 508.58ms`;
- project CLI wall median: `111.86ms`;
- normal first pack: `1.183s`, versus zip `4.384s`;
- archive: `57,110,242` bytes, versus zip `67,749,385` and tar.gz `61,332,985`;
- correctness digest match: true;
- watcher overflow count: 0.

The absolute `<150ms` warm gate is not claimed. Median hotspots were output write `105.18ms`, output flush `38.29ms`, and crypto `20.98ms`.

## Windows Preflight

Windows is not a v1.9.6 release target. Frontend production build is platform-independent and passed. No Windows Rust target is installed on this machine, so a Windows Rust/Tauri package was not built.

Remaining work:

- replace Unix-domain daemon transport and peer credential checks;
- package, sign, and locate the Windows sidecar;
- run watcher correctness and performance acceptance;
- build and test a Windows installer.

## Distribution Boundary

The macOS application is locally signed. It is not notarized and must not be described as ready for unrestricted public Internet distribution.
