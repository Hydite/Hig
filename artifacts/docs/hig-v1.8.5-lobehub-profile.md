# Hig v1.8.5 Lobehub Profile

## Dataset

- Input: `/private/tmp/hig-lobehub-source-data`
- Files: `14330`
- Bytes: `199203924`
- Excluded: `.git, .hig-cache, node_modules, .next, dist, build`

## Top-Level Sizes

| path | bytes |
|---|---:|
| `ios` | 87118635 |
| `packages` | 48044624 |
| `locales` | 22669403 |
| `src` | 16600359 |
| `apps` | 13221325 |
| `public` | 5270150 |
| `docs` | 2404334 |
| `changelog` | 1556470 |
| `.agents` | 800780 |
| `docker-compose` | 306092 |
| `e2e` | 301686 |
| `scripts` | 266045 |
| `.github` | 201494 |
| `CHANGELOG.md` | 75689 |
| `plugins` | 71185 |
| `.claude` | 50128 |
| `README.zh-CN.md` | 38886 |
| `README.md` | 34950 |
| `package.json` | 23898 |
| `.cursor` | 22360 |

## Comparison

| tool | duration ms | archive bytes | notes |
|---|---:|---:|---|
| higv2 balanced first | 1705 | 56729432 | default HIGV2 batch/chunk format |
| higv2 balanced secure daemon | 1420 | 56729505 | secure hot daemon/session path; KDF skipped and cache index is warm |
| higv2 fastest secure | 1832 | 64900109 | fastest mode with secure KDF and sealed block reuse |
| higv2 no-encryption | 2205 | 56720469 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| zip | 4209 | 67642760 | zip -qr |
| tar.gz | 9859 | 61224974 | tar -cf + gzip -6 |
| tar.zst | 6678 | 64878398 | tar -cf + zstd -1 |

## Release Gates

- Environment: `QUALIFIED` (256MiB copy median 819.51 MiB/s)
- Pack-core gate: `true` median 377.563 ms
- Standalone second median: `317.747 ms`
- Summary + quiet CLI-wall gate: `true` median 289.194 ms
- Full + JSON CLI-wall median: `281.368 ms`
- Size-quality gate: `true`

## Incremental Scenario

- Modified files: `apps/cli/README.md, apps/cli/e2e/agent-fs-vfs.e2e.test.ts, apps/cli/e2e/agent-signal.e2e.test.ts, locales/ar/agent.json, public/.well-known/assetlinks.json`
- Hig pack-core: `500.934 ms`
- Hig CLI-wall: `455.256 ms`
- zip CLI-wall: `4056.302 ms`
- Cache hit rate: `100.00%`
- Solid groups/files: `114/11019`
- Journal bytes/entries after: `171594/2`

## Daemon Hot Path

- Daemon pack-core minus standalone second: `59.816 ms`
- Summary response savings vs Full JSON CLI-wall: `-7.826 ms`
- Socket connect / roundtrip: `6/1422191` us
- Daemon auth / job execute: `25/1422084` us
- Response serialize / client decode / bytes: `6/13/1506`
- Cache commit: `0 ms`

## Bottleneck Readout

- Daemon-to-standalone gap is within the 100ms release tolerance; preserve this path as a regression gate.
