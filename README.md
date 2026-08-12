# Hig

## Language Index

- English: [README.md](README.md)
- Chinese: [hig-docs/README.zh-CN.md](hig-docs/README.zh-CN.md)
- Korean: [hig-docs/README.ko.md](hig-docs/README.ko.md)
- German: [hig-docs/README.de.md](hig-docs/README.de.md)
- Russian: [hig-docs/README.ru.md](hig-docs/README.ru.md)
- Japanese: [hig-docs/README.ja.md](hig-docs/README.ja.md)

## Abstract

Hig is a fast, compact, project-aware archiver with secure encrypted archives, daemon-backed project watching, and a macOS desktop interface. It is designed for development workflows where repeated project snapshots matter: save a verified archive before risky edits, move a compact project state between machines, or keep a fast local recovery point.

The first public engineering release is **v1.10.0**. It preserves the HIGV2
archive format and security model while adding an independent HIG repository
history layer for byte, chunk, path, and symbol-level recovery.

Password-protected archives use Argon2id, ChaCha20-Poly1305 authenticated
encryption, BLAKE3 integrity checks, and path traversal protection. The public
repository contains the format, CLI, repository, and IDE/MCP integration
implementation. Private research and paper materials are intentionally
excluded from this release.

## Downloads

| Artifact | Path |
| --- | --- |
| macOS Universal CLI/MCP | `artifacts/hig-v1.10.0-ide-mcp-macos-universal.tar.gz` |
| macOS package SHA-256 | `artifacts/hig-v1.10.0-ide-mcp-macos-universal.tar.gz.sha256` |
| Linux x86_64 GNU CLI/MCP | `artifacts/hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz` |
| Linux package SHA-256 | `artifacts/hig-v1.10.0-ide-mcp-linux-x86_64-gnu.tar.gz.sha256` |
| Linux release QA | `artifacts/hig-v1.10.0-linux-x86_64-gnu-release-qa.md` |

The macOS app is signed with the locally available Apple Development identity. It is **not notarized** because Developer ID notarization credentials are not configured for this build.

The macOS and Linux IDE packages bundle the platform-native CLI together with
the constrained Node.js MCP stdio adapter. The Linux package is validated on
Ubuntu 24.04 x86_64 with glibc 2.39 and Node.js 18 or later. The macOS package
contains a universal arm64/x86_64 CLI.

Linux package SHA-256:

```text
5f2a239a87bd2a4af38e9e97f895516011b7e8f67c94964f3dbaeed79a56338f
```

macOS package SHA-256:

```text
fc9b05bffe4faed236060f4b2792a1b98c2dda10d6f0baae828556b2094acece
```

## Quick Start

### Desktop

1. Open the DMG and launch Hig.
2. Use **Runtime** to start the daemon and unlock a session if you want fast secure repeated archives.
3. Use **Projects** to initialize a directory and let Hig prepare project snapshots in the background.
4. Use **Create Archive** to pack a directory into `.hig`.
5. Use **Open Archive** to inspect and unpack a `.hig` archive.
6. Use **Cache** for dry-run GC and compaction, and **Diagnostics** for benchmark comparisons.

### CLI

```bash
hig pack <dir> -o <archive.hig> --password <password>
hig inspect <archive.hig> --password <password> --json
hig unpack <archive.hig> -d <output-dir> --password <password>
```

Project mode:

```bash
hig init <dir>
hig session unlock --password <password> --cache-dir <cache-dir>
hig pack <dir> -o <archive.hig> --use-session --cache-dir <cache-dir>
```

Benchmark:

```bash
hig bench /Volumes/Build/lobehub \
  --compare \
  --bench-suite lobehub-watch \
  --daemon required \
  --cache-dir /private/tmp/hig-v196-cache \
  --bench-dir /private/tmp/hig-v196-bench \
  --password benchmark-password \
  --json
```

## Safety Model

| Mode | Security behavior |
| --- | --- |
| Default balanced + password | Secure Argon2id KDF, random archive salt, independent block authentication, no metadata trust. |
| Session | Derives the secure key once and keeps it only in daemon memory for the configured TTL. Passwords and keys are not written to disk. |
| Project Mode | Uses a daemon-owned verified snapshot. If the watcher overflows, restarts, or loses trust, Hig falls back to full verification or fails in required mode. |
| Fastest | Explicit speed mode. It may trust metadata and reuse sealed encrypted cache, so the UI and CLI show risk warnings. |
| No encryption | Provides compression and hash verification only. It does not provide confidentiality or AEAD authentication. |

## Cache and Daemon

Hig stores compressed cache objects and metadata under the selected cache directory. The cache accelerates repeated archives and project watch workflows. It does not store plaintext passwords or derived encryption keys. Cache corruption is handled as fail-fast or cache miss; already generated `.hig` archives remain self-contained.

The daemon owns project watchers, task queues, session keys, cache state, and benchmark diagnostics. Desktop and CLI both submit work through the same daemon/task semantics for recoverable pack, unpack, rebuild, GC, and compaction operations.

## Repository History

HIG v1.10.0 provides an HIG-native content-addressed repository. It is not a
Git wire-compatible implementation. The repository records immutable commits,
FastCDC chunks, byte-range change indexes, rename-aware path history, a
compression tree, and optional Tree-sitter semantic indexes.

```bash
hig repo init <project>
hig repo snapshot <project> -m "before refactor"
hig repo diff <project> --from <commit> --to HEAD --json
hig repo restore-range <project> --path src/lib.rs --start 120 --len 1 -o byte.bin
hig repo symbols <project> --revision HEAD --json
hig repo restore-symbol <project> --revision <commit> --symbol 'Type::method' -o method.rs
hig repo verify <project> --json
```

See [docs/ide/hig-cli-tools.md](docs/ide/hig-cli-tools.md) for the complete
CLI/MCP interface and [docs/spec/hig-format-v2.md](docs/spec/hig-format-v2.md)
for the public archive format specification.

## Benchmark Interpretation

Historical v1.9.6 and v1.9.7 benchmark reports include:

- `environment_status`: whether the selected volume passes the 256MiB copy baseline.
- `release_gate_status`: whether absolute gates passed, failed on a qualified volume, or could not be claimed because the environment was not qualified.
- `io_hotspot_summary`: the largest observed warm-path bottlenecks.
- zip, tar.gz, and tar.zst comparisons on the same corpus.

If the environment is reported as `ENVIRONMENT_NOT_QUALIFIED`, Hig still reports relative speed and size results, but it does not claim absolute `<150ms` project warm-pack performance for that run.

The v1.9.6 LobeHub RC run selected `/private/tmp` at `538.21 MiB/s`, below the `650 MiB/s` qualification threshold. It produced a `57,110,242` byte archive versus zip `67,749,385` and tar.gz `61,332,985`. Project warm median was `169.99ms`; the remaining variance was primarily output write and flush latency.

## Documentation

- Desktop guide: [hig-docs/desktop-guide.md](hig-docs/desktop-guide.md)
- Chinese README: [hig-docs/README.zh-CN.md](hig-docs/README.zh-CN.md)

English is the source of truth for the v1.10.0 public release documentation.
Other translations may lag behind detailed release notes.

## Developer

Yike Wang  
GitHub: [Aiomx](https://github.com/Aiomx)  
Published under: [Hydite](https://github.com/Hydite)
