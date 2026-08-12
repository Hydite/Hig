# Hig v1.8.2 Profile

## Scope

v1.8.2 is a release-confidence and real-world benchmark patch for HIGV2.

- Archive format unchanged.
- Default `balanced + secure` safety unchanged.
- No default metadata hash skip.
- No C++ integration.
- Main changes: v1.8.2 benchmark summary JSON, pack-core vs CLI-wall sampling, benchmark suites, daemon status hardening, output-path busy errors, and cache journal observability.

## Verification

Commands run on 2026-06-20:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace
target/release/hig --version
```

Result: all passed before benchmark artifact generation.

Test count:

- `hig-cli`: 3 tests passed.
- `hig-core`: 75 tests passed.
- `hig-ffi`: 0 tests.

## Source Compare

Dataset: `/private/tmp/hig-v182-source-data`

Created from `/Volumes/Build/Hig` excluding `.git`, `target`, `.hig-cache`, and tarball artifacts.

Formal v1.8.2 compare summary:

| metric | value |
|---|---:|
| environment | `QUALIFIED` |
| Hig pack-core median | 1.037 ms |
| Hig CLI-wall median | 2.907 ms |
| zip CLI-wall median | 11.262 ms |
| pack-core gate | pass |
| CLI-wall gate | pass |
| size-quality gate | pass |

The formal benchmark markdown is `artifacts/hig-v1.8.2-benchmark.md`.

## Corpus Results

| corpus | Hig pack-core median | Hig CLI-wall median | zip median | Hig size | zip size | tar.gz size | result |
|---|---:|---:|---:|---:|---:|---:|---|
| small500 | 4.855 ms | 8.185 ms | 19.042 ms | 4,614 B | 92,521 B | 26,526 B | pass |
| textmix | 12.285 ms | 91.824 ms | 251.344 ms | 2,695 B | 41,132 B | 12,393 B | pass |
| repeat4m | 2.813 ms | 5.783 ms | 17.220 ms | 719 B | 12,431 B | 12,756 B | pass |
| random8m | 88.133 ms | 49.088 ms | 205.746 ms | 8,389,885 B | 8,390,058 B | 8,391,758 B | pass |
| binarymix | 1.198 ms | 4.848 ms | 9.496 ms | 1,175 B | 15,473 B | 10,956 B | pass |

Notes:

- `random8m` expansion is about 0.017%, below the 0.1% gate.
- `small500` remains far smaller than zip and below the warm daemon target.
- `textmix` and `repeat4m` remain smaller than gzip.

## Cache Journal / Daemon Status

`cache status` after source compare:

```text
cache: total_bytes=676712 budget_bytes=5368709120 files=22 removable_bytes=0 removed_bytes=0 compacted_bytes=0 generation=0 journal_bytes=3101 journal_entries=1 journal_replayed_entries=0 journal_compacted_entries=0 journal_dirty_record_estimate=1 journal_compact_recommended=false journal_estimated_reclaimed_bytes=3101 last_compact_unix_ns=0 dry_run=false
```

`cache compact --dry-run` succeeded and reported the same journal state with `dry_run=true`.

## Remaining Boundaries

- `--bench-suite all` currently runs the full detailed compare on the source input and supports individual fixed corpora through `--bench-suite <name>`.
- Default balanced mode still hashes current file contents. This is intentional for integrity and is not replaced by hot metadata reuse.
