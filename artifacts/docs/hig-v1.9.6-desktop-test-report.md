# Hig v1.9.6 Desktop Test Report

## Status

RC verification completed.

## Automated Verification

- Rust format, clippy, 109 tests, and release build: PASS.
- Frontend lint, 9 tests, and production build: PASS.
- Tauri Universal macOS bundle: PASS.
- CLI and bundled Universal sidecar both report `hig 1.9.6`.
- Secure and unencrypted archive inspect/unpack smoke: PASS.
- Wrong-password unpack failed and did not create an output directory.

## Desktop Smoke Matrix

| Workflow | Result | Notes |
| --- | --- | --- |
| Launch app from mounted DMG | Pass | Process started from mounted signed bundle |
| English / Chinese switch | Pass | Browser UI regression and frontend tests |
| Daemon start/status/stop | Pass | CLI/runtime smoke |
| Session unlock/clear | Pass | Core/daemon tests and benchmark session |
| Project init/status | Pass | FSEvents project smoke |
| Secure pack/inspect/unpack | Pass | Byte-identical output |
| Cache dry-run/compact preview | Pass | GC and compact dry-run |
| Diagnostics benchmark | Pass | LobeHub suite exited 0 and generated reports |
| App restart task restoration | Pass | Automated daemon task tests |
| Structured failure recovery | Pass | Wrong password and daemon/task tests |

## Visual Regression

English and Simplified Chinese screenshots were captured at `1024x700`, `1280x800`, and `1512x982`. The current macOS session used dark appearance; responsive layout, long labels, navigation, settings controls, and empty states showed no overlap or viewport overflow.

## Package Verification

- Universal sidecar architectures: `x86_64 arm64`.
- `.app` and DMG strict code-signature checks passed.
- The app launched from the mounted DMG.
- The package is signed but not notarized.

## Security Boundary

The application does not persist passwords or keys. Benchmark sidecar passwords use a controlled environment variable rather than command arguments. The webview has no arbitrary shell or filesystem capability.
