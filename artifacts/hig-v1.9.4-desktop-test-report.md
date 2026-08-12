# Hig v1.9.4 Desktop Test Report

## Summary

v1.9.4 focuses on desktop i18n and product polish. The HIGV2 archive format, default KDF parameters, AEAD behavior, daemon task protocol, and compression policies were not changed.

## Implemented

- Added typed desktop i18n dictionaries for English and Simplified Chinese.
- Added `system | en | zh-CN` language preference and persisted it in desktop settings.
- Localized navigation, pages, forms, task states, cache actions, security warnings, and known backend error codes.
- Added runtime language switching in Settings.
- Kept passwords out of settings, task history, screenshots, and frontend persistent state.
- Updated macOS desktop package version and bundled sidecar CLI to `1.9.4`.

## Verification

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| `cargo clippy -p hig-core -p hig-cli -p hig-ffi --all-targets -- -D warnings` | Passed |
| `cargo test -p hig-core -p hig-cli --quiet` | Passed |
| `cargo check -p hig-desktop` | Passed |
| `pnpm lint` | Passed |
| `pnpm test` | Passed, 9 frontend tests |
| `pnpm build` | Passed |
| `pnpm tauri build --target universal-apple-darwin` | Passed |
| `target/release/hig --version` | `hig 1.9.4` |
| `src-tauri/binaries/hig-universal-apple-darwin --version` | `hig 1.9.4` |
| DMG SHA256 verification | Passed |
| Source tarball SHA256 verification | Passed |

## UI Checks

Screenshots were captured for English and Chinese desktop states:

- `artifacts/hig-v1.9.4-ui-screenshots/en/projects-1280x800.jpg`
- `artifacts/hig-v1.9.4-ui-screenshots/en/settings-1512x982.jpg`
- `artifacts/hig-v1.9.4-ui-screenshots/zh-CN/settings-1280x800.jpg`
- `artifacts/hig-v1.9.4-ui-screenshots/zh-CN/settings-1024x700.jpg`

Browser geometry checks at `1024x700` confirmed no horizontal overflow and no sampled button/select/input/code element overflow.

## macOS Package

Generated package:

- `artifacts/hig-v1.9.4-desktop-macos-universal.dmg`
- `artifacts/hig-v1.9.4-desktop-macos-universal.dmg.sha256`

The app bundle was signed with the locally available Apple Development identity and hardened runtime. Notarization was not performed because no Developer ID notarization credentials were configured.

## Known Limitation

`cargo test -p hig-desktop --lib --quiet` did not produce a reliable terminal result in the PTY session after an earlier stall. The desktop adapter was still compiled by `cargo check -p hig-desktop` and by the full Tauri release build.

## Artifacts

- `artifacts/hig-v1.9.4-desktop-macos-universal.dmg`
- `artifacts/hig-v1.9.4-desktop-macos-universal.dmg.sha256`
- `artifacts/hig-v1.9.4-ui-screenshots/`
- `artifacts/hig-v1.9.4-desktop-test-report.md`
- `artifacts/hig-v1.9.4-source.tar.gz`
- `artifacts/hig-v1.9.4-source.tar.gz.sha256`
