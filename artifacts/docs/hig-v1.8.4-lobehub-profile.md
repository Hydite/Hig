# Hig v1.8.4 Lobehub Profile

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
| higv2 balanced first | 2119 | 56729477 | default HIGV2 batch/chunk format |
| higv2 balanced secure daemon | 1982 | 56729457 | secure hot daemon/session path; KDF skipped and cache index is warm |
| higv2 fastest secure | 1946 | 64900109 | fastest mode with secure KDF and sealed block reuse |
| higv2 no-encryption | 2461 | 56720452 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| zip | 4463 | 67642760 | zip -qr |
| tar.gz | 12630 | 61224353 | tar -cf + gzip -6 |
| tar.zst | 8046 | 64878425 | tar -cf + zstd -1 |

## Release Gates

- Environment: `ENVIRONMENT_NOT_QUALIFIED` (256MiB copy median 264.30 MiB/s)
- Pack-core gate: `true` median 751.265 ms
- CLI-wall gate: `true` median 770.490 ms
- Size-quality gate: `true`

## Incremental Scenario

- Modified files: `apps/cli/README.md, apps/cli/e2e/agent-fs-vfs.e2e.test.ts, apps/cli/e2e/agent-signal.e2e.test.ts, locales/ar/agent.json, public/.well-known/assetlinks.json`
- Hig pack-core: `952.218 ms`
- Hig CLI-wall: `602.366 ms`
- zip CLI-wall: `4263.852 ms`
- Cache hit rate: `100.00%`
- Solid groups/files: `114/11019`
- Journal bytes/entries after: `171594/2`

## Bottleneck Readout

- If cold path is slow, prioritize scan/hash parallelism and KDF/session UX.
- If warm path is slow, inspect daemon pack-core telemetry before optimizing CLI wrapper.
- If incremental miss scope is high, tune solid group policy on real project data.
