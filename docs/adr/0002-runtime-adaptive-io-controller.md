# ADR 0002: Runtime-Adaptive I/O Controller

- Status: Accepted
- Date: 2026-07-22
- Version: HIG 1.9.7

## Context

HIG is primarily used with local system storage, but some projects and caches
reside on external, network-attached, or enterprise disks. A startup-only disk
probe cannot represent the full pack operation: a fast disk may become busy,
and a constrained disk may recover while the same job is running. A permanent
`slow` profile also makes an initial transient condition persist for the whole
job.

The previous cold path submitted all source reads through the global Rayon
pool. Cache pack reads and writes, payload spooling, and archive output used
independent buffering strategies and had no shared feedback state.

## Decision

HIGV2 pack operations create one `AdaptiveIoController` for the complete job.
The same controller is passed through source scanning, block-source reads,
object-pack cache reads and writes, payload spool writes, payload prefetch, and
archive output. State is never reset at a pipeline boundary.

The controller starts at the actual pack worker count. It does not run a
separate startup probe and does not classify a filesystem or device by name.
Each I/O completion supplies:

- stage and read/write direction;
- actual bytes transferred to or from the underlying file;
- service time;
- time spent waiting for an adaptive permit.

Each stage and direction owns an independent sliding baseline. This prevents a
fast page-cache read or cache-pack append from becoming the baseline for a
different access pattern. All stage models control the same task-level permit
target.

## Control Policy

The controller evaluates a window after at least eight samples and 8 MiB, or
after 32 MiB regardless of sample count. It uses three signals:

- absolute throughput below 48 MiB/s;
- throughput below 50% of the same stage's decaying best baseline;
- p95 latency of I/O operations no larger than 256 KiB at or above 20 ms.

A relative slowdown is ignored while absolute throughput remains at least
96 MiB/s. Two consecutive bad windows are required before a transition.
Concurrency decreases multiplicatively and never cancels in-flight I/O:

```text
next = max(1, current / 2)
```

Recovery requires two consecutive good windows with recovered throughput and
small-I/O p95 no greater than 8 ms. Concurrency increases gradually. A 750 ms
cooldown separates transitions, limiting oscillation while retaining in-job
recovery.

## Data-Path Behavior

- Small source files are read once under one permit without allocating a 1 MiB
  temporary buffer.
- Large source files are read in 1 MiB segments, allowing the target to change
  during a file read.
- BLAKE3 timing is reported separately from source-read timing; legacy
  `hash_us` remains the sum for compatibility.
- Cache object-pack ranges and appends report actual 1 MiB transfers.
- `BufWriter` operations enter the controller only when bytes are actually
  flushed to the underlying file. Pure memory appends are not treated as disk
  throughput.
- A constrained target serializes competing read/write work through permits;
  sequential file writes are not artificially delayed when there is no
  competing I/O.

Compression level, block planning, encryption, manifest encoding, archive
layout, and atomic temp-file rename semantics are unchanged.

## Observability

`PackReport.adaptive_io` records:

- initial, minimum, maximum, final, and observed concurrency;
- constrained entries and recovery steps;
- normal and constrained durations;
- final constraining stage and direction;
- exact per-stage samples, bytes, service time, and permit wait time;
- bounded transition events with timestamp, stage, direction, reason,
  throughput, p95 latency, and from/to concurrency.

Verbose CLI reports expose the same information. JSON reports remain backward
compatible because all new report fields have defaults.

## Safety and Compatibility

- Adaptive permits are released by RAII on success, error, or early return.
- Poisoned synchronization state is recovered rather than causing a second
  failure path.
- Reducing the target does not interrupt active reads or writes.
- Transition history is bounded to 256 entries.
- HIGV1 remains unchanged and reports adaptive I/O as disabled.
- HIGV2 archive bytes and unpack compatibility are unchanged.

## Verification

Unit coverage includes false-start suppression, sustained degradation,
same-direction recovery, stage-baseline isolation, high-throughput relative
slowdown protection, cooldown behavior, dynamic permit blocking, exact scan
bytes, and exact archive-write bytes. Real-project evidence is recorded in
`artifacts/docs/hig-v1.9.7-adaptive-io-benchmark.md`.
