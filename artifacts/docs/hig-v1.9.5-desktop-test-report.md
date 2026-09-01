# Hig v1.9.5 Desktop Test Report

## Delivered

- Strongly typed Create Archive request with complete HIGV1/HIGV2, manifest, cache, batch, chunk, solid, KDF, project, worker and compression-level controls.
- Daemon task restoration across known cache directories.
- Project rebuild and real cache maintenance through asynchronous daemon tasks.
- Runtime daemon management and session visibility.
- Archive inspection pagination, sorting and overwrite confirmation.
- Diagnostics page reusing the constrained CLI benchmark harness.
- CLI `hig inspect` text and JSON interfaces.
- English and Simplified Chinese coverage for all new controls and errors.

## Automated Verification

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `pnpm lint`
- `pnpm test`
- `pnpm build`
- CLI pack -> inspect JSON -> unpack -> byte comparison smoke test

## Visual Verification

- 1024x700 Create Archive advanced controls: no viewport or control overflow.
- 1280x800 English Diagnostics: no viewport or control overflow.
- 1512x982 Simplified Chinese Diagnostics: no viewport or control overflow.
- Navigation, labels, warnings and task controls remain readable in both languages.

## Security and Compatibility

- Secure defaults are unchanged.
- HIGV2 archive layout is unchanged.
- HIGV1 and legacy/compact HIGV2 reading remain supported.
- Benchmark passwords are passed through the child environment, not command-line arguments, and are not persisted.
- DMG is locally signed and is not notarized.

