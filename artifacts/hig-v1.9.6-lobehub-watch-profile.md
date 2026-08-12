# Hig v1.9.6 LobeHub Watch Benchmark

- Environment: `ENVIRONMENT_NOT_QUALIFIED`
- Release gate status: `NOT_ABSOLUTE_PASS_ENV_UNQUALIFIED`
- Selected volume: `/private/tmp/hig-v196-bench` (`538.21 MiB/s` median 256MiB copy)
- Fastest available volume: `/private/tmp`
- I/O hotspot summary: `output_write_us median=105182us p95=442459us; output_flush_us median=38285us p95=384604us; crypto_us median=20978us p95=33894us`
- Input: `15330` files / `198974616` bytes
- Corpus write: `29028.320 ms`
- Watcher bootstrap catch-up: `1019.469 ms`
- Single edit prepare: `50.430 ms`
- Five edit prepare: `23.851 ms`
- 1000-event burst catch-up: `123.792 ms`

| scenario | duration ms | archive bytes |
|---|---:|---:|
| normal first | 1183.106 | - |
| normal warm | 357.761 | - |
| project bootstrap pack | 1430.034 | - |
| project single edit pack | 187.417 | - |
| project five edit pack | 174.405 | - |
| project burst pack | 293.768 | 57110242 |
| project CLI wall | 111.859 | - |
| zip | 4384 | 67749385 |
| tar.gz | 10887 | 61332985 |
| tar.zst | 8504 | 64936372 |

- Project hash reuses: `15330`
- Prepared object hits: `879`
- Project metadata verify: `12587` us
- Watcher overflow count: `0`
- Correctness digest match: `true`

## Release Gates

- project warm <150ms: `false`
- project CLI <250ms: `true`
- single prepare <50ms: `false`
- five edit pack <150ms: `false`
- 1000-event burst <2s: `true`
- archive quality <= v1.8.5 +1%: `true`

## Project Warm Stage Breakdown

| stage | median us | p95 us |
|---|---:|---:|
| project_verify_us | 11676 | 22285 |
| plan_us | 2881 | 4093 |
| read_us | 0 | 0 |
| compression_us | 0 | 0 |
| crypto_us | 20978 | 33894 |
| manifest_serialize_us | 1110 | 1468 |
| manifest_compress_us | 2283 | 4222 |
| manifest_encrypt_us | 741 | 2374 |
| output_create_us | 117 | 242 |
| output_write_us | 105182 | 442459 |
| output_flush_us | 38285 | 384604 |
| output_rename_us | 2848 | 5981 |
| cache_commit_us | 0 | 0 |
| unattributed_us | 16 | 20 |

## Project Warm Top Hotspots

- `output_write_us`: median `105182us`, p95 `442459us`
- `output_flush_us`: median `38285us`, p95 `384604us`
- `crypto_us`: median `20978us`, p95 `33894us`
