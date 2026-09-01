# Hig v1.9.7 Cold-Path Benchmark

Date: 2026-07-01

## Objective

Measure the v1.9.7 bounded raw-byte reuse optimization against the archived
v1.9.6 implementation. The optimization retains source bytes already read for
BLAKE3 hashing, up to half of the configured pipeline memory budget, and reuses
them during block preparation. The archive format is unchanged.

## Method

- v1.9.6 binary: release build from `hig-v1.9.6-source.tar.gz`.
- v1.9.7 binary: release build from the current workspace.
- Corpus: 6,154 files, approximately 291 MiB.
- Corpus composition: replicated source/configuration files, six 16 MiB random
  files, and four 16 MiB highly compressible files.
- Corpus content digest:
  `975c33ec24857e01536974a4546da202d9c579839cdf076c497e2b32966e8d12`.
- Each run used a new cache directory, a new output path, balanced compression,
  and `--daemon off`.
- Runs were interleaved between versions to reduce ordering bias.
- Cold cache means an empty Hig cache. The operating-system page cache was not
  flushed, so these results isolate application cold-path behavior rather than
  physical cold-disk latency.

## Unencrypted Cold Runs

Three runs per version were used to isolate scanning, reading, compression, and
output behavior from KDF and encryption costs.

| metric | v1.9.6 median | v1.9.7 median | change |
|---|---:|---:|---:|
| core total | 1,605.5 ms | 1,495.5 ms | -6.9% |
| CLI wall | 1.63 s | 1.61 s | -1.2% |
| source read worker time | 810.5 ms | 196.2 ms | -75.8% |
| block preparation | 865 ms | 784 ms | -9.4% |
| retained raw bytes | 0 | 64.0 MiB | +64.0 MiB |
| archive size | 119.60 MB | 119.60 MB | no material change |

The first v1.9.7 wall sample was 2.77 seconds while its reported core duration
was 1.50 seconds. The median limits the effect of this process-exit/I/O outlier.

## Password-Encrypted Cold Runs

Five runs per version used the default secure password path. Absolute medians
were affected by increasing output-write latency during the sequence, so paired
interleaved samples are more informative than comparing unpaired medians.

| metric | v1.9.6 median | v1.9.7 median | change |
|---|---:|---:|---:|
| source read worker time | 761.0 ms | 193.8 ms | -74.5% |
| crypto worker time | 43.6 ms | 41.6 ms | -4.6% |
| retained raw bytes | 0 | 64.0 MiB | +64.0 MiB |
| archive size | 119.62 MB | 119.62 MB | no material change |

Excluding one v1.9.7 process-exit outlier, paired core-duration differences were
approximately -0.8%, +1.0%, +3.7%, and -6.4%. The optimization therefore gives
a stable read-time reduction, while end-to-end secure duration remains neutral
within the observed output-I/O variance.

## Correctness and Compatibility

Both secure archives were extracted with both CLI versions:

| archive | extractor | result digest |
|---|---|---|
| v1.9.6 | v1.9.6 | input digest matched |
| v1.9.6 | v1.9.7 | input digest matched |
| v1.9.7 | v1.9.6 | input digest matched |
| v1.9.7 | v1.9.7 | input digest matched |

This confirms backward and forward interoperability for this unchanged HIGV2
format surface.

## Verification Status

- `cargo build --release -p hig-cli`: passed for v1.9.6 and v1.9.7.
- `cargo test -p hig-core`: 96 passed, 0 failed.
- `cargo test -p hig-cli`: 10 passed, 0 failed.
- End-to-end pack/unpack and cross-version digest verification: passed.
- A supplementary `cargo test --workspace` run did not complete because debug
  `rustc` processes stalled with zero CPU while linking on `/Volumes/Build`.
  No test failure was reported. Desktop workspace verification remains separate
  from this cold-path benchmark gate.

## Conclusion

The v1.9.7 change achieves its narrow objective: duplicate source-read work for
the retained 64 MiB window falls by approximately 75%, with no format or archive
size regression. It does not yet solve overall first-pack latency because output
write/flush variance and large files outside the retained window dominate the
remaining path.

The next optimization should target large-file chunk planning and compression
so whole-file hashing, chunk hashing, and compression do not reread the same
large payload. Output-write and flush telemetry should remain a separate I/O
track because it can mask CPU/read-path gains.
