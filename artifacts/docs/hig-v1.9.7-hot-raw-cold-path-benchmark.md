# HIG v1.9.7 Hot Raw Cold-Path Benchmark

Date: 2026-07-05

## Scope

This stage targets first-pack cold-path time for real IDE projects. The archive
format, manifest layout, block layout, cache schema, and unpack path are
unchanged.

The change increases adaptive-mode source-byte reuse between scan and block
preparation:

- Adaptive scan now uses a hot raw budget resolved from the same memory policy
  family as payload staging.
- Low memory mode keeps the fixed 64 MiB hot raw budget.
- Block preparation reports how many source bytes came from retained hot raw
  memory versus fresh source-file reads.
- Reported peak pipeline memory now includes retained scan hot raw bytes.

## Real Project Corpus

Path: `/Volumes/Build/hig-real-project-bench-20260702/corpus-links`

- Files: 17,583
- Input bytes: 505,906,599
- Speed mode: `fastest`
- Encryption: `none`
- Daemon: `off`
- Project snapshot: `off`
- Hig cache: fresh per run
- Payload memory mode: `adaptive`

Command shape:

```text
hig pack <corpus> --output <corpus>/.hig-real-benchmark-output.hig \
  --cache-dir <fresh-cache> --daemon off --project off \
  --speed fastest --encryption none --memory-mode adaptive --json
```

## Results

| Metric | Previous adaptive median | Hot raw run 1 | Hot raw run 2 |
|---|---:|---:|---:|
| Core duration | 69.49 s | 96.26 s | 46.35 s |
| Scan | 29.61 s | 71.00 s | 30.24 s |
| Block prepare | 34.28 s | 18.58 s | 9.71 s |
| Output write | 5.51 s | 6.47 s | 6.27 s |
| Source bytes read during block prepare | not reported | 0 | 0 |
| Source bytes reused from hot raw | not reported | 505,906,599 | 505,906,599 |
| Hot raw bytes retained | 67,108,864 | 505,906,599 | 505,906,599 |
| Payload memory bytes | 247,620,147 | 247,620,147 | 247,620,147 |
| Spool bytes | 0 | 0 | 0 |
| Reported peak pipeline memory | 289,563,187 | 795,469,786 | 795,469,786 |

## Interpretation

The optimization removes the repeated source-file reads from block preparation
for this 506 MB corpus. The stage-level improvement is visible even when total
wall time varies:

- Block preparation improved from the previous adaptive median of 34.28 s to
  18.58 s and then 9.71 s.
- Run 1 total time was dominated by a slow source scan at 71.00 s.
- Run 2 scan time returned to the previous median range, and total duration
  dropped to 46.35 s.

The deterministic trade-off is memory: retaining the full raw corpus plus the
compressed payload raises reported peak pipeline memory from about 289.56 MiB
to about 795.47 MiB for this workload. `--memory-mode low` remains available
for constrained environments.

## Verification

- `cargo fmt --all --check`: passed
- `cargo test -p hig-core -p hig-cli`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- Small pack smoke: `source_read_bytes=0`, `source_hot_raw_bytes=input_bytes`

## Artifacts

```text
/Volumes/Build/hig-real-project-bench-20260702/runs-hotraw-20260705/current.json
/Volumes/Build/hig-real-project-bench-20260702/runs-hotraw-20260705/current-2.json
```
