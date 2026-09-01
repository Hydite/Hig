# Hig v1.8.1 Profile

## Scope

v1.8.1 is a speed patch for HIGV2 without changing archive format or default security semantics.

- No hot metadata hash skip in default `balanced + secure`.
- Strong KDF defaults are unchanged.
- `fastest` and `--trust-metadata` remain explicit risk modes.
- Primary changes: short/json/quiet reports, daemon direct pack, structured daemon errors, and cache journal writes.

## Verification

Commands run on 2026-06-19:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace
```

Result: all passed.

Test count:

- `hig-cli`: 3 tests passed.
- `hig-core`: 75 tests passed.
- `hig-ffi`: 0 tests.

## Source Dataset

Dataset: `/private/tmp/hig-v181-source-data`

Created from `/Volumes/Build/Hig` excluding `.git`, `target`, `.hig-cache`, and source tarball artifacts.

Isolated warm daemon sampling:

| metric | Hig balanced secure daemon | zip -qr |
|---|---:|---:|
| median | 2.032 ms | 42.633 ms |
| p95 | 2.769 ms | 53.229 ms |
| archive size | 94,641 B | 118,461 B |

Result:

- Hig warm daemon is below the `<3ms` target in isolated sampling.
- Hig is about 20.1% smaller than zip on this source dataset.
- Hig median is about 21x faster than zip in isolated warm daemon sampling.

Formal `bench --compare` output is in `artifacts/docs/hig-v1.8.1-benchmark.md`. The latest formal run marked the selected `/tmp` volume as `ENVIRONMENT_NOT_QUALIFIED` because native 256MiB copy baseline was below the qualification threshold, so those absolute timing rows should not be used as the release gate.

## 500 Small Files

Dataset: 500 text files, 28,500 input bytes.

| mode | duration | archive size | notes |
|---|---:|---:|---|
| daemon first pack | 50.934 ms | 4,939 B | journal path, first cache population |
| daemon second pack single sample | 5.739 ms | 4,938 B | no cache commit |
| daemon second pack 20-run median | 4.504 ms | 4,938 B | below `<5ms` target |
| daemon second pack p95 | 5.231 ms | 4,938 B | real content hash still performed |

Result:

- First pack target `<80ms` passed.
- Warm daemon median `<5ms` passed.
- Default balanced mode still hashes file contents; no metadata-only hash skip was used.

## Key Observations

- Cache journal removes the expensive shard write path from warm cache runs. In the 500-file warm daemon run, `cache_commit_us=0` and dirty shard count was 0.
- Direct daemon pack removes the pre-pack daemon status roundtrip. `--use-session` without an active session now returns structured `NoSession` and does not fall back to standalone.
- Report modes reduce user-facing overhead:
  - default short output is one line,
  - `--quiet` emits no stdout on success,
  - `--json` preserves full machine-readable telemetry.

## Remaining Limit

The default secure path still reads and hashes current file contents. This is intentional for integrity. Avoiding that cost requires explicit `--trust-metadata` or `--speed fastest`, which remain opt-in risk modes.
