# Hig Desktop Guide

This guide covers the v1.9.6 release-candidate desktop app. The macOS build is signed but not notarized.

## Create Archive

1. Open **Create Archive**.
2. Choose an input directory and output `.hig` path.
3. Keep the default **Balanced + Password** mode for secure archives.
4. Enter the password only when starting the task. Hig does not save it.
5. Use advanced settings only when you need compatibility or diagnostics:
   - HIGV1/HIGV2 and compact/legacy manifest.
   - Explicit zstd level or worker count.
   - cache directory, project mode, solid mode, batch and chunk thresholds.
6. Avoid **No encryption** for private data. It provides no confidentiality or AEAD authentication.
7. Use **Fastest** only when you accept the metadata/sealed-cache risk warning.

## Open Archive

1. Open **Open Archive**.
2. Choose a `.hig` file.
3. Enter the password if the archive is encrypted.
4. Click **Inspect** to authenticate and read the manifest.
5. Choose an output directory and extract.
6. Enable overwrite only after confirming existing files may be replaced.

## Projects

1. Open **Projects** and initialize a project directory.
2. Hig creates a `.hig/project.json` config and lets the daemon watch the directory.
3. Ready means the daemon has a verified snapshot.
4. Dirty or Invalid means Hig will rebuild, fall back, or fail depending on the selected project mode.
5. Rebuild runs as a daemon task and appears in **Tasks**.

## Runtime

Use **Runtime** to start, restart, or stop the cache-bound daemon. Session unlock keeps the derived key only in daemon memory until the configured TTL expires. Restarting the daemon clears the session.

## Tasks

Pack, unpack, project rebuild, cache GC, cache compact, and diagnostics jobs are tracked as daemon tasks. Completed, failed, and cancelled tasks can be restored from the daemon while the result is retained.

## Cache

Use **Cache** to inspect cache size, journal state, and compaction recommendations. GC and compact actions run as dry-run previews first; confirmed operations become daemon tasks.

## Diagnostics

Diagnostics runs the existing CLI benchmark harness through a constrained sidecar call. Benchmark passwords are passed through a controlled child environment and are not saved in settings, task history, or logs.

## Troubleshooting

- **Daemon unavailable**: start the daemon from Runtime, or retry after restart.
- **No session**: unlock a session or use a password-backed archive task.
- **Wrong password**: the archive cannot be authenticated; no trusted output should be written.
- **Environment not qualified**: the benchmark volume did not meet the copy baseline. Treat absolute speed gates as not claimable for that run.
