# Hig v1.8.3 Lobehub Profile

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
| higv2 balanced first | 20855 | 56729451 | default HIGV2 batch/chunk format |
| higv2 balanced secure daemon | 804 | 56729514 | secure hot daemon/session path; KDF skipped and cache index is warm |
| higv2 fastest secure | 425 | 64900109 | fastest mode with secure KDF and sealed block reuse |
| higv2 no-encryption | 21504 | 56720437 | no confidentiality or AEAD; BLAKE3 corruption checks remain |
| zip | 4002 | 67642760 | zip -qr |
| tar.gz | 10246 | 61223862 | tar -cf + gzip -6 |
| tar.zst | 7081 | 64878014 | tar -cf + zstd -1 |

## Release Gates

- Environment: `QUALIFIED` (256MiB copy median 827.52 MiB/s)
- Pack-core gate: `true` median 542.466 ms
- CLI-wall gate: `true` median 583.964 ms
- Size-quality gate: `true`

## Incremental Scenario

- Modified files: `apps/cli/README.md, apps/cli/e2e/agent-fs-vfs.e2e.test.ts, apps/cli/e2e/agent-signal.e2e.test.ts, locales/ar/agent.json, public/.well-known/assetlinks.json`
- Hig pack-core: `990.998 ms`
- Hig CLI-wall: `695.268 ms`
- zip CLI-wall: `4297.946 ms`
- Cache hit rate: `100.00%`
- Solid groups/files: `114/11019`
- Journal bytes/entries after: `365897/2`

## Bottleneck Readout

- If cold path is slow, prioritize scan/hash parallelism and KDF/session UX.
- If warm path is slow, inspect daemon pack-core telemetry before optimizing CLI wrapper.
- If incremental miss scope is high, tune solid group policy on real project data.
