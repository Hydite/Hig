# Hig v1.9.6 Security and Privacy Audit

## Summary

Status: PASS. No blocking security or privacy issues were found.

v1.9.6 does not change the HIGV2 archive format, cryptographic primitives, cache format, or default security model. The review focuses on Desktop/Tauri permissions, password flow, benchmark diagnostics, and user-visible risk boundaries.

## Tauri Capability Review

| Check | Result | Notes |
| --- | --- | --- |
| Arbitrary shell exposed to frontend | Pass | `capabilities/default.json` grants `core:default`, `dialog:allow-open`, and `dialog:allow-save` only. |
| Arbitrary filesystem API exposed to frontend | Pass | No broad fs capability is granted to the webview. |
| Sidecar control | Pass | Benchmark sidecar execution is implemented in the Rust adapter, not directly by the frontend. |
| CSP remote script access | Pass | Tauri CSP uses `default-src 'self'`; no remote scripts are allowed. |
| Dialog permissions | Pass | Open/save dialogs are required for archive and directory selection. |

## Password and Key Flow

| Check | Result | Notes |
| --- | --- | --- |
| Password written to settings | Pass | Settings structures do not contain password fields. |
| Password written to task history | Pass | Task status stores output/status/error metadata, not submitted passwords. |
| Password written to benchmark command args | Pass | Desktop benchmark uses `HIG_BENCH_PASSWORD` child environment instead of argv. |
| Session key returned to frontend | Pass | Session key is installed and retained in daemon memory; UI only sees session status. |
| Password React state clearing | Pass | Existing frontend tests cover clearing password after unlock and submit flows. |

## Risk Warnings

| Mode | Result | Notes |
| --- | --- | --- |
| No encryption | Pass | UI text explains no confidentiality and no AEAD authentication. |
| Fastest | Pass | UI text warns about metadata trust and sealed cache reuse risk. |
| Trust metadata | Pass | It remains an explicit advanced option; default balanced does not enable it. |
| fast-bench | Pass | Restricted to Diagnostics; not exposed as a normal archive/session KDF profile. |

## Blocking Issues

None.

## Non-Blocking Notes

- The macOS DMG is signed with an Apple Development identity but not notarized.
- Windows packaging is not a v1.9.6 release blocker.
- Benchmark results on `ENVIRONMENT_NOT_QUALIFIED` volumes must not be presented as absolute performance guarantees.

## Verification Evidence

- `capabilities/default.json` grants only `core:default`, `dialog:allow-open`, and `dialog:allow-save`.
- Tauri CSP is `default-src 'self'` with no remote script source.
- Frontend and Rust settings tests confirm that settings contain no password field.
- Desktop diagnostics passes the benchmark password through `HIG_BENCH_PASSWORD`, not child argv.
- Secure and unencrypted CLI smoke archives both passed inspect/unpack; wrong-password unpack failed without creating an output directory.
- DMG and `.app` passed strict code-signature verification.
