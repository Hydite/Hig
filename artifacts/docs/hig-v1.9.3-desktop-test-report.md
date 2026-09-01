# Hig v1.9.3 Desktop / Daemon Task Test Report

## Scope

v1.9.3 adds daemon task protocol types, daemon task request/status/result/list/cancel handling, background daemon task workers, CLI task debugging commands, and migrates desktop pack/unpack command paths to daemon-owned task submission.

## Verification

- `cargo fmt --all`: passed
- `cargo check --workspace`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- `cargo test -p hig-core -p hig-cli --quiet`: passed, 99 tests passed
- Desktop Rust adapter passed `cargo check` and strict workspace clippy.
- `pnpm lint`: passed
- `pnpm test`: passed, 3 frontend tests passed
- `pnpm build`: passed
- release CLI smoke: `hig 1.9.3`, daemon task pack/list/result/unpack roundtrip passed

## Packaging

- Source tarball: `artifacts/hig-v1.9.3-source.tar.gz`
- Source SHA256: `artifacts/hig-v1.9.3-source.tar.gz.sha256`
- Universal DMG: `artifacts/hig-v1.9.3-desktop-macos-universal.dmg`
- DMG SHA256: `artifacts/hig-v1.9.3-desktop-macos-universal.dmg.sha256`
- Bundle architecture: arm64 + x86_64
- Signing: Apple Development identity with hardened runtime; notarization was not performed.

## Notes

The daemon main loop remains responsive to task status, list, result, and cancellation requests while a background worker owns the pack engine. A 64 MiB smoke test observed the task before completion, retrieved its completed result, and verified pack/unpack byte equality. Unpack uses the existing cooperative cancellation control. Pack cancellation is currently reliable before execution starts; deeper cancellation checkpoints still depend on threading `OperationControl` through the engine-cache pack path.
